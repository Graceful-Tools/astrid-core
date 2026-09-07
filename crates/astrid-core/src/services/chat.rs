//! List chat.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/ChatService.swift`.
//!
//! A message typed offline takes its place in the transcript immediately and stays where it was
//! typed. That ordering is the whole reason the optimistic message keeps a `created_at` from the
//! moment it was written rather than from when it was delivered: a message that jumps to the
//! bottom of a conversation when the network comes back reads as a different message.

use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;
use crate::model::{date, ChatChannel, ChatMessage, CommentType};
use crate::outbox::{self, journal, kind};

pub struct ChatService {
    context: Context,
}

impl ChatService {
    pub fn new(context: Context) -> Self {
        ChatService { context }
    }

    pub fn channels(&self) -> Result<Vec<ChatChannel>> {
        Ok(self.context.store.channels()?)
    }

    /// The channel for a list, if one has been fetched.
    pub fn channel_for_list(&self, list_id: &str) -> Result<Option<ChatChannel>> {
        Ok(self
            .context
            .store
            .channels()?
            .into_iter()
            .find(|channel| channel.list_id.as_deref() == Some(list_id)))
    }

    pub fn messages(&self, channel_id: &str) -> Result<Vec<ChatMessage>> {
        Ok(self.context.store.messages_in_channel(channel_id)?)
    }

    pub async fn refresh_channels(&self) -> Result<Vec<ChatChannel>> {
        let request = self.context.client.get(endpoints::CHAT_CHANNELS);
        let fetched = self
            .context
            .client
            .send_collection::<ChatChannel>(request, Some(endpoints::envelope::CHANNELS))
            .await?;
        self.context.store.upsert_channels(&fetched.items)?;
        Ok(fetched.into_items())
    }

    pub async fn refresh_messages(&self, channel_id: &str) -> Result<Vec<ChatMessage>> {
        let request = self
            .context
            .client
            .get(endpoints::channel_messages(channel_id));
        let fetched = self
            .context
            .client
            .send_collection::<ChatMessage>(request, Some(endpoints::envelope::MESSAGES))
            .await?;
        self.context.store.upsert_messages(&fetched.items)?;
        Ok(fetched.into_items())
    }

    /// Send a message. It is in the transcript before this returns, at the moment it was typed.
    pub fn send(
        &self,
        channel_id: &str,
        content: &str,
        author_id: Option<&str>,
        reply_to_id: Option<&str>,
    ) -> Result<ChatMessage> {
        let now = self.context.clock.now();
        let temp_id = outbox::new_temp_id();

        let optimistic: ChatMessage = serde_json::from_value(json!({
            "id": temp_id,
            "channelId": channel_id,
            "content": content,
            "type": CommentType::Text,
            "authorId": author_id,
            "replyToId": reply_to_id,
            "clientRequestId": temp_id,
            "createdAt": date::format(now),
        }))
        .expect("a message built from known fields always decodes");
        self.context
            .store
            .upsert_messages(std::slice::from_ref(&optimistic))?;

        let entry = outbox::build(
            kind::SEND_CHAT_MESSAGE,
            json!({
                "channelId": channel_id,
                "body": { "content": content, "replyToId": reply_to_id }
            }),
            &temp_id,
            now,
        )
        .for_temp_id(&temp_id);
        journal::enqueue(&self.context.store, &entry)?;

        Ok(optimistic)
    }

    /// Whether anything in this channel is still waiting to be delivered. What the "sending…"
    /// state in the transcript is drawn from.
    pub fn has_pending(&self, channel_id: &str) -> Result<bool> {
        Ok(self
            .messages(channel_id)?
            .iter()
            .any(ChatMessage::is_pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport};
    use crate::platform::{FixedClock, MemorySecureStore};
    use crate::store::Store;
    use std::sync::Arc;

    struct Fixture {
        service: ChatService,
        store: Arc<Store>,
        clock: Arc<FixedClock>,
    }

    fn fixture(transport: StubTransport) -> Fixture {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let clock = Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                Arc::new(transport),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            clock.clone(),
        );
        Fixture {
            service: context.chat(),
            store,
            clock,
        }
    }

    #[test]
    fn a_message_appears_in_the_transcript_before_it_is_sent() {
        let fixture = fixture(StubTransport::new());
        let sent = fixture
            .service
            .send("c1", "on my way", Some("u1"), None)
            .expect("sends");

        assert!(sent.is_pending());
        assert_eq!(fixture.service.messages("c1").expect("reads").len(), 1);
        assert!(fixture.service.has_pending("c1").expect("reads"));

        let entries = journal::all(&fixture.store).expect("reads");
        assert_eq!(entries[0].kind, kind::SEND_CHAT_MESSAGE);
        assert_eq!(entries[0].payload["channelId"], "c1");
    }

    /// A message that jumps to the bottom when the network returns reads as a different message.
    /// It is timestamped when it was typed, not when it was delivered.
    #[test]
    fn an_offline_message_keeps_the_place_it_was_typed_in() {
        let fixture = fixture(StubTransport::new());
        let queued = fixture
            .service
            .send("c1", "typed first", Some("u1"), None)
            .expect("sends");
        assert_eq!(
            queued.created_at,
            Some(date::parse("2026-09-07T12:00:00Z").expect("an instant"))
        );

        // A message that arrives from the server later, timestamped later, sorts after it.
        fixture.clock.advance(chrono::Duration::minutes(5));
        let arrived: ChatMessage = serde_json::from_value(json!({
            "id": "m2", "channelId": "c1", "content": "arrived second",
            "createdAt": "2026-09-07T12:05:00Z"
        }))
        .expect("decodes");
        fixture
            .store
            .upsert_messages(std::slice::from_ref(&arrived))
            .expect("stores");

        let contents: Vec<String> = fixture
            .service
            .messages("c1")
            .expect("reads")
            .into_iter()
            .map(|message| message.content)
            .collect();
        assert_eq!(contents, vec!["typed first", "arrived second"]);
    }

    #[tokio::test]
    async fn a_channel_can_be_found_by_the_list_it_belongs_to() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/chat/channels",
            200,
            json!({ "channels": [
                { "id": "c1", "listId": "l1" },
                { "id": "c2", "listId": "l2" }
            ] }),
        ));
        fixture.service.refresh_channels().await.expect("refreshes");
        assert_eq!(
            fixture
                .service
                .channel_for_list("l2")
                .expect("reads")
                .expect("present")
                .id,
            "c2"
        );
    }
}
