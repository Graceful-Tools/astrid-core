//! Lists, their membership, and the per-list settings the shell renders.
//!
//! Ported from `astrid-ios/Astrid App/Models/TaskList.swift`.
//!
//! Two pieces of hard-won leniency are reproduced here rather than tidied away, because both were
//! paid for in production:
//!
//! - `aiAgentsEnabled` is documented as `string[]`, and on 2026-08-29 the server leaked its stored
//!   object form `{ enabledTypes, defaultAgentId }` on ONE list. Swift decodes an array as a unit,
//!   so that one field failed the decode of every list in the account — the app showed "offline
//!   mode, 0 lists". Both shapes are accepted here, and [`crate::model::lenient`] additionally
//!   keeps one unreadable list from taking the rest with it.
//! - `showSubtasks: null` means **show**. A list fetched before the field existed has to render
//!   exactly as it did (task ba1deb9d), so the absent case can never be read as `false`.
//!
//! Roles are NOT decided here. [`crate::permissions`] owns that, and it is the only place allowed
//! to compare a role string.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use super::date;
use super::task::Task;
use super::user::User;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Privacy {
    Private,
    Shared,
    Public,
}

/// `{ enabledTypes, defaultAgentId }` — the shape the server stores and, since 2026-08-29, emits
/// as `aiAgentConfig` beside the plain `aiAgentsEnabled` array. It carries the default agent the
/// array cannot express.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentConfig {
    #[serde(default)]
    pub enabled_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent_id: Option<String>,
}

/// Someone's membership of a list. Presence IS membership; the role only refines what it allows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMember {
    /// Absent in the embedded member arrays some responses return (creating a task, for one), and
    /// the user id stands in — see [`ListMember::identity`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    pub user_id: String,
    #[serde(default)]
    pub role: String,
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
    pub user: Option<User>,
}

