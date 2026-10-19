//! The task, and everything hanging off it.
//!
//! Ported from `astrid-ios/Astrid App/Models/Task.swift`. The field set is the wire shape, not a
//! tidied version of it: the server is permissive, three clients read the same JSON, and a field
//! renamed here to read better is a field that silently stops round-tripping.
//!
//! **Decoding is lenient by policy.** Every optional field is `#[serde(default)]`, and the enum
//! that has been seen carrying a value outside its range falls back rather than failing. The
//! reason is on `Priority` below, and it is not hypothetical.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use super::date;
use super::user::User;

/// How urgent, 0-3.
///
/// **Decoded leniently.** The server's schema puts no cap on `priority`, and two production tasks
/// were observed carrying `4`. Swift's default enum decode threw on those, which failed the decode
/// of the WHOLE tasks array and silently broke sync for that account. An unknown value reads as
/// [`Priority::None`] so the task stays usable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(into = "i64")]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Priority {
    pub fn as_i64(self) -> i64 {
        match self {
            Priority::None => 0,
            Priority::Low => 1,
            Priority::Medium => 2,
            Priority::High => 3,
        }
    }

    /// Anything outside 0-3 is [`Priority::None`]. See the type's doc comment.
    pub fn from_i64(raw: i64) -> Self {
        match raw {
            1 => Priority::Low,
            2 => Priority::Medium,
            3 => Priority::High,
            _ => Priority::None,
        }
    }
}

impl From<Priority> for i64 {
    fn from(value: Priority) -> Self {
        value.as_i64()
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Priority::from_i64(i64::deserialize(deserializer)?))
    }
}

/// The repeat preset. `Custom` defers to [`CustomRepeatingPattern`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeating {
    Never,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    Custom,
}

impl Repeating {
    /// True when the task rolls over on completion rather than simply being done.
    pub fn repeats(self) -> bool {
        !matches!(self, Repeating::Never)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderType {
    Push,
    Email,
    Both,
}

/// Whether the next occurrence is measured from the due date or from when it was actually done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RepeatFromMode {
    DueDate,
    CompletionDate,
}

/// The custom repeat pattern, kept as loose as it is on the wire.
///
/// Every field is optional because the shape depends on `unit` and the server has never validated
/// the combination. Interpretation lives in [`crate::repeating`] and nowhere else.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomRepeatingPattern {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// `days` | `weeks` | `months` | `years`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<i64>,
    /// `never` | `after_occurrences` | `until_date`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_after_occurrences: Option<i64>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub end_until_date: Option<DateTime<Utc>>,
    /// Weekly patterns: `["monday", "wednesday"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekdays: Option<Vec<String>>,
    /// Monthly patterns: `same_date` | `same_weekday`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month_repeat_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month_day: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month_weekday: Option<MonthWeekday>,
    /// Yearly patterns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthWeekday {
    pub weekday: String,
    pub week_of_month: i64,
}

/// A legacy file attachment. Superseded by [`SecureFile`]; still returned on older tasks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub size: i64,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// A file held by the server's secure-file store. The wire names differ from the field names, and
/// those aliases are the whole reason this type is hand-mapped.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecureFile {
    pub id: String,
    #[serde(rename = "originalName", default)]
    pub name: String,
    #[serde(rename = "fileSize", default)]
    pub size: i64,
    #[serde(rename = "mimeType", default)]
    pub mime_type: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CommentType {
    #[default]
    Text,
    Markdown,
    Attachment,
}

/// `task.remote`: where a task on a GitHub board lives on GitHub (web `lib/backends/task-remote.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRemote {
    /// `issue`, `pull_request` or `draft_issue`.
    #[serde(default)]
    pub kind: String,
    /// The issue or pull request on GitHub. `None` for a draft, which has no page of its own.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub version: Option<String>,
    /// `owner/repo`. `None` for a draft.
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub number: Option<i64>,
}

