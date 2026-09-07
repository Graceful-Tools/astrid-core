//! List chat: channels and messages.
//!
//! Ported from `astrid-ios/Astrid App/Models/ChatMessage.swift`.
//!
//! A message sent offline is created with a `temp_<uuid>` id and keeps that identity — and its
//! position in the transcript — until the Outbox delivers it and the server's id replaces it. The
//! `client_request_id` it carries is what stops a retried send from posting twice.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::date;
use super::task::{CommentType, SecureFile};
use super::user::User;

/// The prefix an id carries while it exists only on this device.
pub const TEMP_ID_PREFIX: &str = "temp_";

/// True for an id this client minted, on a message or anything else that is journalled before it
/// is sent.
pub fn is_temp_id(id: &str) -> bool {
    id.starts_with(TEMP_ID_PREFIX)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatChannel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    /// Set for the channels that belong to a virtual list rather than a stored one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    #[serde(default)]
    pub channel_id: String,
    /// Absent on the messages the server authors itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<User>,
    #[serde(default)]
    pub content: String,
    /// The same TEXT/MARKDOWN/ATTACHMENT enum comments use.
    #[serde(default)]
    pub r#type: CommentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure_files: Option<Vec<SecureFile>>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<DateTime<Utc>>,
}

impl ChatMessage {
    /// Whether an AI agent wrote this.
    pub fn is_from_agent(&self) -> bool {
        self.author.as_ref().map(User::is_agent).unwrap_or(false)
    }

    /// Whether this message exists only on this device so far.
    pub fn is_pending(&self) -> bool {
        is_temp_id(&self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_still_on_this_device_is_pending() {
        let pending: ChatMessage =
            serde_json::from_str(r#"{"id":"temp_abc","channelId":"c1"}"#).expect("decodes");
        assert!(pending.is_pending());

        let delivered: ChatMessage =
            serde_json::from_str(r#"{"id":"m1","channelId":"c1"}"#).expect("decodes");
        assert!(!delivered.is_pending());
    }

    #[test]
    fn an_agents_message_is_recognised_by_its_author() {
        let message: ChatMessage = serde_json::from_str(
            r#"{"id":"m1","channelId":"c1","author":{"id":"a1","isAIAgent":true}}"#,
        )
        .expect("decodes");
        assert!(message.is_from_agent());
    }

    #[test]
    fn a_message_needs_only_an_id() {
        let message: ChatMessage = serde_json::from_str(r#"{"id":"m1"}"#).expect("decodes");
        assert_eq!(message.r#type, CommentType::Text);
        assert!(!message.is_from_agent());
    }
}
