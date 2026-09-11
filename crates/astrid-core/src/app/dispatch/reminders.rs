//! Reminders coming due, shown and snoozed.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// When to be reminded about one task.
pub(super) fn reminder_options(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    Response::ok(serde_json::json!({
        "reminderTime": task.reminder_time.map(|at| at.to_rfc3339()),
        "picks": rows::reminder_picks::options(&task, app.clock.now()),
    }))
}

/// The key a shown reminder is remembered under.
///
/// The value is the reminder's own time, not a flag: a snoozed reminder has a new time, so the
/// same task can ask again without the mark having to be cleared by whoever moved it.
pub(super) fn shown_key(task_id: &str) -> String {
    format!("reminder.shown.{task_id}")
}

/// Reminders whose time has come and which have not been shown.
pub(super) fn reminders_due(app: &App) -> Response {
    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let now = app.clock.now();
    let due = crate::reminders::due_now(&tasks, now, |id| {
        let Some(task) = tasks.iter().find(|task| task.id == id) else {
            return false;
        };
        let Some(at) = task.reminder_time else {
            return false;
        };
        app.store
            .metadata(&shown_key(id))
            .ok()
            .flatten()
            .is_some_and(|stamp| stamp == at.to_rfc3339())
    });
    Response::ok(serde_json::json!({ "reminders": due }))
}

pub(super) fn mark_reminder_shown(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let Some(at) = task.reminder_time else {
        // Nothing to remember. Not an error: the reminder may have been cleared between the
        // banner going up and somebody dismissing it.
        return Response::done();
    };
    match app
        .store
        .set_metadata(&shown_key(task_id), &at.to_rfc3339())
    {
        Ok(()) => Response::done(),
        Err(error) => Response::failed(error.into()),
    }
}

/// Move a reminder forward.
///
/// A write rather than a timer: an in-memory snooze is lost on a restart, and it leaves the
/// server's copy of the reminder where it was, so the push still arrives at the original time.
pub(super) fn snooze_reminder(app: &App, task_id: &str, minutes: i64) -> Response {
    let when = crate::reminders::snooze_until(app.clock.now(), minutes);
    let changes = crate::services::TaskChanges {
        reminder_time: Some(Some(when)),
        ..Default::default()
    };
    match app.context.tasks().update(task_id, &changes) {
        Ok(task) => Response::ok(task),
        Err(error) => Response::failed(error.into()),
    }
}