impl TaskRemote {
    /// `owner/repo#N`, the label a client draws for this remote; `None` when there is no page to
    /// open, because a chip that goes nowhere is worse than no chip.
    pub fn chip_label(&self) -> Option<String> {
        match (&self.url, &self.repo, self.number) {
            (Some(_), Some(repo), Some(number)) => Some(format!("{repo}#{number}")),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// Mutable because an offline comment is created with a `temp_` id and keeps its identity when
    /// the server's id arrives.
    pub id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub r#type: CommentType,
    /// Absent on system comments, which the server authors itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<User>,
    #[serde(default)]
    pub task_id: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_comment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replies: Option<Vec<Comment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure_files: Option<Vec<SecureFile>>,
    /// The server echoes the `temp_<uuid>` idempotency key back. Offline dedup depends on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// `None` is **the response did not say** — `GET /api/v1/tasks?omit=description` leaves the
    /// key out and names it in `meta.omitted` — and `Some("")` is a description a person cleared.
    /// They were one `""` until task ebe0966b (D55), which meant any write built from a cached row
    /// erased text somebody had typed. [`Task::keeping_unsent`] fills an omitted one from the
    /// cache; [`Task::description_text`] is what a reader that only draws or searches it wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The human-readable id — `AST-142` (web task 12f54df4). Minted by the server for tasks on
    /// a project; absent for a task on a project-less list, so a solo user never sees one.
    /// Searchable as a direct hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<User>,
    /// Everyone assigned, as the server sent it (web AWTD-1190). Only a list whose backend
    /// supports several assignees ever holds more than one. Never read this directly:
    /// [`Task::all_assignee_ids`] is the list a client shows, because `assignee_id` is written on
    /// its own — by an older client, or by an optimistic reassignment here — and stays the truth
    /// about who is first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_ids: Option<Vec<String>>,
    /// The MCP API returns the creator object without the id field, so both are optional and
    /// [`Task::effective_creator_id`] is the only correct way to ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<User>,
    /// The single source of truth for when the task is due. See [`super::date`] for what this
    /// means when [`Task::is_all_day`] is set.
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_date_time: Option<DateTime<Utc>>,
    /// Defaults to **true**, matching the Swift memberwise init: a task created with a date and no
    /// stated time is an all-day task.
    #[serde(default = "default_true")]
    pub is_all_day: bool,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub reminder_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_sent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_type: Option<ReminderType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeating: Option<Repeating>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeating_data: Option<CustomRepeatingPattern>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_from: Option<RepeatFromMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence_count: Option<i64>,
    /// The occurrence this one followed, on a list whose series spawns a new task each time
    /// (web AWTD-1118). The link back; the way forward is the task that names this one here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_occurrence_id: Option<String>,
    /// The occurrence that followed this one (web AWTD-1219). Only the single-task read and the
    /// row beside a live event carry the key — an id, or `null` for "none" — and a list page
    /// leaves it out. So absent (`None`) is "this response does not say", which
    /// [`Task::keeping_unsent`] fills from the cache, and `Some(None)` is the server saying there
    /// is no successor. Encodes back as it arrived, because the cache keeps the whole JSON.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub next_occurrence_id: Option<Option<String>>,
    /// Where the server's write-back to the list's backend stands (web spec §11.2): `pending`
    /// while a task made on a GitHub board is still being written there, `error` if that gave
    /// up. Absent and `null` are both "settled" — it is a column on the task, so every response
    /// carries it, and `null` is what a settled task and every classic list send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_state: Option<String>,
    /// The GitHub issue or pull request a task on a GitHub board is (web AWTD-1222, spec §11.2).
    /// Only a GitHub-backed task carries it; absent means classic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<TaskRemote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timer_duration: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_timer_value: Option<String>,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lists: Option<Vec<super::list::TaskList>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_ids: Option<Vec<String>>,
    #[serde(default)]
    pub is_private: bool,
    #[serde(default)]
    pub completed: bool,
    /// The real completion time, which sync may backdate.
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<DateTime<Utc>>,
    /// `astrid` | `google` | `github` | `apple`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_source: Option<String>,
    /// Board status as a state ON the task: `ready` | `doing` | `waiting` | a project's custom
    /// role. `None` means Inbox; Done is derived from `completed`, so neither is ever stored.
    ///
    /// Tolerated as absent on purpose (task AWTD-562): a deployment older than the field simply
    /// does not send it, and the board falls back to list membership. That fallback is what keeps
    /// this build working against an unmigrated server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_role: Option<String>,
    /// Why the task was closed, when it was closed as anything but done: `canceled` |
    /// `duplicate` | `not_planned` (web task 11042ae3). The web keeps `completed` true beside it,
    /// so every view that reads the flag keeps working; only the rendering and the repeat rollover
    /// differ. Kept as text rather than an enum so a reason this build has never heard of cannot
    /// make the whole task unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_reason: Option<String>,
    /// The tasks this one is waiting on, by id, and the tasks waiting on it.
    ///
    /// `GET /api/v1/tasks/:id` adds both beside `listIds`, and the wire contract
    /// (`astrid-web/lib/api-contracts/v1-ios-shapes.ts`, `V1TaskBlockerIds`) makes them
    /// **optional** so the server and each client ship independently: a deployment older than the
    /// feature simply does not send them, which reads here as "nothing known", never as "nothing".
    ///
    /// Ids only. The titles a chip draws come from `GET /api/v1/tasks/:id/blockers`, because a
    /// blocker the reader cannot see has an id and nothing else — and the difference between that
    /// and a blocker that merely has not synced yet is the one thing the row must not get wrong.
    /// Needs no cache migration: the store keeps each model's whole JSON and lifts out only the
    /// columns something queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure_files: Option<Vec<SecureFile>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<Comment>>,
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
    /// Copy lineage - the task this one was copied from. NOT the subtask relation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_list_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    /// Subtasks: the parent's id, `None` for a top-level task. A self-relation on the server,
    /// nulled when the parent is deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
}

fn default_true() -> bool {
    true
}

/// The reasons a task can be closed without being done, exactly as the server accepts them
/// (web's `CLOSED_REASONS`). Anything else is refused rather than written: a typo'd reason must
/// not quietly become "completed normally".
pub const CLOSED_REASONS: [&str; 3] = ["canceled", "duplicate", "not_planned"];

/// Whether `value` is a reason the server will accept.
pub fn is_closed_reason(value: &str) -> bool {
    CLOSED_REASONS.contains(&value)
}

/// A key that is present, even as `null`, decodes as `Some`; serde's own `Option<Option<T>>`
/// folds a `null` into the outer `None`, which is the distinction the field exists to keep.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

impl Task {
    /// This task as the server sent it, with what the response never carries kept from the
    /// cached copy: the blocker ids and the next occurrence ride only on the single-task read, so
    /// a PUT answer or a list page that leaves them out is not saying the task stopped waiting
    /// or lost its successor — and a page fetched with `?omit=description` is not saying the
    /// description was cleared (task ebe0966b).
    pub fn keeping_unsent(mut self, cached: &Task) -> Task {
        if self.blocked_by.is_none() {
            self.blocked_by = cached.blocked_by.clone();
        }
        if self.blocks.is_none() {
            self.blocks = cached.blocks.clone();
        }
        if self.next_occurrence_id.is_none() {
            self.next_occurrence_id = cached.next_occurrence_id.clone();
        }
        if self.description.is_none() {
            self.description = cached.description.clone();
        }
        self
    }

    /// The description as a reader that only draws or searches it wants: `""` both for a task
    /// nobody has described and for a response that did not say. A reader deciding what to WRITE
    /// must take [`Task::description`] itself, because those two are the case this distinguishes.
    pub fn description_text(&self) -> &str {
        self.description.as_deref().unwrap_or_default()
    }

    /// A task with nothing but an id and a title, for the paths that build one from scratch.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Task {
            id: id.into(),
            title: title.into(),
            description: None,
            identifier: None,
            assignee_id: None,
            assignee: None,
            assignee_ids: None,
            creator_id: None,
            creator: None,
            blocked_by: None,
            blocks: None,
            due_date_time: None,
            is_all_day: true,
            reminder_time: None,
            reminder_sent: None,
            reminder_type: None,
            repeating: None,
            repeating_data: None,
            repeat_from: None,
            occurrence_count: None,
            previous_occurrence_id: None,
            next_occurrence_id: None,
            sync_state: None,
            remote: None,
            timer_duration: None,
            last_timer_value: None,
            priority: Priority::None,
            lists: None,
            list_ids: None,
            is_private: false,
            completed: false,
            completed_at: None,
            completed_source: None,
            status_role: None,
            closed_reason: None,
            attachments: None,
            secure_files: None,
            comments: None,
            created_at: None,
            updated_at: None,
            original_task_id: None,
            source_list_id: None,
            client_request_id: None,
            parent_task_id: None,
        }
    }

