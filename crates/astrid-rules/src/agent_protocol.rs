//! Typed reads of the agent protocol's two answers: the queue (`GET /api/v1/agent-queue`) and the
//! claim (`POST /api/v1/tasks/:id/claim-fixall`) (AWTD-1550).
//!
//! The shapes are astrid-web's, pinned by `contracts/fixtures/agent-queue.json` and
//! `fixall-claim.json` and checked in `tests/agent_contract.rs`. These are projections for a
//! client that wants types instead of `serde_json::Value`; nothing here replaces a `Value`-returning
//! call, and every field a later server adds is ignored rather than refused (no
//! `deny_unknown_fields`), so a client pinned to this revision keeps decoding newer answers.
//! Collections default to empty and the newer fields are optional, so an older server's answer
//! decodes too.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who the queue was read for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueAgent {
    pub mailbox: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// One task the queue hands over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueTask {
    pub id: String,
    #[serde(default)]
    pub identifier: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub due_date_time: Option<String>,
    #[serde(default)]
    pub list_id: Option<String>,
    #[serde(default)]
    pub list_name: Option<String>,
    /// False for an unassigned Ready task the caller may claim.
    #[serde(default = "assigned_default")]
    pub assigned: bool,
    #[serde(default)]
    pub url: Option<String>,
}

fn assigned_default() -> bool {
    true
}

/// A task held back by its date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    pub id: String,
    pub title: String,
    pub starts_at: String,
}

/// What the queue held back, and why — so a queue waiting on the clock never reads as idle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueHeld {
    #[serde(default)]
    pub not_due_count: u64,
    #[serde(default)]
    pub not_ready_count: u64,
    #[serde(default)]
    pub scheduled: Vec<ScheduledTask>,
}

/// The newest human comment on a task the agent has not answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionComment {
    pub id: String,
    #[serde(default)]
    pub author_id: Option<String>,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub excerpt: Option<String>,
}

/// A task with an unanswered human comment, in any state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionTask {
    pub id: String,
    #[serde(default)]
    pub identifier: Option<String>,
    pub title: String,
    #[serde(default)]
    pub status_role: Option<String>,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub url: Option<String>,
    pub last_comment: AttentionComment,
    /// Present only for a human reply to this agent's explicit handback; kept as sent.
    #[serde(default)]
    pub resume: Option<Value>,
}

/// A list-chat message to the agent since it last spoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionMessage {
    pub id: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub author_id: Option<String>,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// The agent's inbox: what was said to it and not answered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    #[serde(default)]
    pub tasks: Vec<AttentionTask>,
    #[serde(default)]
    pub messages: Vec<AttentionMessage>,
    #[serde(default)]
    pub truncated: bool,
    /// Halves that were not read, and why. An unread channel is not a quiet one.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// Why the verdict says there is work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunnableReason {
    Ready,
    Attention,
    /// A reason a later server adds. Ignored by a client that does not know it.
    #[serde(other)]
    Unknown,
}

/// The versioned runnable verdict (web AWTD-1560). `incomplete` is never a reason to start work
/// on its own: a token without the chat scopes reports it on every poll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runnable {
    pub version: u32,
    pub has_work: bool,
    #[serde(default)]
    pub reasons: Vec<RunnableReason>,
    #[serde(default)]
    pub incomplete: bool,
}

/// The queue answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQueue {
    pub agent: QueueAgent,
    #[serde(default)]
    pub mode: Option<String>,
    pub empty: bool,
    #[serde(default)]
    pub queue: Vec<QueueTask>,
    /// Present when the queue is empty: the condition that is unmet.
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub held: QueueHeld,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub attention: Attention,
    /// Absent on servers before AWTD-1560.
    #[serde(default)]
    pub runnable: Option<Runnable>,
}

/// What a claim answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// 2xx with `claimed: true`: the task is this caller's, moved to Doing.
    Claimed {
        task_id: String,
        agent: Option<String>,
    },
    /// 409: a peer claimed it first, or it is no longer eligible. Move to the next task.
    Conflict { message: Option<String> },
    /// 403: the caller may not claim this task.
    Forbidden { message: Option<String> },
    /// Anything else, kept so a caller can report it rather than guess.
    Other { status: u16, body: Value },
}

impl ClaimOutcome {
    /// Read a claim response by its status and JSON body.
    pub fn from_response(status: u16, body: &Value) -> Self {
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
        match status {
            200..=299 if body.get("claimed").and_then(Value::as_bool) == Some(true) => {
                ClaimOutcome::Claimed {
                    task_id: text("taskId").unwrap_or_default(),
                    agent: text("agent"),
                }
            }
            409 => ClaimOutcome::Conflict {
                message: text("error"),
            },
            403 => ClaimOutcome::Forbidden {
                message: text("message").or_else(|| text("error")),
            },
            _ => ClaimOutcome::Other {
                status,
                body: body.clone(),
            },
        }
    }
}
