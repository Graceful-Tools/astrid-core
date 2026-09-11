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
    #[serde(default)]
    pub description: String,
    /// The human-readable id — `AST-142` (web task 12f54df4). Minted by the server for tasks on
    /// a project; absent for a task on a project-less list, so a solo user never sees one.
    /// Searchable as a direct hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<User>,
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

impl Task {
    /// A task with nothing but an id and a title, for the paths that build one from scratch.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Task {
            id: id.into(),
            title: title.into(),
            description: String::new(),
            identifier: None,
            assignee_id: None,
            assignee: None,
            creator_id: None,
            creator: None,
            due_date_time: None,
            is_all_day: true,
            reminder_time: None,
            reminder_sent: None,
            reminder_type: None,
            repeating: None,
            repeating_data: None,
            repeat_from: None,
            occurrence_count: None,
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