    /// Everyone assigned, primary first — the web's `assigneeIdsOf` (`lib/task-assignees.ts`).
    ///
    /// `assignee_id` is the primary whatever the stored array says, and the array supplies the
    /// others. No primary means nobody: the server promotes the next person when the first is
    /// cleared, so an array without a primary is a stale one, not a second opinion.
    pub fn all_assignee_ids(&self) -> Vec<&str> {
        let Some(primary) = self.assignee_id.as_deref().filter(|id| !id.is_empty()) else {
            return Vec::new();
        };
        let others = self
            .assignee_ids
            .iter()
            .flatten()
            .map(String::as_str)
            .filter(|id| *id != primary);
        std::iter::once(primary).chain(others).collect()
    }

    /// The creator's id, from whichever of the two places the response put it.
    pub fn effective_creator_id(&self) -> Option<&str> {
        self.creator_id
            .as_deref()
            .or(self.creator.as_ref().map(|u| u.id.as_str()))
    }

    pub fn is_created_by(&self, user_id: &str) -> bool {
        self.effective_creator_id() == Some(user_id)
    }

    /// The ids of the lists this task is in, from whichever of the two shapes the response used.
    pub fn effective_list_ids(&self) -> Vec<String> {
        if let Some(ids) = &self.list_ids {
            return ids.clone();
        }
        self.lists
            .as_ref()
            .map(|lists| lists.iter().map(|l| l.id.clone()).collect())
            .unwrap_or_default()
    }

