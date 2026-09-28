//! List chat.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/ChatService.swift`.
//!
//! A message typed offline takes its place in the transcript immediately and stays where it was
//! typed. That ordering is the whole reason the optimistic message keeps a `created_at` from the
//! moment it was written rather than from when it was delivered: a message that jumps to the
//! bottom of a conversation when the network comes back reads as a different message.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;
use crate::model::{date, ChatChannel, ChatMessage, CommentType, SecureFile};
use crate::outbox::{self, journal, kind};

/// How many messages a page asks for — the Apple apps' size, and the web's.
pub const PAGE_SIZE: u32 = 50;

/// A message to send, beyond its words.
#[derive(Debug, Default, Clone)]
pub struct Outgoing<'a> {
    pub content: &'a str,
    pub message_type: CommentType,
    /// The file it carries, already queued — see [`crate::services::AttachmentService::queue_as`].
    pub file: Option<&'a SecureFile>,
    pub reply_to_id: Option<&'a str>,
    pub author_id: Option<&'a str>,
    /// The id a shell already drew the message under. A temporary one becomes the message's id
    /// until the server answers; anything else is ignored and one is minted.
    pub id: Option<&'a str>,
}

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
        self.send_as(
            channel_id,
            Outgoing {
                content,
                author_id,
                reply_to_id,
                ..Outgoing::default()
            },
        )
    }

    /// [`Self::send`], with everything a message can carry: a type, a file, the id a shell drew it
    /// under.
    pub fn send_as(&self, channel_id: &str, message: Outgoing<'_>) -> Result<ChatMessage> {
        let now = self.context.clock.now();
        let temp_id = message
            .id
            .filter(|id| crate::model::is_temp_id(id))
            .map(str::to_string)
            .unwrap_or_else(outbox::new_temp_id);

        let optimistic: ChatMessage = serde_json::from_value(json!({
            "id": temp_id,
            "channelId": channel_id,
            "content": message.content,
            "type": message.message_type,
            "authorId": message.author_id,
            "replyToId": message.reply_to_id,
            "clientRequestId": temp_id,
            "createdAt": date::format(now),
            // On the optimistic message so the picture is in the transcript the moment it is sent.
            "secureFiles": message.file.map(std::slice::from_ref),
        }))
        .expect("a message built from known fields always decodes");
        self.context
            .store
            .upsert_messages(std::slice::from_ref(&optimistic))?;

        let entry = outbox::build(
            kind::SEND_CHAT_MESSAGE,
            json!({
                "channelId": channel_id,
                "body": {
                    "content": message.content,
                    "type": message.message_type,
                    "fileId": message.file.map(|file| file.id.clone()),
                    "replyToId": message.reply_to_id,
                }
            }),
            &temp_id,
            now,
        )
        .for_temp_id(&temp_id);
        journal::enqueue(&self.context.store, &entry)?;

        Ok(optimistic)
    }

    /// The channel for a list — or, with `virtual_key`, for a virtual list such as My Tasks —
    /// from the cache, else asked of the server, which creates it on first use.
    pub async fn resolve_channel(
        &self,
        list_id: Option<&str>,
        virtual_key: Option<&str>,
    ) -> Result<ChatChannel> {
        let cached = self.context.store.channels()?.into_iter().find(|channel| {
            match (list_id, virtual_key) {
                (_, Some(key)) => channel.virtual_key.as_deref() == Some(key),
                (Some(list_id), None) => channel.list_id.as_deref() == Some(list_id),
                (None, None) => false,
            }
        });
        if let Some(channel) = cached {
            return Ok(channel);
        }
        let body = match virtual_key {
            Some(key) => json!({ "virtualKey": key }),
            None => json!({ "listId": list_id }),
        };
        let request = self
            .context
            .client
            .post(endpoints::CHAT_CHANNELS)
            .value(body);
        let value = self.context.client.send(request).await?;
        let channel: ChatChannel =
            serde_json::from_value(value.get("channel").cloned().unwrap_or(value))
                .map_err(|error| crate::api::ApiError::Decode(error.to_string()))?;
        self.context
            .store
            .upsert_channels(std::slice::from_ref(&channel))?;
        Ok(channel)
    }

    /// Fetch a page of a channel's history — the newest, or the one before `before` — into the
    /// cache. Answers whether there is more before it.
    ///
    /// The page is authoritative only for the stretch of time it covers, so only there does a
    /// cached message it did not return count as deleted: see [`stale_ids`].
    pub async fn load_messages(
        &self,
        channel_id: &str,
        before: Option<DateTime<Utc>>,
        limit: Option<u32>,
    ) -> Result<bool> {
        let request = self
            .context
            .client
            .get(endpoints::channel_messages(channel_id))
            .query("limit", Some(limit.unwrap_or(PAGE_SIZE).to_string()))
            .query("before", before.map(date::format));
        let value = self.context.client.send(request).await?;
        let has_more = value
            .get("hasMore")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let page: Vec<ChatMessage> = crate::model::lenient(
            value
                .get(endpoints::envelope::MESSAGES)
                .cloned()
                .unwrap_or_default(),
        )
        .into_items();
        let window = Window {
            covers_whole_channel: before.is_none() && !has_more,
            ..Window::of(&page)
        };
        let stale = stale_ids(
            &window,
            &self.context.store.messages_in_channel(channel_id)?,
        );
        self.context.store.upsert_messages(&page)?;
        for id in stale {
            self.context.store.delete_message(&id)?;
        }
        Ok(has_more)
    }

    /// Take a message out of this device's transcript. Chat has no delete on the server; this is
    /// the Apple apps' local hide. A message still waiting to be sent is not sent.
    pub fn forget(&self, message_id: &str) -> Result<()> {
        if crate::model::is_temp_id(message_id) {
            for entry in journal::all(&self.context.store)? {
                if entry.kind == kind::SEND_CHAT_MESSAGE
                    && entry.temp_id.as_deref() == Some(message_id)
                    && entry.status == outbox::Status::Pending
                {
                    journal::remove(&self.context.store, &entry.id)?;
                }
            }
        }
        self.context.store.delete_message(message_id)?;
        Ok(())
    }

    /// Post, as the agent, an answer this device produced itself — the on-device model's. Online:
    /// an answer that arrives an hour late answers a conversation that has moved on.
    pub async fn post_agent_response(&self, channel_id: &str, content: &str) -> Result<()> {
        let request = self
            .context
            .client
            .post(endpoints::channel_agent_response(channel_id))
            .value(json!({ "content": content }));
        self.context.client.send(request).await?;
        Ok(())
    }

    /// Ask the server to answer as Astrid, when this device cannot (task 9dce4c73). `message_id`
    /// is what the server dedupes on, so a retry does not produce two replies.
    pub async fn request_astrid_response(
        &self,
        channel_id: &str,
        message_id: Option<&str>,
        content: &str,
    ) -> Result<()> {
        let request = self
            .context
            .client
            .post(endpoints::channel_astrid_response(channel_id))
            .value(json!({ "message_id": message_id, "content": content }));
        self.context.client.send(request).await?;
        Ok(())
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

/// The stretch of a channel's history one fetched page can speak for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Window {
    pub ids: std::collections::HashSet<String>,
    pub oldest: Option<DateTime<Utc>>,
    /// Both bounds matter: an older page sits entirely below what is cached, and a lower bound
    /// alone would let it prune everything above it — including a message the stream delivered
    /// mid-fetch.
    pub newest: Option<DateTime<Utc>>,
    /// The first page with nothing behind it: the whole channel, where absence anywhere is
    /// evidence of deletion.
    pub covers_whole_channel: bool,
}

