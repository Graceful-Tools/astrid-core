//! The inbox: what happened to work you are involved in (web task ab0572cb).
//!
//! Derived on the server from task events — assigned to you, mentioned, replied to, commented
//! on, status changed, completed — and never written by a client. The wire shape is what
//! `GET /api/v1/notifications` answers, with the task summary the route embeds.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::date;

/// One row of the inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    /// `assigned` | `mentioned` | `replied` | `commented` | `status_changed` | `completed`, and
    /// whatever a newer server adds — kept as text so an unknown kind still draws.
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<String>,
    /// Who caused it. Never the reader — nobody is told what they just did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub read_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    /// The task it is about, as the route summarises it. Absent when the task is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<NotificationTask>,
}

impl Notification {
    pub fn is_read(&self) -> bool {
        self.read_at.is_some()
    }
}

/// What the inbox knows about a task without opening it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationTask {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub completed: bool,
}

/// The inbox as last fetched: the rows, newest first, and the server's unread count.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    #[serde(default)]
    pub notifications: Vec<Notification>,
    #[serde(default)]
    pub unread_count: usize,
}
