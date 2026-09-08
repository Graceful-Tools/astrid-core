//! My Tasks: what one person is actually holding, wherever it lives.
//!
//! Ports `astrid-ios/Astrid Mac/App/MacMyTasks.swift` and the `MyTasksPreferences` model beside
//! it. Not a list and not one of the virtual lists either: a virtual list is a saved set of
//! filters that belongs to the account's list collection, and this is the view the app opens on.
//!
//! ## Why the preferences are shaped differently from a list's
//!
//! A list stores one priority as a string; My Tasks stores a *set* of them, because its filter
//! sheet offers checkboxes. Empty means "all" — and empty is the default, so reading it as "match
//! nothing" blanks the view for everybody who has never opened the sheet. That mistake is why the
//! Mac's port says so in a comment, and it is repeated here.
//!
//! ## Scope is not a filter
//!
//! "Mine or nobody's" is what My Tasks *is*, not something the sheet can change. An unassigned
//! task counts: it is the pile of things nobody has picked up, which is exactly what somebody
//! opening this view is looking for.

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

use crate::model::Task;

/// The id the shell asks for these rows under.
///
/// Not a real list id and deliberately shaped so it cannot collide with one: nothing on the server
/// is named this, so a stale reference resolves to nothing rather than to somebody's list.
pub const VIRTUAL_ID: &str = "virtual:my-tasks";

/// What My Tasks is filtered and sorted by, for this account, on every device.
///
/// Held by the account rather than the machine, which is the whole reason this exists as its own
/// endpoint: filters set on a laptop are the filters on the desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// Empty means every priority — see the module note.
    #[serde(default)]
    pub filter_priority: Vec<i64>,
    /// Empty means everybody the scope already allows.
    #[serde(default)]
    pub filter_assignee: Vec<String>,
    #[serde(default = "all")]
    pub filter_due_date: String,
    #[serde(default = "default_completion")]
    pub filter_completion: String,
    #[serde(default = "auto")]
    pub sort_by: String,
    #[serde(default)]
    pub manual_sort_order: Vec<String>,
}

fn all() -> String {
    "all".into()
}

fn default_completion() -> String {
    "default".into()
}

fn auto() -> String {
    "auto".into()
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            filter_priority: Vec::new(),
            filter_assignee: Vec::new(),
            filter_due_date: all(),
            filter_completion: default_completion(),
            sort_by: auto(),
            manual_sort_order: Vec::new(),
        }
    }
}

/// My Tasks, with this account's saved filters applied.
///
/// The preferences are passed in rather than read from anywhere, for the same reason the list
/// filters take the current user id: every rule here is then a test rather than a screen somebody
/// has to set up.
pub fn filter<'a>(
    tasks: &'a [Task],
    current_user_id: Option<&str>,
    preferences: &Preferences,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<&'a Task> {
    // The list's own filter fields, filled in from the preferences, so completion and due-date
    // mean exactly what they mean everywhere else — including the recently-completed window and
    // the undated-task rule in `docs/CONTRACTS.md` D7. Two implementations of "due this week" is
    // two answers to the same question.
    let mut shape = crate::model::TaskList::new(VIRTUAL_ID, "");
    shape.filter_completion = Some(preferences.filter_completion.clone());
    shape.filter_due_date = Some(preferences.filter_due_date.clone());

    super::filter_refs(tasks, &shape, current_user_id, now, offset)
        .into_iter()
        .filter(|task| in_scope(task, current_user_id))
        .filter(|task| {
            preferences.filter_priority.is_empty()
                || preferences
                    .filter_priority
                    .contains(&task.priority.as_i64())
        })
        .filter(|task| {
            preferences.filter_assignee.is_empty()
                || task
                    .assignee_id
                    .as_deref()
                    .is_some_and(|id| preferences.filter_assignee.iter().any(|held| held == id))
        })
        .collect()
}

