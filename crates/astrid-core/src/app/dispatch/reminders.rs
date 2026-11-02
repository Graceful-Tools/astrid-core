//! Reminders coming due, shown and snoozed.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Every command about a reminder, handed here whole by [`super::run`]
/// (task 8a91c5b0).
///
/// Every arm is short on purpose: it names a service call, and the service decides. An arm that
/// grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file nobody
/// would look in for it.
pub(super) fn run(app: &App, command: Command) -> Response {
    match command {
        Command::ReminderOptions { task_id } => reminder_options(app, &task_id),
        Command::RemindersDue => reminders_due(app),
        Command::NextReminder => next_reminder(app),
        Command::ReminderShown { task_id } => mark_reminder_shown(app, &task_id),
        Command::SnoozeReminder { task_id, minutes } => snooze_reminder(app, &task_id, minutes),
        // `super::run` is one exhaustive match over every variant and the groups it routes are
        // disjoint, so the compiler has already proved nothing else reaches this arm.
        _ => misrouted("reminders"),
    }
}

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

/// The tasks a reminder read has to consider, and only those (task `71ddacb8`).
///
/// Two sources, because [`crate::reminders::reminder_at`] has two: the `reminder_time` column
/// migration 2 lifted out, in the window the caller asks for, **and** the tasks carrying a snooze
/// mark made on this device. The second is not optional — a snooze alone qualifies a task whose
/// own `reminderTime` is `None` (`at.or(snoozed)`), so a read bounded on the column alone would
/// silently stop showing snoozed reminders.
///
/// Both are bounded: the window is an index range, and the snooze marks are a handful of rows
/// somebody tapped. Deduplicated by id, since a snoozed task may also be in the window.
fn candidates(
    app: &App,
    snoozes: &std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
    window: impl FnOnce() -> crate::store::Result<Vec<crate::model::Task>>,
) -> crate::store::Result<Vec<crate::model::Task>> {
    let mut tasks = window()?;
    let held: std::collections::HashSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    let missing: Vec<String> = snoozes
        .keys()
        .filter(|id| !held.contains(id.as_str()))
        .cloned()
        .collect();
    let snoozed = app.store.tasks_with_ids(&missing)?;
    tasks.extend(snoozed);
    Ok(tasks)
}

/// The reminders whose time has come and which have not been shown — for the command and for the
/// background loop, which must agree.
///
/// Reads its own candidates rather than taking the account: the loop behind it runs every thirty
/// seconds for as long as the app is open, and it used to decode every task in the cache each time
/// to find the nothing that was usually there (task `71ddacb8`).
pub(in crate::app) fn due_reminders(
    app: &App,
) -> crate::store::Result<Vec<crate::reminders::Reminder>> {
    let snoozes = snoozes(app);
    let now = app.clock.now();
    // The same window `due_now` keeps: not yet due is not due, and older than GRACE is history.
    // Snoozes can only move a reminder later, so the window's lower edge is safe to apply in SQL.
    let tasks = candidates(app, &snoozes, || {
        app.store
            .tasks_with_reminder_between(now - crate::reminders::GRACE, now)
    })?;
    Ok(due_now_among(app, &tasks, &snoozes))
}

/// [`crate::reminders::due_now`] over tasks already gathered, with this device's marks.
fn due_now_among(
    app: &App,
    tasks: &[crate::model::Task],
    snoozes: &std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
) -> Vec<crate::reminders::Reminder> {
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

/// The earliest reminder still to come, by the same reading as [`due_reminders`] (AATD-21).
pub(super) fn next_reminder(app: &App) -> Response {
    let snoozes = snoozes(app);
    let now = app.clock.now();
    // Everything with a reminder still ahead of it, rather than everything. Not a `LIMIT 1`: a
    // snooze or an already-shown mark can disqualify the earliest row, and SQL can see neither.
    let tasks = match candidates(app, &snoozes, || app.store.tasks_with_reminder_after(now)) {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let next = crate::reminders::next_after(
        &tasks,
        app.clock.now(),
        |id| snoozes.get(id).copied(),
        |id, at| {
            app.store
                .metadata(&shown_key(id))
                .ok()
                .flatten()
                .is_some_and(|stamp| stamp == at.to_rfc3339())
        },
    );
    Response::ok(serde_json::json!({ "reminder": next }))
}

/// Reminders whose time has come and which have not been shown.
pub(super) fn reminders_due(app: &App) -> Response {
    match due_reminders(app) {
        Ok(due) => Response::ok(serde_json::json!({ "reminders": due })),
        Err(error) => Response::failed(error.into()),
    }
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
