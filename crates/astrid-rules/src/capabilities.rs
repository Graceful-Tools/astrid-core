//! Which editing controls a task's lists let an editor offer, and what deleting it is called.
//!
//! Ports `astrid-ios/Astrid App/Core/Tasks/TaskCapabilities.swift` (AITD-491, AITD-667), itself
//! web's `lib/backends/supports.ts`. A GitHub Projects board refuses a priority or due-date write
//! when the board has no such field mapped, and a private flag means nothing there, so the server
//! says so in `list.supports` and the editor does not offer the control.
//!
//! ## A list that has not loaded refuses nothing
//!
//! Only a list the task is on **and** the caller has in hand can refuse. Hiding a control that
//! works is the worse mistake: the server refuses a write it cannot take, while a control that
//! is not drawn is a feature nobody finds.

use serde::Serialize;

use crate::model::{DeleteTaskMode, TaskList};

/// What deleting a task is called. On a GitHub board an Astrid delete takes the item off the
/// project and the issue stays, so "Delete" would promise more than happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Deletion {
    Delete,
    RemoveFromList,
}

/// The controls a task's lists allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub priority: bool,
    pub due_date: bool,
    pub private_tasks: bool,
    pub deletion: Deletion,
}

/// What a task on `list_ids` may offer, judged by the lists among `lists` that it is on.
///
/// A control is allowed unless one of those lists refuses it. The deletion is a removal only when
/// some list removes and none deletes: a task also on a classic list really is deleted from
/// Astrid.
pub fn for_lists(list_ids: &[String], lists: &[TaskList]) -> Capabilities {
    let on: Vec<&TaskList> = lists
        .iter()
        .filter(|list| list_ids.contains(&list.id))
        .collect();
    let removes = on
        .iter()
        .any(|list| list.delete_task_mode() == DeleteTaskMode::RemoveFromList);
    let deletes = on
        .iter()
        .any(|list| list.delete_task_mode() == DeleteTaskMode::Delete);
    Capabilities {
        priority: on.iter().all(|list| list.supports_priority()),
        due_date: on.iter().all(|list| list.supports_due_date()),
        private_tasks: on.iter().all(|list| list.supports_private_tasks()),
        deletion: if removes && !deletes {
            Deletion::RemoveFromList
        } else {
            Deletion::Delete
        },
    }
}