impl ListMember {
    /// A stable key for this row. The membership id when the response carried one, the user id
    /// otherwise — a member list keyed on an empty string collapses into one row.
    pub fn identity(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.user_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListInvite {
    pub id: String,
    #[serde(default)]
    pub list_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub token: String,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

/// How far back "Recently completed" reaches for one list.
///
/// Mirrors `astrid-web/lib/recently-completed-window.ts`, discriminated by `kind`. The same
/// setting drives the list view's default completion filter and the board's Done column, so the
/// two views can never disagree. `None` on the list means the legacy 24-hour default.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RecentlyCompletedWindow {
    Duration { amount: i64, unit: DurationUnit },
    SinceWeekday { weekday: i64 },
    SinceDayOfMonth { day: i64 },
    SinceDate { date: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DurationUnit {
    Hour,
    Day,
    Week,
    Month,
}

/// `list.backend` for a GitHub Projects board (astrid-web `lib/backends/supports.ts`).
pub const GITHUB_PROJECT_BACKEND: &str = "github_project";

/// `list.supports` on the wire: what a list's backend can do that a client would otherwise have
/// to guess. Every key is at its classic value when absent, and the web adds a key as each
/// capability lands (`lib/backends/supports.ts`) — so a key this build does not know is ignored
/// rather than refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSupports {
    /// A task here may hold several assignees (`assigneeIds`). Classic lists keep at most one.
    #[serde(default)]
    pub multiple_assignees: bool,
    /// How a repeating task here advances when it is completed (web AWTD-1118, spec §10).
    #[serde(default)]
    pub repeat_mode: RepeatMode,
    /// `isPrivate` means something here. A GitHub board's tasks are as visible as its project.
    #[serde(default = "classic")]
    pub private_tasks: bool,
    /// Members and invitations are managed in Astrid. On a GitHub board the roster is GitHub's.
    #[serde(default = "classic")]
    pub membership: bool,
    /// Priority can be set. False on a GitHub board with no Priority field mapped.
    #[serde(default = "classic")]
    pub priority: bool,
    /// A due date can be set. False on a GitHub board with no date field mapped.
    #[serde(default = "classic")]
    pub due_date: bool,
    #[serde(default = "classic")]
    pub comments: bool,
    /// Tasks here can carry the project's own field values (web AWTD-1208).
    #[serde(default)]
    pub fields: bool,
    /// What deleting a task does here.
    #[serde(default)]
    pub delete_task: DeleteTaskMode,
}

/// A key the server leaves out of `supports` means what it means on a classic list, so a server
/// older than the key never hides a control that works (web AWTD-1222).
fn classic() -> bool {
    true
}

impl Default for ListSupports {
    fn default() -> Self {
        Self {
            multiple_assignees: false,
            repeat_mode: RepeatMode::RollForward,
            private_tasks: true,
            membership: true,
            priority: true,
            due_date: true,
            comments: true,
            fields: false,
            delete_task: DeleteTaskMode::Delete,
        }
    }
}

/// `list.supports.deleteTask`. A GitHub board takes the item off the project and leaves the issue.
/// A value this build does not know reads as the classic one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteTaskMode {
    RemoveFromList,
    #[default]
    #[serde(other)]
    Delete,
}

/// `list.supports.repeatMode`. On a GitHub board the completed issue stays closed as history and
/// the next occurrence is a NEW task the server makes, so a client sends the completion and does
/// not roll the row forward itself. A value this build does not know reads as the classic one:
/// rolling forward is recoverable, a series silently dropped is not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    Spawn,
    #[default]
    #[serde(other)]
    RollForward,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
    /// Absent from the thinner responses, so never assume a missing privacy means private.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy: Option<Privacy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_list_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<User>,
    /// Legacy denormalised arrays. Kept so the public-lists endpoint still decodes; **never**
    /// consulted for a role — see [`crate::permissions`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admins: Option<Vec<User>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<User>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_members: Option<Vec<ListMember>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitations: Option<Vec<ListInvite>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_assignee_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_assignee: Option<User>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_repeating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_is_private: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_due_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_access_level: Option<String>,
    #[serde(
        default,
        rename = "aiAstridEnabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_astrid_enabled: Option<bool>,
    #[serde(
        default,
        rename = "preferredAiProvider",
        skip_serializing_if = "Option::is_none"
    )]
    pub preferred_ai_provider: Option<String>,
    #[serde(
        default,
        rename = "fallbackAiProvider",
        skip_serializing_if = "Option::is_none"
    )]
    pub fallback_ai_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_repository_id: Option<String>,
    /// Accepts both the documented `string[]` and the object form the server once leaked. See the
    /// module note.
    #[serde(
        default,
        rename = "aiAgentsEnabled",
        deserialize_with = "deserialize_agents_enabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_agents_enabled: Option<Vec<String>>,
    #[serde(
        default,
        rename = "aiAgentConfig",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_agent_config: Option<ListAgentConfig>,
    #[serde(
        default,
        rename = "aiAgentConfiguredBy",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_agent_configured_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_count: Option<i64>,
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
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<Task>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_favorite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorite_order: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_virtual: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_list_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_sort_order: Option<Vec<String>>,
    /// **`None` means SHOW.** See the module note and [`TaskList::shows_subtasks`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_subtasks: Option<bool>,
    /// What this list's backend can do (web AWTD-1190, spec §11.2). **`None` means classic**: a
    /// server older than the field omits it. Ask through the methods, not the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports: Option<ListSupports>,
    /// Which backend holds this list (web AWTD-1256, spec §11.2): `"github_project"` for a GitHub
    /// Projects board, `null` for a classic one. Ask through [`TaskList::is_github_backed`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,

    // Filter settings, used by virtual lists and as the saved default of a real one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_completion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_assigned_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_repeating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_in_lists: Option<String>,

    /// Project status board. A project holds both `listType: "regular"` (domain) lists and
    /// `listType: "status"` (board column) lists. Inbox and Done are virtual columns derived from
    /// task state and are never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_order: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_completed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_recently_completed_window"
    )]
    pub recently_completed_window: Option<RecentlyCompletedWindow>,
}

