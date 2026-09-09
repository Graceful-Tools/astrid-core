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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

    /// True for list-shaped destinations a person can navigate into and file tasks in.
    pub fn is_domain_list(&self) -> bool {
        !self.is_status_list()
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
