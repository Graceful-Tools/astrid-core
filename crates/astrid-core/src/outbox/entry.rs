//! One unit of offline-first work.
//!
//! Ported from `astrid-ios/Astrid App/Core/Outbox/OutboxEntry.swift`.
//!
//! The Outbox replaced a pending queue per service — tasks, comments, chat, attachments — with one
//! journal and one runner. Each entry carries everything the handler needs, an idempotency key so
//! a retry never duplicates server-side, and explicit dependency edges, so a comment waits for its
//! attachment upload *by construction* rather than by throwing, observing and trying again.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where an entry is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Waiting to run, or waiting out a backoff.
    Pending,
    /// A handler has it right now.
    Running,
    /// Done. Kept a while so dependents can still resolve against it.
    Completed,
    /// Dead-lettered: refused by the server, or out of attempts.
    FailedPermanent,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Completed => "completed",
            Status::FailedPermanent => "failedPermanent",
        }
    }

    /// Anything unrecognised reads as pending. A journal written by a newer build must not wedge
    /// an older one, and pending is the only state that can still make progress.
    ///
    /// Deliberately not `FromStr`: that trait is fallible by contract, and this is the opposite —
    /// it cannot fail, and the leniency is the point.
    pub fn from_wire(raw: &str) -> Self {
        match raw {
            "running" => Status::Running,
            "completed" => Status::Completed,
            "failedPermanent" => Status::FailedPermanent,
            _ => Status::Pending,
        }
    }
}

/// The handler keys. Strings on the wire and in the journal, so an entry written by one build is
/// still legible to the next; named here so a typo is a compile error rather than an entry no
/// handler ever claims.
pub mod kind {
    pub const CREATE_TASK: &str = "createTask";
    pub const UPDATE_TASK: &str = "updateTask";
    pub const DELETE_TASK: &str = "deleteTask";
    pub const COMPLETE_TASK: &str = "completeTask";
    pub const CREATE_COMMENT: &str = "createComment";
    pub const UPDATE_COMMENT: &str = "updateComment";
    pub const DELETE_COMMENT: &str = "deleteComment";
    pub const SEND_CHAT_MESSAGE: &str = "sendChatMessage";
    pub const CREATE_LIST: &str = "createList";
    pub const UPDATE_LIST: &str = "updateList";
    pub const DELETE_LIST: &str = "deleteList";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    /// Which handler runs this. See [`kind`].
    pub kind: String,
    /// Everything the handler needs, self-contained. Self-contained matters: the entry has to be
    /// runnable weeks later, on a launch where nothing else about the app's state survived.
    pub payload: serde_json::Value,
    /// The idempotency key sent to the server. A retry of a request the server already applied
    /// returns the same row rather than making a second one.
    pub client_request_id: String,
    /// Entries that must complete before this one may run.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// The optimistic task id this entry produces (a create) or consumes (an edit to a task that
    /// has not been delivered yet).
    ///
    /// Without it, an edit to a not-yet-created task has no dependency edge to strand on, so a
    /// create that dead-letters leaves its edits pending forever — see
    /// [`super::scheduler::temp_stranded`].
    #[serde(default)]
    pub temp_id: Option<String>,
    pub status: Status,
    pub attempts: i64,
    /// The earliest this may run again.
    pub next_attempt_at: DateTime<Utc>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// What the handler produced, for dependents to read — the real file id from an upload, the
    /// server id from a create.
    #[serde(default)]
    pub result: Option<BTreeMap<String, String>>,
    /// The journal's own ordering. Assigned on insert; two entries created in the same millisecond
    /// still have the order the person performed them in.
    #[serde(default)]
    pub sequence: i64,
}

impl Entry {
    /// A new entry, due immediately.
    pub fn new(
        id: impl Into<String>,
        kind: impl Into<String>,
        payload: serde_json::Value,
        client_request_id: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Entry {
            id: id.into(),
            kind: kind.into(),
            payload,
            client_request_id: client_request_id.into(),
            depends_on: Vec::new(),
            temp_id: None,
            status: Status::Pending,
            attempts: 0,
            next_attempt_at: now,
            last_error: None,
            created_at: now,
            updated_at: now,
            result: None,
            sequence: 0,
        }
    }

    pub fn depending_on(mut self, ids: impl IntoIterator<Item = String>) -> Self {
        self.depends_on = ids.into_iter().collect();
        self
    }