impl Window {
    pub fn of(page: &[ChatMessage]) -> Window {
        Window {
            ids: page.iter().map(|message| message.id.clone()).collect(),
            oldest: page.iter().filter_map(|message| message.created_at).min(),
            newest: page.iter().filter_map(|message| message.created_at).max(),
            covers_whole_channel: false,
        }
    }
}

/// Cached messages a fetched page shows the server no longer has (Apple AITD-354).
///
/// Chat is paged, so the comment rule — absent means deleted — would wipe every cached message
/// older than the newest fifty on each refresh. A message goes only when it has been delivered
/// (never an unsent one), the page did not return it, and it falls inside the page's window —
/// or the page is the whole channel. One with no timestamp cannot be placed, so it goes only then.
pub fn stale_ids(window: &Window, cached: &[ChatMessage]) -> Vec<String> {
    if !window.covers_whole_channel && window.oldest.is_none() {
        // An empty page past the beginning says nothing about what came before it.
        return Vec::new();
    }
    cached
        .iter()
        .filter(|message| !message.is_pending() && !window.ids.contains(&message.id))
        .filter(|message| {
            window.covers_whole_channel
                || matches!(
                    (window.oldest, window.newest, message.created_at),
                    (Some(oldest), Some(newest), Some(created)) if created >= oldest && created <= newest
                )
        })
        .map(|message| message.id.clone())
        .collect()
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
        sent: Arc<std::sync::Mutex<Vec<crate::api::HttpRequest>>>,
    }

    impl Fixture {
        /// Every request made, as `METHOD url body`.
        fn requests(&self) -> Vec<String> {
            self.sent
                .lock()
                .expect("not poisoned")
                .iter()
                .map(|request| {
                    let body = request
                        .body
                        .as_ref()
                        .map(|body| String::from_utf8_lossy(body).to_string())
                        .unwrap_or_default();
                    format!("{:?} {} {}", request.method, request.url, body)
                })
                .collect()
        }

        fn ids(&self, channel_id: &str) -> Vec<String> {
            self.service
                .messages(channel_id)
                .expect("reads")
                .into_iter()
                .map(|message| message.id)
                .collect()
        }

        fn cache(&self, messages: serde_json::Value) {
            let messages: Vec<ChatMessage> = serde_json::from_value(messages).expect("decodes");
            self.store.upsert_messages(&messages).expect("stores");
        }
    }

    fn fixture(transport: StubTransport) -> Fixture {
        let sent = transport.recorded.clone();
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
            sent,
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

    /// The stream can deliver the server's copy of a message before the send's own answer does.
    /// It carries the optimistic message's `clientRequestId`, and it replaces that message rather
    /// than joining it — two copies of what somebody said, one stuck on "sending" (the Apple
    /// apps' `handleMessageCreated` did this by hand).
    #[test]
    fn the_servers_copy_replaces_the_message_it_echoes() {
        let fixture = fixture(StubTransport::new());
        let sent = fixture
            .service
            .send("c1", "on my way", Some("u1"), None)
            .expect("sends");
        let echoed: ChatMessage = serde_json::from_value(json!({
            "id": "m1", "channelId": "c1", "content": "on my way",
            "clientRequestId": sent.client_request_id, "createdAt": "2026-09-07T12:00:01Z"
        }))
        .expect("decodes");
        fixture
            .store
            .upsert_messages(std::slice::from_ref(&echoed))
            .expect("stores");

        let ids: Vec<String> = fixture
            .service
            .messages("c1")
            .expect("reads")
            .into_iter()
            .map(|message| message.id)
            .collect();
        assert_eq!(ids, vec!["m1"]);
        // And whatever still names the optimistic id — a reply to it — finds the real one.
        assert_eq!(fixture.store.resolve_id(&sent.id).expect("resolves"), "m1");
    }

    // ── What the Apple apps' chat needed ────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_channel_is_resolved_from_the_cache_before_the_server() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/chat/channels",
            200,
            json!({ "channel": { "id": "c9", "listId": "l1" } }),
        ));
        let first = fixture
            .service
            .resolve_channel(Some("l1"), None)
            .await
            .expect("resolves");
        let again = fixture
            .service
            .resolve_channel(Some("l1"), None)
            .await
            .expect("resolves from the cache");
        assert_eq!((first.id.as_str(), again.id.as_str()), ("c9", "c9"));
        let requests = fixture.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(requests[0].contains(r#""listId":"l1""#), "{requests:?}");
    }

    /// My Tasks has no list row: its channel is named by a virtual key.
    #[tokio::test]
    async fn a_virtual_lists_channel_is_resolved_by_its_key() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/chat/channels",
            200,
            json!({ "channel": { "id": "c7", "virtualKey": "virtual-chat:u1:my-tasks" } }),
        ));
        let channel = fixture
            .service
            .resolve_channel(None, Some("virtual-chat:u1:my-tasks"))
            .await
            .expect("resolves");
        assert_eq!(channel.id, "c7");
        assert!(fixture.requests()[0].contains(r#""virtualKey":"virtual-chat:u1:my-tasks""#));
    }

    #[tokio::test]
    async fn the_newest_page_is_fetched_and_says_whether_there_is_more() {
        let fixture = fixture(StubTransport::new().push_json(
            "/messages",
            200,
            json!({ "messages": [
                { "id": "m1", "channelId": "c1", "content": "hi", "createdAt": "2026-09-07T11:00:00Z" }
            ], "hasMore": true, "nextCursor": "2026-09-07T11:00:00Z" }),
        ));
        let more = fixture
            .service
            .load_messages("c1", None, None)
            .await
            .expect("loads");
        assert!(more);
        assert_eq!(fixture.ids("c1"), vec!["m1"]);
        assert!(fixture.requests()[0].contains("limit=50"));
    }

    /// Paging back names the oldest message the device holds, in the wire's date format.
    #[tokio::test]
    async fn an_older_page_is_asked_for_by_its_cursor() {
        let fixture = fixture(StubTransport::new().push_json(
            "/messages",
            200,
            json!({ "messages": [], "hasMore": false }),
        ));
        let before = date::parse("2026-09-07T11:00:00Z").expect("an instant");
        fixture
            .service
            .load_messages("c1", Some(before), Some(20))
            .await
            .expect("loads");
        let request = &fixture.requests()[0];
        assert!(request.contains("limit=20"), "{request}");
        assert!(
            request.contains("before=2026-09-07T11%3A00%3A00"),
            "{request}"
        );
    }

    /// Inside the page's window the server is right; outside it, nothing is known. An unsent
    /// message is never taken (AITD-354).
    #[tokio::test]
    async fn a_page_prunes_only_what_it_can_speak_for() {
        let fixture = fixture(StubTransport::new().push_json(
            "/messages",
            200,
            json!({ "messages": [
                { "id": "m2", "channelId": "c1", "createdAt": "2026-09-07T10:00:00Z" },
                { "id": "m4", "channelId": "c1", "createdAt": "2026-09-07T12:00:00Z" }
            ], "hasMore": true }),
        ));
        fixture.cache(json!([
            { "id": "m1", "channelId": "c1", "createdAt": "2026-09-07T09:00:00Z" },
            { "id": "m3", "channelId": "c1", "createdAt": "2026-09-07T11:00:00Z" },
            { "id": "m5", "channelId": "c1", "createdAt": "2026-09-07T13:00:00Z" },
            { "id": "temp_x", "channelId": "c1", "createdAt": "2026-09-07T11:30:00Z" }
        ]));
        fixture
            .service
            .load_messages("c1", None, None)
            .await
            .expect("loads");
        // m3 was deleted on the server; m1 is older history, m5 arrived mid-fetch.
        assert_eq!(fixture.ids("c1"), vec!["m1", "m2", "temp_x", "m4", "m5"]);
    }

    #[tokio::test]
    async fn the_whole_channel_prunes_everything_it_does_not_hold() {
        let fixture = fixture(StubTransport::new().push_json(
            "/messages",
            200,
            json!({ "messages": [], "hasMore": false }),
        ));
        fixture.cache(json!([
            { "id": "m1", "channelId": "c1", "createdAt": "2026-09-07T09:00:00Z" },
            { "id": "m2", "channelId": "c1" }
        ]));
        fixture
            .service
            .load_messages("c1", None, None)
            .await
            .expect("loads");
        assert!(fixture.ids("c1").is_empty());
    }

    /// A picture sent in chat is in the transcript at once, under the id its thumbnail was drawn
    /// with, and its send names the file so the journal can swap in the real id.
    #[test]
    fn a_message_carries_its_file_type_and_row_id() {
        let fixture = fixture(StubTransport::new());
        let file = SecureFile {
            id: "temp_file".into(),
            name: "photo.jpg".into(),
            size: 3,
            mime_type: "image/jpeg".into(),
        };
        let sent = fixture
            .service
            .send_as(
                "c1",
                Outgoing {
                    content: "",
                    message_type: CommentType::Attachment,
                    file: Some(&file),
                    id: Some("temp_row"),
                    ..Outgoing::default()
                },
            )
            .expect("sends");
        assert_eq!(sent.id, "temp_row");
        assert_eq!(
            sent.secure_files.as_deref(),
            Some(std::slice::from_ref(&file))
        );
        let entry = &journal::all(&fixture.store).expect("reads")[0];
        assert_eq!(entry.payload["body"]["fileId"], "temp_file");
        assert_eq!(entry.payload["body"]["type"], "ATTACHMENT");
        assert_eq!(entry.client_request_id, "temp_row");
    }

    /// Chat has no server delete: forgetting is local. A message not yet sent is not sent.
    #[test]
    fn forgetting_an_unsent_message_withdraws_it() {
        let fixture = fixture(StubTransport::new());
        let sent = fixture
            .service
            .send("c1", "oops", None, None)
            .expect("sends");
        fixture.service.forget(&sent.id).expect("forgets");
        assert!(fixture.ids("c1").is_empty());
        assert!(journal::all(&fixture.store).expect("reads").is_empty());
    }

    #[tokio::test]
    async fn astrid_is_asked_to_answer_with_the_message_it_answers() {
        let fixture = fixture(StubTransport::new().push_json(
            "/astrid-response",
            200,
            json!({ "message": {} }),
        ));
        fixture
            .service
            .request_astrid_response("c1", Some("m1"), "@astrid hi")
            .await
            .expect("asks");
        let request = &fixture.requests()[0];
        assert!(request.contains("/api/v1/chat/channels/c1/astrid-response"));
        assert!(request.contains(r#""message_id":"m1""#), "{request}");
    }
}
