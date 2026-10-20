//! Where a dragged row lands in the subtask tree (AATD-42) — iOS's `DragNesting` and
//! `SubtaskPromotion`, so every client nests the same way.
//!
//! A row dropped ON another row becomes its subtask; dropped on the LINE between rows it leaves
//! its parent for the top level (a top-level row just reorders); pulled into the outdent gutter it
//! climbs one level; indented from the keyboard it nests under the previous sibling. A task can
//! never be nested under itself or anything beneath it.

use std::collections::HashMap;

use crate::model::Task;

/// Where the drop happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zone {
    /// On a row: become its subtask.
    OnRow(String),
    /// On the insertion line between rows.
    Line,
    /// In the leading gutter: up one level.
    Outdent,
    /// The keyboard's indent: under the previous sibling, among these rows as drawn.
    Indent(Vec<String>),
}

/// What the drop means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    MakeSubtask { parent_id: String },
    MoveToTopLevel,
    ReorderOnly,
    None,
}

impl Outcome {
    pub fn name(&self) -> &'static str {
        match self {
            Outcome::MakeSubtask { .. } => "makeSubtask",
            Outcome::MoveToTopLevel => "moveToTopLevel",
            Outcome::ReorderOnly => "reorderOnly",
            Outcome::None => "none",
        }
    }
}

fn parent_of(task: &Task) -> Option<&str> {
    task.parent_task_id.as_deref().filter(|id| !id.is_empty())
}

/// Whether `child` may sit under `parent`: never itself, never under its own descendant.
pub fn can_parent(child_id: &str, parent_id: &str, by_id: &HashMap<String, &Task>) -> bool {
    if child_id == parent_id {
        return false;
    }
    let mut cursor = Some(parent_id.to_string());
    let mut steps = 0;
    while let Some(id) = cursor {
        if steps >= 100 {
            break;
        }
        if id == child_id {
            return false;
        }
        cursor = by_id
            .get(&id)
            .and_then(|task| parent_of(task))
            .map(str::to_string);
        steps += 1;
    }
    true
}

/// The outcome of dropping `dragged` in `zone`.
pub fn outcome(zone: &Zone, dragged: &Task, by_id: &HashMap<String, &Task>) -> Outcome {
    match zone {
        Zone::OnRow(target) => {
            if !can_parent(&dragged.id, target, by_id)
                || parent_of(dragged) == Some(target.as_str())
            {
                Outcome::None
            } else {
                Outcome::MakeSubtask {
                    parent_id: target.clone(),
                }
            }
        }
        Zone::Line => {
            if parent_of(dragged).is_some() {
                Outcome::MoveToTopLevel
            } else {
                Outcome::ReorderOnly
            }
        }
        Zone::Outdent => outdent(dragged, by_id),
        Zone::Indent(rows) => indent(dragged, rows, by_id),
    }
}

fn outdent(task: &Task, by_id: &HashMap<String, &Task>) -> Outcome {
    let Some(parent_id) = parent_of(task) else {
        return Outcome::None;
    };
    match by_id.get(parent_id).and_then(|parent| parent_of(parent)) {
        Some(grandparent) => Outcome::MakeSubtask {
            parent_id: grandparent.to_string(),
        },
        None => Outcome::MoveToTopLevel,
    }
}