/// `aiAgentsEnabled` arrives as `string[]`, as the stored `{ enabledTypes, defaultAgentId }`, as
/// null, or not at all. Anything else reads as "nothing configured".
///
/// It goes through [`serde_json::Value`] rather than an untagged enum because an untagged decode
/// that fails leaves serde_json's deserializer mid-token — the recovery would fail the whole list,
/// which is the outage this function exists to prevent.
fn deserialize_agents_enabled<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match raw {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone())
            .ok()
            .or_else(|| {
                serde_json::from_value::<ListAgentConfig>(value)
                    .ok()
                    .map(|config| config.enabled_types)
            }),
    })
}

/// `recentlyCompletedWindow` is an unvalidated JSON column that `lib/lists/v1-list-shape.ts` passes
/// straight through, so the wire can carry a value no version of web's own type describes. Anything
/// this crate cannot read means "the legacy 24-hour default", exactly as an absent one does.
///
/// Task f8dfc84e found the cost of not doing this: the `list.event` surface of
/// `contracts/fixtures/wire-shapes.json` sends `{ "amount": 3, "unit": "days" }` — no `kind`, and
/// a plural unit — and a tagged decode answered `missing field \`kind\`` for the whole `TaskList`.
/// The API client decodes collections leniently, so in production that is not one unreadable
/// setting: it is a list that disappears from the sidebar, which is the 2026-08-29 outage again.
/// Through [`serde_json::Value`] for the same reason as [`deserialize_agents_enabled`].
fn deserialize_recently_completed_window<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RecentlyCompletedWindow>, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match raw {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => serde_json::from_value::<RecentlyCompletedWindow>(value).ok(),
    })
}

