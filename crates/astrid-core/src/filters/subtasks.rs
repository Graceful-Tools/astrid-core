//! Subtasks in a flat list: whether they appear, and where.
//!
//! Ported from `astrid-ios/Astrid App/Core/Filters/SubtaskSplicing.swift` and
//! `ListSubtaskVisibility.swift`, whose canonical twin is `astrid-web/lib/list-subtask-visibility.ts`.
//!
//! ## Two settings, and which one wins
//!
//! - The **user** setting `subtaskDisplay` decides where subtasks appear at all: `indented`
//!   (inline under their parent) or `under_parent` (in the parent's detail view only).
//! - The **list** setting `showSubtasks` decides whether this particular list splices them inline.
//!
//! A list can opt out; it cannot opt back in over a user who has chosen detail-only, because that
//! choice is about the whole product rather than one list. The more restrictive wins, and the
//! user's is the one that cannot be overridden.
//!
//! **Absent means show** (task ba1deb9d). Every list decoded by a build older than the field reads
//! back as absent, as does any response that omits it. Defaulting to hide would silently empty all
//! of them, on a screen where nothing looks wrong.

use crate::model::Task;

/// The user-level display mode that means "detail view only".
pub const DETAIL_ONLY_DISPLAY: &str = "under_parent";

/// How deep the splice will follow a chain of parents. A guard against bad data, not a product
/// limit: a cycle in `parent_task_id` would otherwise be an infinite list.
pub const MAX_SPLICE_DEPTH: usize = 10;

/// How deep [`depth_of`] will walk before giving up, for the same reason.
pub const MAX_DEPTH_WALK: usize = 8;

/// True unless the list has explicitly turned subtasks off.
pub fn list_shows_subtasks(show_subtasks: Option<bool>) -> bool {
    show_subtasks != Some(false)
}

/// Whether this list should splice subtasks in under their parents.
///
/// Anything other than `under_parent` — including absent, and a mode a future build introduces
/// that this one has never heard of — means inline is wanted. Treating an unknown mode as "hide"
/// would blank out every list on the older client.
pub fn should_splice(list_show_subtasks: Option<bool>, subtask_display: Option<&str>) -> bool {
    if subtask_display == Some(DETAIL_ONLY_DISPLAY) {
        return false;
    }
    list_shows_subtasks(list_show_subtasks)
}

/// What to send for `showSubtasks`, or `None` to leave the key out of the payload entirely.
///
/// Only an actual change is sent. The web handler writes the column only when the caller sends a
/// real boolean, precisely so a client PUTing a whole list object cannot reset somebody's toggle;
/// this is the same guard from the other side. Sending `false` merely because a decoded model
/// defaulted would turn every unrelated list edit — a rename, a colour change — into a silent
/// "hide the subtasks".
pub fn payload_value(original: Option<bool>, edited: Option<bool>) -> Option<bool> {
    if original == edited {
        return None;
    }
    // An edit back to "no opinion" still has to say something on the wire, and the value that
    // means no opinion is `true` — absent means show.
    Some(edited.unwrap_or(true))
}

/// Splice each visible parent's subtasks in directly after it, depth first.
///
/// `top_level` is the already filtered and sorted set of rows with no parent. `all_tasks` is
/// everything, used to find children at any depth. `is_visible` decides which children show —
/// usually the same completion rule the parent rows went through, so a completed subtask under an
/// open parent obeys the list's completion filter rather than appearing regardless.
pub fn splice(
    top_level: &[Task],
    all_tasks: &[Task],
    indented: bool,
    is_visible: impl Fn(&Task) -> bool,
) -> Vec<Task> {
    if !indented {
        return top_level.to_vec();
    }

    let mut by_parent: std::collections::HashMap<&str, Vec<&Task>> =
        std::collections::HashMap::new();
    for task in all_tasks {
        if let Some(parent) = task.parent_task_id.as_deref() {
            by_parent.entry(parent).or_default().push(task);
        }
    }
    if by_parent.is_empty() {
        return top_level.to_vec();
    }

    // Children of one parent are ordered oldest first — the order they were added in, which is the
    // order somebody breaking a task down expects to read them back in.
    for children in by_parent.values_mut() {
        children.sort_by_key(|task| {
            (
                task.created_at
                    .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC),
                task.id.clone(),
            )
        });
    }

    let mut out = Vec::with_capacity(top_level.len());
    for task in top_level {
        append_subtree(task, &by_parent, &is_visible, 0, &mut out);
    }
    out
}

fn append_subtree(
    task: &Task,
    by_parent: &std::collections::HashMap<&str, Vec<&Task>>,
    is_visible: &impl Fn(&Task) -> bool,
    depth: usize,
    out: &mut Vec<Task>,
) {
    out.push(task.clone());
    if depth >= MAX_SPLICE_DEPTH {
        return;
    }
    let Some(children) = by_parent.get(task.id.as_str()) else {
        return;
    };
    for child in children {
        if is_visible(child) {
            append_subtree(child, by_parent, is_visible, depth + 1, out);
        }
    }
}

/// How deeply nested a task is: 0 for top level.
///
/// Capped, so a `parent_task_id` cycle — which bad data has produced before — costs a wrong
/// indent rather than a hang.
pub fn depth_of(task: &Task, by_id: &std::collections::HashMap<String, Task>) -> usize {
    let mut depth = 0;
    let mut parent = task.parent_task_id.clone();
    while let Some(id) = parent {
        if depth >= MAX_DEPTH_WALK {
            break;
        }
        depth += 1;
        parent = by_id.get(&id).and_then(|task| task.parent_task_id.clone());
    }
    depth
}

