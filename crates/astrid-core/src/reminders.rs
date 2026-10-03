//! Which tasks are asking to be remembered, and when to ask again.
//!
//! Ports the decisions inside `astrid-ios/Astrid App/Core/Notifications/` — the parts that are
//! decisions rather than platform calls. Apple's presenter mixes the two: fetching the task,
//! deciding whether to show it, formatting the banner and scheduling the next one all live in the
//! same object, which is why none of it is tested there.
//!
//! ## Two kinds of reminder, and only one of them is ours
//!
//! The server sends push and email reminders from `reminderTime`, and it is the server that knows
//! about quiet hours, digests and every device a person owns. A client that also fired its own
//! notification for the same task would double every reminder.
//!
//! What a client can do that the server cannot is notice, while it is running, that a reminder has
//! come due for a task the person is looking at right now. That is what this decides — and because
//! a desktop app is often left running for days, "already shown" has to be remembered or the same
//! reminder arrives every time the loop ticks.
//!
//! ## Snoozing moves the due date, as iOS does
//!
//! `docs/CONTRACTS.md` D24, resolved toward iOS 2026-10-03. iOS's `ReminderPresenter.snoozeTask`
//! writes the task's `dueDateTime` to now plus the snooze, as a timed task, and leaves
//! `reminderTime` untouched; its notification is rescheduled on the device for the snoozed time.
//! The core does the same write, and the device-local half is a snooze mark in the cache's
//! metadata: the in-app reminder comes back at the later of `reminderTime` and that mark. (The v1
//! task PUT ignores `reminderTime` anyway, so the old `reminderTime` snooze never reached the
//! server; the due date does.)

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::model::Task;

/// How long after its time a reminder is still worth showing.
///
/// A laptop that was asleep for a week should not open onto forty banners for last Tuesday. An
/// hour is long enough to cover a lunch break and short enough that what arrives is still news.
pub const GRACE: Duration = Duration::hours(1);

/// The snooze choices, in minutes: 15 minutes, a day, a week — iOS's `ReminderView` (D24).
pub const SNOOZE_CHOICES: [i64; 3] = [15, 1440, 10080];

/// What a notification's own snooze action moves by: an hour, iOS's `ReminderAction.snoozeMinutes`.
pub const NOTIFICATION_SNOOZE_MINUTES: i64 = 60;

/// A task asking to be remembered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub task_id: String,
    pub title: String,
    /// When the reminder was for, so the shell can say "was due at 09:00" rather than "now".
    pub reminder_time: DateTime<Utc>,
    pub due_date_time: Option<DateTime<Utc>>,
}

/// When a task's reminder is next for: the later of its `reminderTime` and a snooze made on this
/// device. Either alone will do.
pub fn reminder_at(task: &Task, snoozed_until: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (task.reminder_time, snoozed_until) {
        (Some(at), Some(snoozed)) => Some(at.max(snoozed)),
        (at, snoozed) => at.or(snoozed),
    }
}

/// Which of `tasks` should be shown now.
///
/// `snoozed_until` answers a task's snooze mark on this device, if any (see the module note).
/// `shown` answers whether the reminder for a task at a given time has already been surfaced in
/// this installation. A reminder is shown once: a banner that comes back every thirty seconds is
/// one that gets dismissed without being read.
///
/// Ordered oldest first — if several came due while the machine was asleep, the one that has been
/// waiting longest is the one to answer first.
pub fn due_now(
    tasks: &[Task],
    now: DateTime<Utc>,
    snoozed_until: impl Fn(&str) -> Option<DateTime<Utc>>,
    shown: impl Fn(&str, DateTime<Utc>) -> bool,
) -> Vec<Reminder> {
    let mut due: Vec<Reminder> = tasks
        .iter()
        .filter(|task| !task.completed)
        .filter_map(|task| {
            let at = reminder_at(task, snoozed_until(task.id.as_str()))?;
            // Not yet, and not so long ago that it is history rather than a reminder.
            if at > now || now - at > GRACE {
                return None;
            }
            if shown(task.id.as_str(), at) {
                return None;
            }
            Some(Reminder {
                task_id: task.id.clone(),
                title: task.title.clone(),
                reminder_time: at,
                due_date_time: task.due_date_time,
            })
        })
        .collect();
    due.sort_by_key(|reminder| (reminder.reminder_time, reminder.task_id.clone()));
    due
}