impl TaskList {
    /// A list with nothing but an id and a name, for the paths that build one from scratch.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        serde_json::from_value(serde_json::json!({
            "id": id.into(),
            "name": name.into(),
        }))
        .expect("a list of an id and a name is always decodable")
    }

    pub fn display_color(&self) -> &str {
        self.color
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or("#3b82f6")
    }

    /// True for status/state rows (Ready/Doing/Waiting/custom project states), which are never
    /// rendered as ordinary lists in sidebars or pickers.
    pub fn is_status_list(&self) -> bool {
        self.list_type.as_deref() == Some("status")
    }

    /// True for a label: a list that renders as a chip and never as a destination.
    ///
    /// The web's third flavour (`lib/list-flavors.ts`, task 60f5849d): multi-list membership
    /// already *is* labels — a task in "iOS To-do" and "Bugs" is a task with a label — so a label
    /// is a `TaskList` with `listType: "label"` rather than a new entity. What it is not: a place.
    /// It has no defaults, no chat, no agent, and the sidebar and the pickers must not offer it as
    /// somewhere to go.
    pub fn is_label_list(&self) -> bool {
        self.list_type.as_deref() == Some("label")
    }

    /// True for a GitHub milestone (web AWTD-1189): a tag, like a label, of which a task has at
    /// most one. Read-only here — GitHub owns the membership.
    pub fn is_milestone_list(&self) -> bool {
        self.list_type.as_deref() == Some("milestone")
    }

    /// True for a GitHub project iteration, a sprint (web AWTD-1230, AITD-506): a tag, like a
    /// milestone, with dates. Read-only here; changing a task's iteration is a project-field edit.
    pub fn is_iteration_list(&self) -> bool {
        self.list_type.as_deref() == Some("iteration")
    }

    /// A tag rather than a place: a label, a milestone or an iteration — the web's `isTagList`.
    ///
    /// Ask this, not the flavours one by one, wherever the question is "chip or destination", so a
    /// sixth flavour is added here and nowhere else.
    pub fn is_tag_list(&self) -> bool {
        self.is_label_list() || self.is_milestone_list() || self.is_iteration_list()
    }

    /// True for list-shaped destinations a person can navigate into and file tasks in.
    ///
    /// A missing `listType` is regular: every row predating flavours is a domain list, and
    /// defaulting the other way would hide them all.
    pub fn is_domain_list(&self) -> bool {
        !self.is_status_list() && !self.is_tag_list()
    }

    /// May a task on this list hold several assignees? Classic unless the server says otherwise.
    pub fn supports_multiple_assignees(&self) -> bool {
        self.supports
            .is_some_and(|supports| supports.multiple_assignees)
    }

    /// Does completing a repeating task here leave it completed, with the next occurrence arriving
    /// from the server as a new task? Classic — roll forward in place — unless the server says so.
    pub fn spawns_next_occurrence(&self) -> bool {
        self.supports
            .is_some_and(|supports| supports.repeat_mode == RepeatMode::Spawn)
    }

    /// What this list supports, at classic values where the server says nothing.
    fn supported(&self) -> ListSupports {
        self.supports.unwrap_or_default()
    }

    /// Does a task's private flag mean anything here? Classic unless the server says otherwise.
    pub fn supports_private_tasks(&self) -> bool {
        self.supported().private_tasks
    }

    /// Are members and invitations managed in Astrid? Classic unless the server says otherwise.
    pub fn supports_membership(&self) -> bool {
        self.supported().membership
    }

    /// Can a task's priority be set? Classic unless the server says otherwise.
    pub fn supports_priority(&self) -> bool {
        self.supported().priority
    }

    /// Can a task's due date be set? Classic unless the server says otherwise.
    pub fn supports_due_date(&self) -> bool {
        self.supported().due_date
    }

    /// Can tasks here be commented on? Classic unless the server says otherwise.
    pub fn supports_comments(&self) -> bool {
        self.supported().comments
    }

    /// Can tasks here carry the project's own field values? Never on a classic list.
    pub fn supports_fields(&self) -> bool {
        self.supported().fields
    }

    /// What deleting a task here does. Classic unless the server says otherwise.
    pub fn delete_task_mode(&self) -> DeleteTaskMode {
        self.supported().delete_task
    }

    /// Is this list a GitHub Projects board, whose issues the server mirrors itself? Read from
    /// `backend`; a server older than that field leaves only `supports`, whose every key the web
    /// derives from the backend, so a spawning series identifies such a board there.
    pub fn is_github_backed(&self) -> bool {
        match self.backend.as_deref() {
            Some(backend) => backend == GITHUB_PROJECT_BACKEND,
            None => self.spawns_next_occurrence(),
        }
    }

    /// Whether this list splices subtasks inline. **Absent means yes** (task ba1deb9d): a list
    /// fetched before the field existed must render exactly as it did.
    pub fn shows_subtasks(&self) -> bool {
        self.show_subtasks.unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AITD-466: `supports` was dropped by the cache shape, so no shell could gate on it.
    #[test]
    fn aitd_466_supports_survives_the_cache_shape_and_absent_means_classic() {
        let github: TaskList = serde_json::from_str(
            r#"{"id":"l1","name":"Board","supports":{"multipleAssignees":true,"aKeyFromLater":"spawn"}}"#,
        )
        .expect("decodes, unknown keys and all");
        assert!(github.supports_multiple_assignees());
        let stored = serde_json::to_value(&github).expect("encodes");
        assert_eq!(stored["supports"]["multipleAssignees"], true);

        let classic: TaskList =
            serde_json::from_str(r#"{"id":"l2","name":"Home"}"#).expect("decodes");
        assert!(!classic.supports_multiple_assignees());
        assert!(serde_json::to_value(&classic)
            .expect("encodes")
            .get("supports")
            .is_none());
    }

    /// AITD-491: `repeatMode` was the key from later, and it was dropped like any unknown one.
    #[test]
    fn aitd_491_repeat_mode_survives_the_cache_shape_and_anything_else_rolls_forward() {
        let board: TaskList = serde_json::from_str(
            r#"{"id":"l1","name":"Board","supports":{"multipleAssignees":true,"repeatMode":"spawn"}}"#,
        )
        .expect("decodes");
        assert!(board.spawns_next_occurrence());
        assert_eq!(
            serde_json::to_value(&board).expect("encodes")["supports"]["repeatMode"],
            "spawn"
        );

        for classic in [
            r#"{"id":"l2","name":"Home"}"#,
            r#"{"id":"l3","name":"Older","supports":{"multipleAssignees":true}}"#,
            r#"{"id":"l4","name":"Home","supports":{"repeatMode":"roll_forward"}}"#,
            r#"{"id":"l5","name":"Later","supports":{"repeatMode":"aModeFromLater"}}"#,
        ] {
            let list: TaskList = serde_json::from_str(classic).expect("decodes");
            assert!(!list.spawns_next_occurrence(), "{classic}");
        }
    }

    /// AITD-491: `backend` reached the v1 list payload (web AWTD-1256) and the cache dropped it, so
    /// "is this a GitHub board" could only be guessed from `supports`.
    #[test]
    fn aitd_491_backend_survives_the_cache_and_decides_github_backed() {
        let board: TaskList =
            serde_json::from_str(r#"{"id":"l1","name":"Board","backend":"github_project"}"#)
                .expect("decodes");
        assert!(board.is_github_backed(), "backend alone decides it");
        assert_eq!(
            serde_json::to_value(&board).expect("encodes")["backend"],
            "github_project"
        );

        // A server older than the field: the supports derivation is all there is.
        let older: TaskList =
            serde_json::from_str(r#"{"id":"l2","name":"Board","supports":{"repeatMode":"spawn"}}"#)
                .expect("decodes");
        assert!(older.is_github_backed());

        for classic in [
            r#"{"id":"l3","name":"Home"}"#,
            r#"{"id":"l4","name":"Home","backend":null}"#,
            r#"{"id":"l5","name":"Home","backend":null,"supports":{"repeatMode":"roll_forward"}}"#,
            r#"{"id":"l6","name":"Later","backend":"aBackendFromLater"}"#,
        ] {
            let list: TaskList = serde_json::from_str(classic).expect("decodes");
            assert!(!list.is_github_backed(), "{classic}");
        }
    }

    /// AITD-491: the rest of `supports` reached the v1 list payload (web AWTD-1222) and the cache
    /// dropped every key it did not name, so no app could hide a control the board refuses.
    #[test]
    fn aitd_491_every_supports_key_survives_the_cache_and_absent_is_classic() {
        let board: TaskList = serde_json::from_str(
            r#"{"id":"l1","name":"Board","backend":"github_project","supports":{"multipleAssignees":true,"repeatMode":"spawn","privateTasks":false,"membership":false,"priority":false,"dueDate":true,"comments":true,"fields":true,"deleteTask":"remove_from_list"}}"#,
        )
        .expect("decodes");
        assert!(!board.supports_private_tasks());
        assert!(!board.supports_membership());
        assert!(!board.supports_priority());
        assert!(board.supports_due_date());
        assert!(board.supports_comments());
        assert!(board.supports_fields());
        assert_eq!(board.delete_task_mode(), DeleteTaskMode::RemoveFromList);
        let stored = serde_json::to_value(&board).expect("encodes");
        assert_eq!(stored["supports"]["privateTasks"], false);
        assert_eq!(stored["supports"]["deleteTask"], "remove_from_list");

        // No `supports`, an older server's partial one, and an unknown delete mode: all classic.
        for classic in [
            r#"{"id":"l2","name":"Home"}"#,
            r#"{"id":"l3","name":"Older","supports":{"multipleAssignees":true}}"#,
            r#"{"id":"l4","name":"Later","supports":{"deleteTask":"aModeFromLater"}}"#,
        ] {
            let list: TaskList = serde_json::from_str(classic).expect("decodes");
            assert!(list.supports_private_tasks(), "{classic}");
            assert!(list.supports_membership(), "{classic}");
            assert!(list.supports_priority(), "{classic}");
            assert!(list.supports_due_date(), "{classic}");
            assert!(list.supports_comments(), "{classic}");
            assert!(!list.supports_fields(), "{classic}");
            assert_eq!(list.delete_task_mode(), DeleteTaskMode::Delete, "{classic}");
        }
    }

    /// The 2026-08-29 outage in one test: the object form on one list must not cost the account
    /// every list it has.
    #[test]
    fn agents_enabled_decodes_from_the_object_form_the_server_leaked() {
        let decoded: TaskList = serde_json::from_str(
            r#"{"id":"l1","name":"Home","aiAgentsEnabled":{"enabledTypes":["claude"],"defaultAgentId":"a1"}}"#,
        )
        .expect("decodes");
        assert_eq!(
            decoded.ai_agents_enabled.as_deref(),
            Some(&["claude".to_string()][..])
        );
    }

    #[test]
    fn agents_enabled_decodes_from_the_documented_array() {
        let decoded: TaskList = serde_json::from_str(
            r#"{"id":"l1","name":"Home","aiAgentsEnabled":["claude","openai"]}"#,
        )
        .expect("decodes");
        assert_eq!(decoded.ai_agents_enabled.as_ref().map(Vec::len), Some(2));
    }

    /// Absent, null, and a shape nobody has seen yet all read as "no agents configured" rather
    /// than as a failed response.
    #[test]
    fn agents_enabled_survives_anything_else() {
        for body in [
            r#"{"id":"l1","name":"Home"}"#,
            r#"{"id":"l1","name":"Home","aiAgentsEnabled":null}"#,
            r#"{"id":"l1","name":"Home","aiAgentsEnabled":7}"#,
        ] {
            let decoded: TaskList = serde_json::from_str(body).expect("decodes");
            assert_eq!(decoded.ai_agents_enabled, None, "for {body}");
        }
    }

    /// nil means SHOW. Reading absent as false hides every subtask on every list saved before the
    /// field existed (task ba1deb9d).
    #[test]
    fn a_list_with_no_subtask_setting_still_shows_subtasks() {
        let decoded: TaskList =
            serde_json::from_str(r#"{"id":"l1","name":"Home"}"#).expect("decodes");
        assert!(decoded.shows_subtasks());

        let hidden: TaskList =
            serde_json::from_str(r#"{"id":"l1","name":"Home","showSubtasks":false}"#)
                .expect("decodes");
        assert!(!hidden.shows_subtasks());
    }

    #[test]
    fn status_lists_are_not_destinations() {
        let status: TaskList =
            serde_json::from_str(r#"{"id":"l1","name":"Doing","listType":"status"}"#)
                .expect("decodes");
        assert!(status.is_status_list());
        assert!(!status.is_domain_list());
        assert!(TaskList::new("l2", "Home").is_domain_list());
    }

    /// AITD-506: a GitHub milestone and a GitHub project iteration are tags, as a label is — chips
    /// on a task, never a sidebar list. Before this only a label was, so every sprint and every
    /// milestone drew as a list of its own.
    #[test]
    fn aitd506_milestones_and_iterations_are_tags_not_destinations() {
        for flavor in ["label", "milestone", "iteration"] {
            let list: TaskList = serde_json::from_str(&format!(
                r#"{{"id":"l1","name":"Sprint 4","listType":"{flavor}"}}"#
            ))
            .expect("decodes");
            assert!(list.is_tag_list(), "{flavor} is a tag");
            assert!(!list.is_domain_list(), "{flavor} is not a destination");
        }
        assert!(!TaskList::new("l2", "Home").is_tag_list());
        let status: TaskList =
            serde_json::from_str(r#"{"id":"l3","name":"Doing","listType":"status"}"#)
                .expect("decodes");
        assert!(!status.is_tag_list());
    }

    /// Task f8dfc84e: `recentlyCompletedWindow` is an unvalidated JSON column, and the
    /// `list.event` surface of `contracts/fixtures/wire-shapes.json` sends a value no version of
    /// web's type describes — `{ "amount": 3, "unit": "days" }`, no `kind`. A tagged decode
    /// answered `missing field \`kind\`` and failed the WHOLE list, and because collections decode
    /// leniently that is a list vanishing from the sidebar over one unreadable setting.
    #[test]
    fn f8dfc84e_an_unreadable_recently_completed_window_costs_the_setting_not_the_list() {
        for malformed in [
            r#"{"amount":3,"unit":"days"}"#,
            r#"{"kind":"duration","amount":3,"unit":"fortnight"}"#,
            r#"{"kind":"aModeFromLater"}"#,
            r#""3 days""#,
            r#"7"#,
        ] {
            let json =
                format!(r#"{{"id":"l1","name":"Board","recentlyCompletedWindow":{malformed}}}"#);
            let list: TaskList = serde_json::from_str(&json)
                .unwrap_or_else(|error| panic!("{malformed} must not fail the list: {error}"));
            assert_eq!(list.id, "l1");
            assert_eq!(
                list.recently_completed_window, None,
                "{malformed} reads as the legacy default"
            );
        }

        // A readable one still arrives, so the tolerance is not blanket.
        let good: TaskList = serde_json::from_str(
            r#"{"id":"l2","name":"Home","recentlyCompletedWindow":{"kind":"duration","amount":3,"unit":"day"}}"#,
        )
        .expect("decodes");
        assert_eq!(
            good.recently_completed_window,
            Some(RecentlyCompletedWindow::Duration {
                amount: 3,
                unit: DurationUnit::Day
            })
        );
    }

    #[test]
    fn the_recently_completed_window_reads_every_kind_web_writes() {
        let cases = [
            (
                r#"{"kind":"duration","amount":3,"unit":"day"}"#,
                RecentlyCompletedWindow::Duration {
                    amount: 3,
                    unit: DurationUnit::Day,
                },
            ),
            (
                r#"{"kind":"since-weekday","weekday":1}"#,
                RecentlyCompletedWindow::SinceWeekday { weekday: 1 },
            ),
            (
                r#"{"kind":"since-day-of-month","day":15}"#,
                RecentlyCompletedWindow::SinceDayOfMonth { day: 15 },
            ),
            (
                r#"{"kind":"since-date","date":"2026-01-01"}"#,
                RecentlyCompletedWindow::SinceDate {
                    date: "2026-01-01".into(),
                },
            ),
        ];
        for (body, expected) in cases {
            let decoded: RecentlyCompletedWindow = serde_json::from_str(body).expect("decodes");
            assert_eq!(decoded, expected);
            // It round-trips: the app writes this shape back in an update payload.
            assert_eq!(
                serde_json::from_str::<RecentlyCompletedWindow>(
                    &serde_json::to_string(&decoded).expect("encodes")
                )
                .expect("decodes"),
                expected
            );
        }
    }

    /// Embedded member arrays omit the membership id. Keying a member row on an empty string
    /// collapses every member into one row.
    #[test]
    fn a_member_without_an_id_is_keyed_by_its_user() {
        let decoded: ListMember =
            serde_json::from_str(r#"{"userId":"u1","role":"admin"}"#).expect("decodes");
        assert_eq!(decoded.identity(), "u1");
    }

    #[test]
    fn a_list_needs_only_an_id_and_a_name() {
        let decoded: TaskList = serde_json::from_str(r#"{"id":"l1"}"#).expect("decodes");
        assert_eq!(decoded.display_color(), "#3b82f6");
    }
}
