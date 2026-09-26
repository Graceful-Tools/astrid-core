//! What the task detail shows, and in what order.
//!
//! Ported from `astrid-ios/Astrid App/Core/Layout/TaskDetailFieldOrder.swift` (task c8a1ff51).
//!
//! The order is a product decision, stated once:
//!
//! > "Order (under task title) — Who, Date, Priority, Lists. Implement same order on all
//! > interfaces for list mode (iOS, Mac, web — board details, mobile etc)."
//!
//! It lives in the core rather than in a view because the point of it is **continuity**: the phone,
//! the Mac, the web and this window are supposed to agree. Both Apple platforms put Priority first
//! and Who second before it was written down — the same wrong order twice, which is what happens
//! when two views each decide for themselves.
//!
//! Project mode is deliberately absent. There, priority and assignee live behind the leading
//! control and are not rows at all (task 729a190e), so there is no order to state.

use crate::board::{BoardColumn, VIRTUAL_DONE_COLUMN_ID};
use crate::model::{Priority, Task, TaskList};

/// A field in the task detail, named so the order can be stated once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailField {
    /// "Who".
    Assignee,
    /// "Date".
    When,
    Priority,
    Lists,
}

impl DetailField {
    /// The stable name the shell dispatches on.
    pub fn name(self) -> &'static str {
        match self {
            DetailField::Assignee => "assignee",
            DetailField::When => "when",
            DetailField::Priority => "priority",
            DetailField::Lists => "lists",
        }
    }
}

/// List mode, top to bottom, under the title.
pub const LIST_MODE: [DetailField; 4] = [
    DetailField::Assignee,
    DetailField::When,
    DetailField::Priority,
    DetailField::Lists,
];

/// The order for a display mode.
pub fn field_order(display_mode: super::DisplayMode) -> &'static [DetailField] {
    match display_mode {
        super::DisplayMode::List => &LIST_MODE,
        // Project mode puts assignee and priority behind the leading control, so the rows that
        // remain are the two that are still rows.
        super::DisplayMode::Project => &[DetailField::When, DetailField::Lists],
    }
}

/// Whether the detail shows a BOARD STATE row.
///
/// The third copy of one rule: the Mac and iOS share `TaskDetailProjectStateRow.isVisible`
/// (AITD-327, AITD-332), and the web copied it as `showsTaskDetailProjectState` (task 5221e43f).
/// Copied rather than redesigned, for the reason the field order above is written down once.
///
/// - **List mode only.** In project mode the state already lives behind the leading control, and
///   a row would say it twice in the layout that is compact on purpose.
/// - **On a board only.** A board column is a project idea, and a row for one on a task that has
///   no column would rebuild exactly the hybrid the display-mode setting exists to end. For a
///   task that IS on a board, the column is real information the list layout was merely hiding.
/// - **Not read-only.** The row is a mover, not a label: its chips write. So it follows Who and
///   Priority, which are hidden from a public-list viewer rather than drawn as dead controls.
pub fn shows_board_state(
    display_mode: super::DisplayMode,
    is_in_project: bool,
    is_read_only: bool,
) -> bool {
    display_mode == super::DisplayMode::List && is_in_project && !is_read_only
}

/// Which project a task belongs to, from its OWN list memberships.
///
/// Deliberately not the list the reader has selected: a detail opens from search, from a label
/// or from another list entirely, and the task's board is a fact about the task rather than
/// about where the reader happened to be standing. A membership the reader cannot see names no
/// project — guessing one from an unknown id would draw chips from the wrong board.
pub fn project_id_for_task(task: &Task, lists: &[TaskList]) -> Option<String> {
    task.effective_list_ids().into_iter().find_map(|id| {
        lists
            .iter()
            .find(|list| list.id == id)
            .and_then(|list| list.project_id.clone())
    })
}

/// Does this task have a board column at all?
///
/// Either it already carries a status, or it is on a list that belongs to a project. The first
/// case matters because a task can hold a role from a board whose list the viewer cannot see, and
/// hiding the row would hide a state the task demonstrably has.
pub fn is_task_in_project(task: &Task, lists: &[TaskList]) -> bool {
    task.status_role.is_some() || project_id_for_task(task, lists).is_some()
}

/// The chips the row offers: every column except Done.
///
/// Done is what the checkbox is for (iOS task 7574067b). Offering it as a chip as well gave the
/// same action twice — and the chip was the one that never said it would finish the task. Inbox
/// stays: moving a task back out of Ready is a real thing to want, and it completes nothing.
pub fn board_state_chips(columns: &[BoardColumn]) -> Vec<&BoardColumn> {
    columns
        .iter()
        .filter(|column| column.id != VIRTUAL_DONE_COLUMN_ID)
        .collect()
}

/// The mark that stands for a priority.
///
/// One definition, because the `!`/`!!`/`!!!` convention was written out in half a dozen places on
/// Apple — the Mac's visuals, the list's sort keys, quick add, the model prompt — and a convention
/// spelled six times is one that drifts.
pub fn priority_glyph(priority: Priority) -> &'static str {
    match priority {
        Priority::None => "○",
        Priority::Low => "!",
        Priority::Medium => "!!",
        Priority::High => "!!!",
    }
}