    /// True when the task rolls over on completion.
    pub fn is_repeating(&self) -> bool {
        self.repeating.map(Repeating::repeats).unwrap_or(false)
    }

    /// Closed as anything other than done — web's `isCanceled` (`lib/closed-reason.ts`). A reason
    /// on an open task means nothing, exactly as it means nothing there.
    pub fn is_canceled(&self) -> bool {
        self.completed && self.closed_reason.as_deref().is_some_and(is_closed_reason)
    }

    /// Every distinct secure file reachable from this task: its own, its legacy attachments
    /// converted, and the ones on its comments. Order is stable and duplicates are dropped by id.
    pub fn all_secure_files(&self) -> Vec<SecureFile> {
        let mut seen = std::collections::HashSet::new();
        let mut files = Vec::new();
        for file in self.secure_files.iter().flatten() {
            if seen.insert(file.id.clone()) {
                files.push(file.clone());
            }
        }
        for attachment in self.attachments.iter().flatten() {
            if seen.insert(attachment.id.clone()) {
                files.push(SecureFile {
                    id: attachment.id.clone(),
                    name: attachment.name.clone(),
                    size: attachment.size,
                    mime_type: attachment.r#type.clone(),
                });
            }
        }
        for comment in self.comments.iter().flatten() {
            for file in comment.secure_files.iter().flatten() {
                if seen.insert(file.id.clone()) {
                    files.push(file.clone());
                }
            }
        }
        files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AITD-466: the cache keeps a task as this struct, so a field it does not name is gone
    /// before any shell reads it. `assigneeIds` was dropped that way.
    #[test]
    fn aitd_466_assignee_ids_survive_the_round_trip_through_the_cache_shape() {
        let decoded: Task = serde_json::from_str(
            r#"{"id":"t1","title":"x","assigneeId":"u1","assigneeIds":["u1","u2"]}"#,
        )
        .expect("decodes");
        let stored = serde_json::to_value(&decoded).expect("encodes");
        assert_eq!(stored["assigneeIds"], serde_json::json!(["u1", "u2"]));
    }

    #[test]
    fn aitd_466_the_primary_assignee_leads_whatever_the_array_says() {
        let mut task = Task::new("t1", "x");
        assert!(task.all_assignee_ids().is_empty());

        // A server older than the field: one assignee, no array.
        task.assignee_id = Some("u1".into());
        assert_eq!(task.all_assignee_ids(), ["u1"]);

        // Reassigned here, array still as fetched: the new primary leads and is not repeated.
        task.assignee_ids = Some(vec!["u1".into(), "u2".into(), "u3".into()]);
        task.assignee_id = Some("u3".into());
        assert_eq!(task.all_assignee_ids(), ["u3", "u1", "u2"]);

        // Unassigned here: nobody, not the array's leftovers.
        task.assignee_id = None;
        assert!(task.all_assignee_ids().is_empty());
    }

    /// Two production tasks were observed with `priority: 4`. Swift's default decode threw, which
    /// failed the whole array and broke sync for that account until it was found.
    #[test]
    fn a_priority_the_schema_never_capped_decodes_rather_than_failing() {
        let decoded: Task =
            serde_json::from_str(r#"{"id":"t1","title":"x","priority":4}"#).expect("decodes");
        assert_eq!(decoded.priority, Priority::None);
    }

    #[test]
    fn priority_round_trips_through_its_number() {
        for priority in [
            Priority::None,
            Priority::Low,
            Priority::Medium,
            Priority::High,
        ] {
            assert_eq!(Priority::from_i64(priority.as_i64()), priority);
        }
        assert_eq!(
            serde_json::to_string(&Priority::High).expect("encodes"),
            "3"
        );
    }

    /// A task arrives from a dozen endpoints, several of which return only a handful of fields.
    /// Anything mandatory beyond the id is a decode failure waiting for the next thin response.
    #[test]
    fn the_id_is_the_only_field_a_task_needs() {
        let decoded: Task = serde_json::from_str(r#"{"id":"t1"}"#).expect("decodes");
        assert_eq!(decoded.id, "t1");
        assert_eq!(decoded.title, "");
        assert!(!decoded.completed);
    }

    /// Matches the Swift memberwise default. A task given a date and no time is an all-day task,
    /// and defaulting the other way puts a spurious midnight on every one of them.
    #[test]
    fn a_task_is_all_day_unless_the_server_says_otherwise() {
        let decoded: Task = serde_json::from_str(r#"{"id":"t1"}"#).expect("decodes");
        assert!(decoded.is_all_day);
        let timed: Task = serde_json::from_str(r#"{"id":"t1","isAllDay":false}"#).expect("decodes");
        assert!(!timed.is_all_day);
    }

    #[test]
    fn the_creator_id_comes_from_whichever_shape_the_response_used() {
        let embedded: Task =
            serde_json::from_str(r#"{"id":"t1","creator":{"id":"u9"}}"#).expect("decodes");
        assert_eq!(embedded.effective_creator_id(), Some("u9"));
        assert!(embedded.is_created_by("u9"));

        let flat: Task = serde_json::from_str(r#"{"id":"t1","creatorId":"u9"}"#).expect("decodes");
        assert_eq!(flat.effective_creator_id(), Some("u9"));
    }

    #[test]
    fn list_ids_fall_back_to_the_embedded_lists() {
        let embedded: Task = serde_json::from_str(
            r#"{"id":"t1","lists":[{"id":"l1","name":"Home"},{"id":"l2","name":"Work"}]}"#,
        )
        .expect("decodes");
        assert_eq!(embedded.effective_list_ids(), vec!["l1", "l2"]);
    }

    /// The wire names are `originalName`, `fileSize` and `mimeType`. Getting one wrong shows an
    /// unnamed zero-byte file in the attachment row rather than failing loudly.
    #[test]
    fn secure_files_keep_the_wire_names_they_arrive_under() {
        let decoded: SecureFile = serde_json::from_str(
            r#"{"id":"f1","originalName":"plan.pdf","fileSize":2048,"mimeType":"application/pdf"}"#,
        )
        .expect("decodes");
        assert_eq!(decoded.name, "plan.pdf");
        assert_eq!(decoded.size, 2048);
        assert_eq!(decoded.mime_type, "application/pdf");
    }

    #[test]
    fn every_file_reachable_from_a_task_is_listed_once() {
        let file = |id: &str| SecureFile {
            id: id.into(),
            name: format!("{id}.pdf"),
            size: 1,
            mime_type: "application/pdf".into(),
        };
        let mut task = Task::new("t1", "x");
        task.secure_files = Some(vec![file("a")]);
        task.attachments = Some(vec![Attachment {
            id: "b".into(),
            name: "b.pdf".into(),
            url: String::new(),
            r#type: "application/pdf".into(),
            size: 1,
            created_at: None,
            task_id: None,
        }]);
        let mut comment =
            serde_json::from_str::<Comment>(r#"{"id":"c1","taskId":"t1"}"#).expect("decodes");
        // The same file on the task and on a comment is one file, not two.
        comment.secure_files = Some(vec![file("a"), file("c")]);
        task.comments = Some(vec![comment]);

        let ids: Vec<String> = task.all_secure_files().into_iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    /// Optional fields are omitted rather than sent as null: a PATCH body that spelled out every
    /// absent field would clear the ones the caller never touched.
    #[test]
    fn encoding_omits_what_was_never_set() {
        let encoded = serde_json::to_string(&Task::new("t1", "Buy milk")).expect("encodes");
        assert!(!encoded.contains("null"), "unexpected null in {encoded}");
        assert!(encoded.contains(r#""isAllDay":true"#));
    }

    /// The successor rides only on the single-task read, which always carries the key — an id, or
    /// `null` for "none". A list page leaves the key out, and that must not read as "none".
    #[test]
    fn next_occurrence_id_tells_absent_from_none() {
        let decode = |json: &str| serde_json::from_str::<Task>(json).expect("decodes");
        assert_eq!(decode(r#"{"id":"t1"}"#).next_occurrence_id, None);
        assert_eq!(
            decode(r#"{"id":"t1","nextOccurrenceId":null}"#).next_occurrence_id,
            Some(None)
        );
        let known = decode(r#"{"id":"t1","nextOccurrenceId":"t2"}"#);
        assert_eq!(known.next_occurrence_id, Some(Some("t2".to_string())));

        // The cache keeps the task's whole JSON, so all three have to survive a round trip.
        let again = |task: &Task| decode(&serde_json::to_string(task).expect("encodes"));
        assert_eq!(
            again(&known).next_occurrence_id,
            Some(Some("t2".to_string()))
        );
        let none = decode(r#"{"id":"t1","nextOccurrenceId":null}"#);
        assert_eq!(again(&none).next_occurrence_id, Some(None));
        assert_eq!(again(&Task::new("t1", "x")).next_occurrence_id, None);
    }

    #[test]
    fn a_response_without_the_successor_keeps_the_one_the_cache_knows() {
        let mut cached = Task::new("t1", "Water the plants");
        cached.next_occurrence_id = Some(Some("t2".to_string()));

        // A list page: the key is absent, so the cached answer stands.
        let page = Task::new("t1", "Water the plants").keeping_unsent(&cached);
        assert_eq!(page.next_occurrence_id, Some(Some("t2".to_string())));

        // The single-task read saying there is none: the server's word, so it clears.
        let mut detail = Task::new("t1", "Water the plants");
        detail.next_occurrence_id = Some(None);
        assert_eq!(
            detail.keeping_unsent(&cached).next_occurrence_id,
            Some(None)
        );
    }

    /// Task ebe0966b (D55): a description the server OMITTED and one a person CLEARED are
    /// different facts, and `#[serde(default)] String` made them the same `""`.
    ///
    /// `GET /api/v1/tasks?omit=description` answers with the key absent and says so in
    /// `meta.omitted`. Decoded as `""`, that empty string is a value the server will store — so the
    /// next write built from the cached row erases text a person typed.
    #[test]
    fn ebe0966b_an_omitted_description_is_not_a_cleared_one() {
        let decode = |json: &str| serde_json::from_str::<Task>(json).expect("decodes");

        // Omitted: the response does not say, and re-encoding must not start saying.
        let omitted = decode(r#"{"id":"t1","title":"Buy milk"}"#);
        assert_eq!(omitted.description, None);
        assert_eq!(omitted.description_text(), "");
        let stored = serde_json::to_value(&omitted).expect("encodes");
        assert!(
            stored.get("description").is_none(),
            "an omitted description must not reappear as a key: {stored}"
        );

        // Cleared: the server said "empty", which is a value and survives the cache verbatim.
        let cleared = decode(r#"{"id":"t1","title":"Buy milk","description":""}"#);
        assert_eq!(cleared.description.as_deref(), Some(""));
        assert_eq!(
            serde_json::to_value(&cleared).expect("encodes")["description"],
            serde_json::json!("")
        );

        // Written: unchanged by any of this.
        let written = decode(r#"{"id":"t1","title":"Buy milk","description":"two litres"}"#);
        assert_eq!(written.description_text(), "two litres");
    }

    /// The half that stops the erasure: a payload that left the description out is merged against
    /// the cached copy, the same way the blocker ids and the successor already are.
    #[test]
    fn ebe0966b_a_response_without_a_description_keeps_the_one_the_cache_knows() {
        let mut cached = Task::new("t1", "Buy milk");
        cached.description = Some("The long note the person typed".to_string());

        let page = Task::new("t1", "Buy milk").keeping_unsent(&cached);
        assert_eq!(
            page.description.as_deref(),
            Some("The long note the person typed"),
            "a page that omitted the description does not blank it"
        );

        // The server saying it is empty is the server's word, not an omission, so it stands.
        let mut cleared = Task::new("t1", "Buy milk");
        cleared.description = Some(String::new());
        assert_eq!(
            cleared.keeping_unsent(&cached).description.as_deref(),
            Some("")
        );
    }

    /// A task made on a GitHub board whose write-back has not finished carries `pending` (AITD-491).
    /// The cache keeps the task's whole JSON, so a key the model does not name is lost there.
    #[test]
    fn aitd_491_sync_state_survives_the_cache_and_null_is_settled() {
        let decode = |json: &str| serde_json::from_str::<Task>(json).expect("decodes");
        let pending = decode(r#"{"id":"t1","syncState":"pending"}"#);
        assert_eq!(pending.sync_state.as_deref(), Some("pending"));
        let again = decode(&serde_json::to_string(&pending).expect("encodes"));
        assert_eq!(again.sync_state.as_deref(), Some("pending"));

        // What the server sends for every settled task, and what a classic list sends.
        assert_eq!(decode(r#"{"id":"t1","syncState":null}"#).sync_state, None);
        assert_eq!(decode(r#"{"id":"t1"}"#).sync_state, None);
    }

    /// A GitHub-backed task carries `remote` (web AWTD-1222), the issue a `repo#N` chip opens. The
    /// cache dropped it, so no app could draw the chip (AITD-491).
    #[test]
    fn aitd_491_remote_survives_the_cache_and_names_its_chip() {
        let decode = |json: &str| serde_json::from_str::<Task>(json).expect("decodes");
        let issue = decode(
            r#"{"id":"t1","remote":{"kind":"issue","url":"https://github.com/o/r/issues/7","nodeId":"I_1","version":null,"repo":"o/r","number":7}}"#,
        );
        let remote = issue.remote.as_ref().expect("kept");
        assert_eq!(
            remote.url.as_deref(),
            Some("https://github.com/o/r/issues/7")
        );
        assert_eq!(remote.chip_label().as_deref(), Some("o/r#7"));
        let again = decode(&serde_json::to_string(&issue).expect("encodes"));
        assert_eq!(again.remote, issue.remote);

        // A draft has no page of its own and no chip; a classic task has no remote at all.
        let draft = decode(
            r#"{"id":"t2","remote":{"kind":"draft_issue","url":null,"nodeId":"D_1","version":null,"repo":null,"number":null}}"#,
        );
        assert_eq!(draft.remote.expect("kept").chip_label(), None);
        assert_eq!(decode(r#"{"id":"t3"}"#).remote, None);
    }

    #[test]
    fn repeat_from_uses_the_wire_spelling() {
        let decoded: Task =
            serde_json::from_str(r#"{"id":"t1","repeatFrom":"COMPLETION_DATE"}"#).expect("decodes");
        assert_eq!(decoded.repeat_from, Some(RepeatFromMode::CompletionDate));
        assert_eq!(
            serde_json::to_string(&RepeatFromMode::DueDate).expect("encodes"),
            r#""DUE_DATE""#
        );
    }
}
