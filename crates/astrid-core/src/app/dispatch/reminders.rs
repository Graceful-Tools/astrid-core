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

/// The key a snooze made on this device is remembered under (D24): iOS reschedules its own
/// notification for the snoozed time; this is that, for the in-app reminder.
pub(super) fn snoozed_key(task_id: &str) -> String {
    format!("{SNOOZED_PREFIX}{task_id}")
}

const SNOOZED_PREFIX: &str = "reminder.snoozed.";

/// Every snooze mark on this device, read in one pass.
fn snoozes(app: &App) -> std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> {
    app.store
        .metadata_with_prefix(SNOOZED_PREFIX)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| {
            let id = key.strip_prefix(SNOOZED_PREFIX)?.to_string();
            Some((id, date::parse(&value)?))
        })
        .collect()
}

/// The reminders whose time has come and which have not been shown — for the command and for the
/// background loop, which must agree.
pub(in crate::app) fn due_reminders(
    app: &App,
    tasks: &[crate::model::Task],
) -> Vec<crate::reminders::Reminder> {
    let snoozes = snoozes(app);
    crate::reminders::due_now(
        tasks,
        app.clock.now(),
        |id| snoozes.get(id).copied(),
        |id, at| {
            app.store
                .metadata(&shown_key(id))
                .ok()
                .flatten()
                .is_some_and(|stamp| stamp == at.to_rfc3339())
        },
    )
}

/// Reminders whose time has come and which have not been shown.
pub(super) fn reminders_due(app: &App) -> Response {
    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let due = due_reminders(app, &tasks);
    Response::ok(serde_json::json!({ "reminders": due }))
}

pub(super) fn mark_reminder_shown(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let snoozed = app
        .store
        .metadata(&snoozed_key(task_id))
        .ok()
        .flatten()
        .and_then(|value| date::parse(&value));
    let Some(at) = crate::reminders::reminder_at(&task, snoozed) else {
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

/// Snooze a reminder, as iOS does (`ReminderPresenter.snoozeTask`, D24): the task becomes due at
/// now plus the snooze, as a timed task, and `reminderTime` is left where it was. The write goes
/// through the Outbox like any edit; the snooze mark brings the in-app reminder back then, the
/// way iOS reschedules its notification on the device.
pub(super) fn snooze_reminder(app: &App, task_id: &str, minutes: i64) -> Response {
    let when = crate::reminders::snooze_until(app.clock.now(), minutes);
    let changes = crate::services::TaskChanges {
        due_date_time: Some(Some(when)),
        is_all_day: Some(false),
        ..Default::default()
    };
    match app.context.tasks().update(task_id, &changes) {
        Ok(task) => {
            if let Err(error) = app
                .store
                .set_metadata(&snoozed_key(task_id), &date::format(when))
            {
                return Response::failed(error.into());
            }
            Response::ok(task)
        }
        Err(error) => Response::failed(error.into()),
    }
}
