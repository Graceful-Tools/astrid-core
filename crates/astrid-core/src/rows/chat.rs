//! What a chat message shows.
//!
//! Ports the parts of `astrid-ios/Astrid Mac/Views/MacChatBubbleStyle.swift` and its neighbours
//! that are decisions rather than drawing: whose message it is, whether it has been delivered, and
//! whether anybody said it at all.
//!
//! Three of those look trivial and are not:
//!
//! - **Mine or theirs** decides which side a bubble sits on, and it is `author_id` against the
//!   signed-in user — not the author's *name*, which two people can share.
//! - **Delivered or not** is the temporary id, the same signal the task rows use. A message typed
//!   offline is in the transcript at the moment it was typed and stays where it was typed; the
//!   only thing that changes when it lands is that it stops saying "sending".
//! - **Said by nobody** is a message the server authored — "Dana joined the list". Drawing it as a
//!   bubble from a person with no name is how those come to look like somebody's message.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::model::{is_temp_id, ChatMessage, User};

/// One message, as the shell draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: String,
    pub content: String,
    /// Who said it, or `None` when the server did.
    pub author_name: Option<String>,
    /// Avatar fallback. Empty for a message nobody said.
    pub initials: String,
    pub image: Option<String>,
    /// Whether the signed-in account wrote it. Which side the bubble sits on.
    pub is_mine: bool,
    /// Still in the Outbox. Not an error — see the module note.
    pub is_pending: bool,
    /// Written by the server rather than by a person.
    pub is_system: bool,
    pub reply_to_id: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

/// Project a transcript.
///
/// `people` is whatever the cache holds; a message's own embedded author wins over it, because it
/// is the record the server sent with that message.
pub fn transcript(
    messages: &[ChatMessage],
    current_user_id: Option<&str>,
    people: &[User],
) -> Vec<MessageRow> {
    messages
        .iter()
        .map(|message| {
            let author = message.author.clone().or_else(|| {
                message
                    .author_id
                    .as_deref()
                    .and_then(|id| people.iter().find(|user| user.id == id).cloned())
            });
            MessageRow {
                id: message.id.clone(),
                content: message.content.clone(),
                // `known_name` rather than `display_name`, whose fallback is English. An author
                // we know nothing about but an id is named by the shell.
                author_name: author
                    .as_ref()
                    .and_then(|user| user.known_name())
                    .map(str::to_string),
                initials: author
                    .as_ref()
                    .map(|user| user.initials())
                    .unwrap_or_default(),
                image: author.and_then(|user| user.image),
                is_mine: match (message.author_id.as_deref(), current_user_id) {
                    (Some(author), Some(me)) => author == me,
                    _ => false,
                },
                is_pending: is_temp_id(&message.id),
                // Nobody said it: the server did.
                is_system: message.author_id.is_none(),
                reply_to_id: message.reply_to_id.clone(),
                created_at: message.created_at,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(id: &str, author: Option<&str>, content: &str) -> ChatMessage {
        serde_json::from_value(json!({
            "id": id,
            "channelId": "c1",
            "authorId": author,
            "content": content,
        }))
        .expect("decodes")
    }

    fn user(id: &str, name: &str) -> User {
        User {
            name: Some(name.into()),
            ..User::new(id)
        }
    }

    /// Which side a bubble sits on is the author's id, not their name: two people can share one.
    #[test]
    fn a_message_is_mine_by_id() {
        let people = vec![user("me", "Jon"), user("dana", "Jon")];
        let rows = transcript(
            &[
                message("m1", Some("me"), "morning"),
                message("m2", Some("dana"), "morning"),
            ],
            Some("me"),
            &people,
        );
        assert!(rows[0].is_mine);
        assert!(!rows[1].is_mine);
    }

    /// A message typed offline is in the transcript from the moment it was typed. The only thing
    /// that changes when it lands is that it stops saying "sending".
    #[test]
    fn a_message_still_in_the_outbox_says_so() {
        let rows = transcript(
            &[
                message("temp_abc", Some("me"), "typed on a train"),
                message("m2", Some("me"), "sent"),
            ],
            Some("me"),
            &[],
        );
        assert!(rows[0].is_pending);
        assert!(!rows[1].is_pending);
    }

    /// "Dana joined the list" is not somebody's message, and drawing it as one from a person with
    /// no name is how it comes to look like one.
    #[test]
    fn a_message_nobody_said_is_marked_as_the_servers() {
        let rows = transcript(
            &[message("m1", None, "Dana joined the list")],
            Some("me"),
            &[],
        );
        assert!(rows[0].is_system);
        assert_eq!(rows[0].author_name, None);
        assert!(rows[0].initials.is_empty());
        assert!(!rows[0].is_mine);
    }

    #[test]
    fn an_author_is_named_from_the_cache_when_the_message_carries_no_record() {
        let rows = transcript(
            &[message("m1", Some("dana"), "hello")],
            Some("me"),
            &[user("dana", "Dana Scully")],
        );
        assert_eq!(rows[0].author_name.as_deref(), Some("Dana Scully"));
        assert_eq!(rows[0].initials, "DS");
    }

    /// The record the server sent with the message wins over whatever the cache happens to hold.
    #[test]
    fn the_messages_own_author_wins() {
        let mut carried = message("m1", Some("dana"), "hello");
        carried.author = Some(user("dana", "Dana Scully"));
        let rows = transcript(&[carried], Some("me"), &[user("dana", "Stale Name")]);
        assert_eq!(rows[0].author_name.as_deref(), Some("Dana Scully"));
    }

    /// Signed out, nothing is mine — rather than everything with no author.
    #[test]
    fn nothing_is_mine_when_nobody_is_signed_in() {
        let rows = transcript(&[message("m1", Some("dana"), "hello")], None, &[]);
        assert!(!rows[0].is_mine);
    }
}