/// Yours, or nobody's.
///
/// Signed out this is nobody's tasks rather than everybody's: showing every task in the account as
/// if it were yours is the more alarming of the two failures, which is the rule the list filters
/// follow too.
fn in_scope(task: &Task, current_user_id: Option<&str>) -> bool {
    let assignee = task
        .assignee_id
        .as_deref()
        .or(task.assignee.as_ref().map(|user| user.id.as_str()));
    match assignee {
        None => true,
        Some(assignee) => current_user_id == Some(assignee),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{date, Priority};

    fn now() -> DateTime<Utc> {
        date::parse("2026-09-07T12:00:00Z").expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("a zone")
    }

    fn task(id: &str, assignee: Option<&str>) -> Task {
        let mut task = Task::new(id, id);
        task.assignee_id = assignee.map(str::to_string);
        task
    }

    /// The pile of things nobody has picked up is exactly what somebody opening this is looking
    /// for, so an unassigned task belongs here.
    #[test]
    fn my_tasks_is_yours_and_nobodys() {
        let tasks = vec![
            task("mine", Some("u1")),
            task("nobodys", None),
            task("theirs", Some("u2")),
        ];

        let held = filter(&tasks, Some("u1"), &Preferences::default(), now(), utc());

        let ids: Vec<&str> = held.iter().map(|task| task.id.as_str()).collect();
        assert_eq!(ids, vec!["mine", "nobodys"]);
    }

    /// Signed out, this is nobody's tasks rather than everybody's.
    #[test]
    fn signed_out_it_holds_only_what_nobody_owns() {
        let tasks = vec![task("mine", Some("u1")), task("nobodys", None)];

        let held = filter(&tasks, None, &Preferences::default(), now(), utc());

        assert_eq!(held.len(), 1);
        assert_eq!(held[0].id, "nobodys");
    }

    /// The mistake the Mac's port calls out: empty is the default, so reading it as "match
    /// nothing" blanks the view for everybody who has never opened the filter sheet.
    #[test]
    fn no_priorities_chosen_means_every_priority() {
        let mut high = task("high", Some("u1"));
        high.priority = Priority::from_i64(1);
        let mut low = task("low", Some("u1"));
        low.priority = Priority::from_i64(3);
        let tasks = vec![high, low];

        let held = filter(&tasks, Some("u1"), &Preferences::default(), now(), utc());

        assert_eq!(held.len(), 2);
    }

    #[test]
    fn a_chosen_priority_keeps_only_that_one() {
        let mut high = task("high", Some("u1"));
        high.priority = Priority::from_i64(1);
        let mut low = task("low", Some("u1"));
        low.priority = Priority::from_i64(3);
        let tasks = vec![high, low];

        let preferences = Preferences {
            filter_priority: vec![1],
            ..Default::default()
        };
        let held = filter(&tasks, Some("u1"), &preferences, now(), utc());

        assert_eq!(held.len(), 1);
        assert_eq!(held[0].id, "high");
    }

    /// Completion goes through the shared rule, so "default" here means what it means in a list —
    /// including the recently-completed window.
    #[test]
    fn a_completed_task_is_hidden_by_default() {
        let mut done = task("done", Some("u1"));
        done.completed = true;
        done.completed_at = date::parse("2026-01-01T12:00:00Z");
        let tasks = vec![done, task("open", Some("u1"))];

        let held = filter(&tasks, Some("u1"), &Preferences::default(), now(), utc());

        assert_eq!(held.len(), 1);
        assert_eq!(held[0].id, "open");
    }

    #[test]
    fn asking_for_completed_tasks_shows_them() {
        let mut done = task("done", Some("u1"));
        done.completed = true;
        done.completed_at = date::parse("2026-01-01T12:00:00Z");
        let tasks = vec![done];

        let preferences = Preferences {
            filter_completion: "all".into(),
            ..Default::default()
        };
        let held = filter(&tasks, Some("u1"), &preferences, now(), utc());

        assert_eq!(held.len(), 1);
    }

    /// The server sends a blob it has been storing for years. A field this build has never heard
    /// of, or one that is simply missing, must not stop the view drawing.
    #[test]
    fn preferences_the_server_sends_with_fields_missing_still_read() {
        let read: Preferences =
            serde_json::from_str(r#"{ "sortBy": "when", "unknownField": 7 }"#).expect("reads");

        assert_eq!(read.sort_by, "when");
        assert_eq!(read.filter_due_date, "all");
        assert_eq!(read.filter_completion, "default");
        assert!(read.filter_priority.is_empty());
    }
}