fn indent(task: &Task, rows: &[String], by_id: &HashMap<String, &Task>) -> Outcome {
    let Some(index) = rows
        .iter()
        .position(|id| id == &task.id)
        .filter(|index| *index > 0)
    else {
        return Outcome::None;
    };
    let parent = parent_of(task);
    for id in rows[..index].iter().rev() {
        let Some(row) = by_id.get(id) else { continue };
        if parent_of(row) != parent {
            continue;
        }
        return if can_parent(&task.id, &row.id, by_id) {
            Outcome::MakeSubtask {
                parent_id: row.id.clone(),
            }
        } else {
            Outcome::None
        };
    }
    Outcome::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, parent: Option<&str>) -> Task {
        let mut task = Task::new(id, id);
        task.parent_task_id = parent.map(str::to_string);
        task
    }

    fn index(tasks: &[Task]) -> HashMap<String, &Task> {
        tasks.iter().map(|task| (task.id.clone(), task)).collect()
    }

    // iOS DragNestingTests, one for one.
    #[test]
    fn aatd_42_dropping_on_a_row_makes_it_a_subtask_of_that_row() {
        let tasks = [task("a", None), task("b", None)];
        assert_eq!(
            outcome(&Zone::OnRow("b".into()), &tasks[0], &index(&tasks)),
            Outcome::MakeSubtask {
                parent_id: "b".into()
            }
        );
    }

    #[test]
    fn aatd_42_a_task_cannot_become_its_own_subtask_nor_its_descendants() {
        let tasks = [
            task("a", None),
            task("child", Some("a")),
            task("grandchild", Some("child")),
        ];
        let by_id = index(&tasks);
        assert_eq!(
            outcome(&Zone::OnRow("a".into()), &tasks[0], &by_id),
            Outcome::None
        );
        assert_eq!(
            outcome(&Zone::OnRow("grandchild".into()), &tasks[0], &by_id),
            Outcome::None
        );
    }

    #[test]
    fn aatd_42_dropping_on_the_parent_it_already_has_is_a_no_op() {
        let tasks = [task("a", None), task("child", Some("a"))];
        assert_eq!(
            outcome(&Zone::OnRow("a".into()), &tasks[1], &index(&tasks)),
            Outcome::None
        );
    }

    #[test]
    fn aatd_42_the_line_promotes_a_subtask_and_reorders_a_top_level_task() {
        let tasks = [task("a", None), task("child", Some("a"))];
        let by_id = index(&tasks);
        assert_eq!(
            outcome(&Zone::Line, &tasks[1], &by_id),
            Outcome::MoveToTopLevel
        );
        assert_eq!(
            outcome(&Zone::Line, &tasks[0], &by_id),
            Outcome::ReorderOnly
        );
    }

    #[test]
    fn aatd_42_outdent_climbs_one_level_and_a_top_level_task_has_nowhere_to_go() {
        let tasks = [
            task("a", None),
            task("child", Some("a")),
            task("grandchild", Some("child")),
        ];
        let by_id = index(&tasks);
        assert_eq!(
            outcome(&Zone::Outdent, &tasks[1], &by_id),
            Outcome::MoveToTopLevel
        );
        assert_eq!(
            outcome(&Zone::Outdent, &tasks[2], &by_id),
            Outcome::MakeSubtask {
                parent_id: "a".into()
            }
        );
        assert_eq!(outcome(&Zone::Outdent, &tasks[0], &by_id), Outcome::None);
    }

    #[test]
    fn aatd_42_indent_nests_under_the_previous_sibling_and_back_out_again() {
        let tasks = [
            task("a", None),
            task("b", None),
            task("c", Some("b")),
            task("d", Some("b")),
        ];
        let by_id = index(&tasks);
        let rows: Vec<String> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(
            outcome(&Zone::Indent(rows.clone()), &tasks[1], &by_id),
            Outcome::MakeSubtask {
                parent_id: "a".into()
            }
        );
        // The first child cannot indent further; the second goes under the first.
        assert_eq!(
            outcome(&Zone::Indent(rows.clone()), &tasks[2], &by_id),
            Outcome::None
        );
        assert_eq!(
            outcome(&Zone::Indent(rows.clone()), &tasks[3], &by_id),
            Outcome::MakeSubtask {
                parent_id: "c".into()
            }
        );
        // The first row cannot indent.
        assert_eq!(
            outcome(&Zone::Indent(rows), &tasks[0], &by_id),
            Outcome::None
        );
    }
}