    pub fn for_temp_id(mut self, temp_id: impl Into<String>) -> Self {
        self.temp_id = Some(temp_id.into());
        self
    }

    /// The lane this entry serialises in.
    ///
    /// Explicit dependencies gate execution separately; this key is what keeps repeated writes to
    /// the *same* task, comment thread or channel in the order they were made, while letting
    /// unrelated entities go at once. Two edits to one task racing each other is how "my rename
    /// came back" happens.
    pub fn serialization_key(&self) -> String {
        if let Some(temp_id) = &self.temp_id {
            return format!("task:{temp_id}");
        }
        let field = |name: &str| {
            self.payload
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let lane = match self.kind.as_str() {
            kind::CREATE_TASK | kind::UPDATE_TASK | kind::DELETE_TASK | kind::COMPLETE_TASK => {
                field("taskId").map(|id| format!("task:{id}"))
            }
            kind::CREATE_COMMENT => field("taskId").map(|id| format!("task-comments:{id}")),
            kind::UPDATE_COMMENT | kind::DELETE_COMMENT => {
                field("commentId").map(|id| format!("comment:{id}"))
            }
            kind::SEND_CHAT_MESSAGE => field("channelId").map(|id| format!("channel:{id}")),
            kind::CREATE_LIST | kind::UPDATE_LIST | kind::DELETE_LIST => {
                field("listId").map(|id| format!("list:{id}"))
            }
            _ => None,
        };
        // An entry whose payload does not name its subject gets a lane of its own rather than
        // sharing one with every other unnameable entry, which would serialise them all.
        lane.unwrap_or_else(|| format!("entry:{}", self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn now() -> DateTime<Utc> {
        date::parse("2026-09-07T12:00:00Z").expect("an instant")
    }

    fn entry(kind: &str, payload: serde_json::Value) -> Entry {
        Entry::new("e1", kind, payload, "crid", now())
    }

    #[test]
    fn writes_to_the_same_task_share_a_lane() {
        let update = entry(kind::UPDATE_TASK, serde_json::json!({ "taskId": "t1" }));
        let complete = entry(kind::COMPLETE_TASK, serde_json::json!({ "taskId": "t1" }));
        assert_eq!(update.serialization_key(), complete.serialization_key());
    }

    #[test]
    fn writes_to_different_entities_do_not() {
        let a = entry(kind::UPDATE_TASK, serde_json::json!({ "taskId": "t1" }));
        let b = entry(kind::UPDATE_TASK, serde_json::json!({ "taskId": "t2" }));
        let comment = entry(
            kind::UPDATE_COMMENT,
            serde_json::json!({ "commentId": "c1" }),
        );
        assert_ne!(a.serialization_key(), b.serialization_key());
        assert_ne!(a.serialization_key(), comment.serialization_key());
    }

    /// A temp id wins over the payload: every write to a task that has not been created yet
    /// belongs in the same lane as the create, whatever the payload calls it.
    #[test]
    fn a_temp_id_names_the_lane() {
        let create = entry(kind::CREATE_TASK, serde_json::json!({})).for_temp_id("temp_1");
        let update = entry(kind::UPDATE_TASK, serde_json::json!({ "taskId": "temp_1" }))
            .for_temp_id("temp_1");
        assert_eq!(create.serialization_key(), "task:temp_1");
        assert_eq!(update.serialization_key(), "task:temp_1");
    }

    /// Every unnameable entry sharing one lane would serialise writes that have nothing to do with
    /// each other.
    #[test]
    fn an_entry_that_names_no_subject_gets_its_own_lane() {
        let a = Entry::new("a", "somethingNew", serde_json::json!({}), "c", now());
        let b = Entry::new("b", "somethingNew", serde_json::json!({}), "c", now());
        assert_ne!(a.serialization_key(), b.serialization_key());
    }

    /// A journal written by a newer build must not wedge an older one; pending is the only state
    /// that can still make progress.
    #[test]
    fn an_unknown_status_reads_as_pending() {
        assert_eq!(Status::from_wire("somethingLater"), Status::Pending);
        for status in [
            Status::Pending,
            Status::Running,
            Status::Completed,
            Status::FailedPermanent,
        ] {
            assert_eq!(Status::from_wire(status.as_str()), status);
        }
    }
}