/// When a reminder snoozed now should come back.
pub fn snooze_until(now: DateTime<Utc>, minutes: i64) -> DateTime<Utc> {
    // A snooze of nothing is a reminder that fires again immediately; a minute is the floor.
    now + Duration::minutes(minutes.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn at(text: &str) -> DateTime<Utc> {
        date::parse(text).expect("a date")
    }

    fn task(id: &str, reminder: Option<&str>) -> Task {
        Task {
            reminder_time: reminder.map(at),
            ..Task::new(id, format!("Task {id}"))
        }
    }

    fn none(_: &str, _: DateTime<Utc>) -> bool {
        false
    }

    fn unsnoozed(_: &str) -> Option<DateTime<Utc>> {
        None
    }

    #[test]
    fn a_reminder_whose_time_has_come_is_due() {
        let tasks = vec![task("t1", Some("2026-09-07T09:00:00Z"))];
        let found = due_now(&tasks, at("2026-09-07T09:00:30Z"), unsnoozed, none);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].task_id, "t1");
    }

    #[test]
    fn a_reminder_in_the_future_is_not() {
        let tasks = vec![task("t1", Some("2026-09-07T10:00:00Z"))];
        assert!(due_now(&tasks, at("2026-09-07T09:00:00Z"), unsnoozed, none).is_empty());
    }

    /// A laptop asleep for a week should not open onto forty banners for last Tuesday.
    #[test]
    fn a_reminder_old_enough_to_be_history_is_not_shown() {
        let tasks = vec![task("t1", Some("2026-09-01T09:00:00Z"))];
        assert!(due_now(&tasks, at("2026-09-07T09:00:00Z"), unsnoozed, none).is_empty());
    }

    /// A banner that comes back every thirty seconds is one that gets dismissed without reading.
    #[test]
    fn a_reminder_already_shown_is_not_shown_again() {
        let tasks = vec![task("t1", Some("2026-09-07T09:00:00Z"))];
        let found = due_now(&tasks, at("2026-09-07T09:00:30Z"), unsnoozed, |id, _| {
            id == "t1"
        });
        assert!(found.is_empty());
    }

    #[test]
    fn a_finished_task_does_not_remind_anybody() {
        let mut done = task("t1", Some("2026-09-07T09:00:00Z"));
        done.completed = true;
        assert!(due_now(&[done], at("2026-09-07T09:00:30Z"), unsnoozed, none).is_empty());
    }

    #[test]
    fn a_task_with_no_reminder_time_is_never_due() {
        assert!(due_now(
            &[task("t1", None)],
            at("2026-09-07T09:00:30Z"),
            unsnoozed,
            none
        )
        .is_empty());
    }

    /// Several came due while the machine was asleep: answer the one that has waited longest.
    #[test]
    fn the_oldest_reminder_comes_first() {
        let tasks = vec![
            task("newer", Some("2026-09-07T09:30:00Z")),
            task("older", Some("2026-09-07T09:00:00Z")),
        ];
        let found = due_now(&tasks, at("2026-09-07T09:31:00Z"), unsnoozed, none);
        let ids: Vec<&str> = found.iter().map(|r| r.task_id.as_str()).collect();
        assert_eq!(ids, vec!["older", "newer"]);
    }

    /// D24, resolved toward iOS: the reminder sheet offers 15 minutes, a day and a week
    /// (`ReminderView`).
    #[test]
    fn d24_snooze_choices_are_ios() {
        assert_eq!(SNOOZE_CHOICES.to_vec(), vec![15, 1440, 10080]);
        assert_eq!(NOTIFICATION_SNOOZE_MINUTES, 60);
    }

    /// The later of the two wins, so a snooze brings the reminder back without moving
    /// `reminderTime`, and a reminder set later than an old snooze is not held back by it.
    #[test]
    fn a_snooze_mark_moves_the_reminder_on_this_device() {
        let snoozed = task("t1", Some("2026-09-07T09:00:00Z"));
        let found = due_now(
            &[snoozed],
            at("2026-09-07T09:15:00Z"),
            |_| Some(at("2026-09-07T09:15:00Z")),
            none,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reminder_time, at("2026-09-07T09:15:00Z"));

        let later = task("t2", Some("2026-09-07T10:00:00Z"));
        assert_eq!(
            reminder_at(&later, Some(at("2026-09-07T09:15:00Z"))),
            Some(at("2026-09-07T10:00:00Z"))
        );
    }

    #[test]
    fn snoozing_moves_the_reminder_forward() {
        let now = at("2026-09-07T09:00:00Z");
        assert_eq!(snooze_until(now, 10), at("2026-09-07T09:10:00Z"));
        // A snooze of nothing would fire again immediately.
        assert_eq!(snooze_until(now, 0), at("2026-09-07T09:01:00Z"));
    }
}