/// Index the tasks by id, for [`depth_of`].
pub fn by_id(tasks: &[Task]) -> std::collections::HashMap<String, Task> {
    tasks
        .iter()
        .map(|task| (task.id.clone(), task.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn child(id: &str, parent: &str, created: &str) -> Task {
        let mut task = Task::new(id, id);
        task.parent_task_id = Some(parent.to_string());
        task.created_at = date::parse(created);
        task
    }

    fn ids(tasks: &[Task]) -> Vec<&str> {
        tasks.iter().map(|task| task.id.as_str()).collect()
    }

    /// nil means show. Reading absent as false hides every subtask on every list saved before the
    /// field existed (task ba1deb9d).
    #[test]
    fn a_list_with_no_opinion_shows_subtasks() {
        assert!(list_shows_subtasks(None));
        assert!(list_shows_subtasks(Some(true)));
        assert!(!list_shows_subtasks(Some(false)));
    }

    /// The list can opt out; it cannot opt back in over a user who chose detail-only.
    #[test]
    fn the_user_setting_cannot_be_overridden_by_a_list() {
        assert!(!should_splice(Some(true), Some(DETAIL_ONLY_DISPLAY)));
        assert!(!should_splice(None, Some(DETAIL_ONLY_DISPLAY)));
        assert!(should_splice(Some(true), Some("indented")));
        assert!(should_splice(None, None));
        assert!(!should_splice(Some(false), Some("indented")));
    }

    /// A display mode a newer build introduces means inline, not hidden. Treating it as hidden
    /// blanks out every list on the older client.
    #[test]
    fn a_display_mode_this_build_does_not_know_still_shows_subtasks() {
        assert!(should_splice(None, Some("somethingLater")));
    }

    /// Sending `false` because a decoded model defaulted turns a rename into a silent "hide the
    /// subtasks".
    #[test]
    fn only_a_real_change_is_sent() {
        assert_eq!(payload_value(None, None), None);
        assert_eq!(payload_value(Some(true), Some(true)), None);
        assert_eq!(payload_value(Some(true), Some(false)), Some(false));
        assert_eq!(
            payload_value(Some(false), None),
            Some(true),
            "back to no opinion still has to be said, and no opinion means show"
        );
    }

    #[test]
    fn children_are_spliced_in_under_their_parent_oldest_first() {
        let parent = Task::new("p", "Parent");
        let all = vec![
            parent.clone(),
            child("c2", "p", "2026-09-07T12:00:00Z"),
            child("c1", "p", "2026-09-06T12:00:00Z"),
        ];
        let spliced = splice(&[parent], &all, true, |_| true);
        assert_eq!(ids(&spliced), vec!["p", "c1", "c2"]);
    }

    #[test]
    fn nesting_is_followed_depth_first() {
        let parent = Task::new("p", "Parent");
        let all = vec![
            parent.clone(),
            child("c", "p", "2026-09-06T12:00:00Z"),
            child("g", "c", "2026-09-06T13:00:00Z"),
            child("c2", "p", "2026-09-07T12:00:00Z"),
        ];
        let spliced = splice(&[parent], &all, true, |_| true);
        assert_eq!(ids(&spliced), vec!["p", "c", "g", "c2"]);
    }

    /// The visibility predicate is how a completed subtask obeys the list's completion filter
    /// rather than appearing regardless.
    #[test]
    fn a_hidden_child_takes_its_own_children_with_it() {
        let parent = Task::new("p", "Parent");
        let mut hidden = child("c", "p", "2026-09-06T12:00:00Z");
        hidden.completed = true;
        let all = vec![
            parent.clone(),
            hidden,
            child("g", "c", "2026-09-06T13:00:00Z"),
        ];
        let spliced = splice(&[parent], &all, true, |task| !task.completed);
        assert_eq!(ids(&spliced), vec!["p"]);
    }

    #[test]
    fn with_splicing_off_the_top_level_rows_are_the_whole_list() {
        let parent = Task::new("p", "Parent");
        let all = vec![parent.clone(), child("c", "p", "2026-09-06T12:00:00Z")];
        assert_eq!(ids(&splice(&[parent], &all, false, |_| true)), vec!["p"]);
    }

    /// Bad data has produced a parent chain that loops. It should cost a wrong indent, not a hang.
    #[test]
    fn a_cycle_in_the_parent_chain_terminates() {
        let mut a = Task::new("a", "A");
        a.parent_task_id = Some("b".into());
        let mut b = Task::new("b", "B");
        b.parent_task_id = Some("a".into());

        let index = by_id(&[a.clone(), b.clone()]);
        assert_eq!(depth_of(&a, &index), MAX_DEPTH_WALK);

        // And the splice, which walks the other way, also terminates.
        let spliced = splice(&[a.clone()], &[a, b], true, |_| true);
        assert!(spliced.len() <= MAX_SPLICE_DEPTH + 2);
    }

    #[test]
    fn depth_counts_the_parents_above_a_task() {
        let parent = Task::new("p", "Parent");
        let c = child("c", "p", "2026-09-06T12:00:00Z");
        let g = child("g", "c", "2026-09-06T13:00:00Z");
        let index = by_id(&[parent.clone(), c.clone(), g.clone()]);
        assert_eq!(depth_of(&parent, &index), 0);
        assert_eq!(depth_of(&c, &index), 1);
        assert_eq!(depth_of(&g, &index), 2);
    }
}