/// The mark that identifies the priority row itself.
///
/// `!!!`, not a flag (task c8a1ff51). A flag says "some field about importance"; the marks are the
/// vocabulary the app already uses for priority everywhere else, so the row is recognisable
/// without reading its label.
pub const PRIORITY_ROW_ICON: &str = "!!!";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::DisplayMode;

    /// The order is the whole contract. Written as a literal so a change to it is a change
    /// somebody made on purpose, in a commit that can be mirrored to the other clients.
    #[test]
    fn list_mode_is_who_date_priority_lists() {
        let order: Vec<&str> = field_order(DisplayMode::List)
            .iter()
            .map(|field| field.name())
            .collect();
        assert_eq!(order, vec!["assignee", "when", "priority", "lists"]);
    }

    /// Project mode has no assignee or priority row: both live behind the leading control.
    #[test]
    fn project_mode_drops_the_two_fields_that_are_not_rows_there() {
        let order: Vec<&str> = field_order(DisplayMode::Project)
            .iter()
            .map(|field| field.name())
            .collect();
        assert_eq!(order, vec!["when", "lists"]);
    }

    fn list(id: &str, project_id: Option<&str>) -> TaskList {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "projectId": project_id
        }))
        .expect("a list")
    }

    fn task_in(list_ids: &[&str], status_role: Option<&str>) -> Task {
        let mut task = Task::new("t1", "t");
        task.list_ids = Some(list_ids.iter().map(|id| id.to_string()).collect());
        task.status_role = status_role.map(str::to_string);
        task
    }

    // ── The board-state row (task 5221e43f) ───────────────────────────────────────────────

    /// The row appears for a board task in list mode, and list mode is what a viewer who never
    /// touched the setting has.
    #[test]
    fn a_board_task_in_list_mode_shows_the_row() {
        assert!(shows_board_state(DisplayMode::List, true, false));
        assert!(shows_board_state(
            DisplayMode::from_stored(None),
            true,
            false
        ));
    }

    /// In project mode the quick changer already offers the state; a row would say it twice.
    #[test]
    fn project_mode_hides_the_row() {
        assert!(!shows_board_state(DisplayMode::Project, true, false));
    }

    /// A row for a state the task cannot have is the list/project hybrid the display-mode setting
    /// exists to end.
    #[test]
    fn a_task_with_no_board_column_has_no_row() {
        assert!(!shows_board_state(DisplayMode::List, false, false));
    }

    /// The chips write, so a read-only viewer does not get them.
    #[test]
    fn a_read_only_viewer_does_not_get_the_row() {
        assert!(!shows_board_state(DisplayMode::List, true, true));
    }

    /// The project comes from the task's own memberships, never from the selected list, and a
    /// membership the viewer cannot see names nothing.
    #[test]
    fn the_project_is_the_tasks_own_and_never_a_guess() {
        let board = list("board-list", Some("project-1"));
        let plain = list("plain-list", None);
        let lists = vec![board, plain];
        assert_eq!(
            project_id_for_task(&task_in(&["board-list"], None), &lists),
            Some("project-1".to_string())
        );
        assert_eq!(
            project_id_for_task(&task_in(&["plain-list"], None), &lists),
            None
        );
        assert_eq!(project_id_for_task(&task_in(&[], None), &lists), None);
        assert_eq!(
            project_id_for_task(&task_in(&["invisible"], None), &[list("plain-list", None)]),
            None,
            "a membership the viewer cannot see names no project"
        );
    }

    /// A task on a project list is on a board; so is one that already carries a role from a board
    /// whose list this viewer cannot see; an ordinary task on an ordinary list is not.
    #[test]
    fn in_project_means_a_project_list_or_a_role_already_carried() {
        let board = list("board-list", Some("project-1"));
        let plain = list("plain-list", None);
        assert!(is_task_in_project(
            &task_in(&["board-list"], None),
            &[board.clone()]
        ));
        assert!(is_task_in_project(
            &task_in(&["plain-list"], Some("ready")),
            &[plain.clone()]
        ));
        assert!(!is_task_in_project(
            &task_in(&["plain-list"], None),
            &[board, plain]
        ));
    }

    /// Done is never a chip — the checkbox already does that — and Inbox stays, because moving a
    /// task back out of Ready completes nothing. A board's custom states ride through, so the row
    /// and the board agree about which states exist (iOS hit the disagreement as AITD-379).
    #[test]
    fn the_chips_are_every_column_but_done() {
        let columns = crate::board::columns(Some(&serde_json::json!([
            { "role": "blocked", "name": "Blocked", "description": "Waiting on someone" }
        ])));
        assert!(columns
            .iter()
            .any(|column| column.id == VIRTUAL_DONE_COLUMN_ID));
        let chips = board_state_chips(&columns);
        assert!(!chips.iter().any(|chip| chip.id == VIRTUAL_DONE_COLUMN_ID));
        assert!(chips
            .iter()
            .any(|chip| chip.id == crate::board::VIRTUAL_INBOX_COLUMN_ID));
        assert!(chips.iter().any(|chip| chip.name == "Blocked"));
    }

    #[test]
    fn the_priority_marks_are_the_ones_used_everywhere_else() {
        assert_eq!(priority_glyph(Priority::None), "○");
        assert_eq!(priority_glyph(Priority::Low), "!");
        assert_eq!(priority_glyph(Priority::Medium), "!!");
        assert_eq!(priority_glyph(Priority::High), "!!!");
        assert_eq!(PRIORITY_ROW_ICON, priority_glyph(Priority::High));
    }
}
