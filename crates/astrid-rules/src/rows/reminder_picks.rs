//! When to be reminded about one task.
//!
//! Ports the offsets in `astrid-ios/Astrid App/Models/ReminderSettings.swift`. On Apple those are
//! the *default* for new tasks, chosen once in settings, and a task's own reminder is then a bare
//! instant with no way to say "fifteen minutes before" again later. The same list makes a better
//! per-task picker than a raw clock does — "an hour before" is what somebody means, and a date
//! picker makes them work out what that is.
//!
//! The arithmetic is here for the same reason the due picks are: an hour before a time is not the
//! same wall-clock answer across a daylight-saving boundary, and three clients each doing their own
//! subtraction is three chances to disagree.
//!
//! Titles are keys. The shell says the words.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::model::Task;

/// One choice, and the instant it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderPick {
    pub title_key: &'static str,
    /// The instant to store, or `None` for "no reminder".
    pub reminder_time: Option<String>,
    pub is_selected: bool,
}

/// How long before the due time, in minutes. `0` is "at the time it is due".
///
/// The order is the order Apple offers, and the clearing choice leads — the same decision the date
/// picks made, for the same reason: turning a reminder off is a choice, not an escape hatch.
pub const OFFSETS: [(&str, i64); 8] = [
    ("reminder.at_due_time", 0),
    ("reminder.5_minutes_before", 5),
    ("reminder.15_minutes_before", 15),
    ("reminder.30_minutes_before", 30),
    ("reminder.hour_before", 60),
    ("reminder.2_hours_before", 120),
    ("reminder.day_before", 1440),
    ("reminder.week_before", 10080),
];

/// The choices for a task.
///
/// A task with no due date has nothing to be "before", so it gets one choice — no reminder — and
/// whatever it already holds. Offering "an hour before" for a task with no when is offering to
/// compute something from nothing.
pub fn options(task: &Task, now: DateTime<Utc>) -> Vec<ReminderPick> {
    let current = task.reminder_time;
    let mut picks = vec![ReminderPick {
        title_key: "reminder.none",
        reminder_time: None,
        is_selected: current.is_none(),
    }];

    let Some(due) = task.due_date_time else {
        return picks;
    };

    for (title_key, minutes) in OFFSETS {
        let at = due - Duration::minutes(minutes);
        // A reminder in the past is one that has already happened. Offering it would store a time
        // the loop skips over, and the picker would show a choice that never arrives.
        if at <= now {
            continue;
        }
        picks.push(ReminderPick {
            title_key,
            reminder_time: Some(at.to_rfc3339()),
            is_selected: current == Some(at),
        });
    }
    picks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn at(text: &str) -> DateTime<Utc> {
        date::parse(text).expect("a date")
    }

    fn task(due: Option<&str>, reminder: Option<&str>) -> Task {
        Task {
            due_date_time: due.map(at),
            reminder_time: reminder.map(at),
            ..Task::new("t1", "Call the vet")
        }
    }

    fn keys(picks: &[ReminderPick]) -> Vec<&str> {
        picks.iter().map(|pick| pick.title_key).collect()
    }

    #[test]
    fn the_choices_are_offsets_from_the_due_time_with_clearing_first() {
        let picks = options(
            &task(Some("2026-09-20T09:00:00Z"), None),
            at("2026-09-07T12:00:00Z"),
        );
        assert_eq!(keys(&picks)[0], "reminder.none");
        assert_eq!(picks.len(), 9);
        let hour_before = picks
            .iter()
            .find(|pick| pick.title_key == "reminder.hour_before")
            .expect("an hour before");
        assert_eq!(
            hour_before.reminder_time.as_deref(),
            Some(at("2026-09-20T08:00:00Z").to_rfc3339().as_str())
        );
    }

    /// A task with no when has nothing to be "before".
    #[test]
    fn a_task_with_no_due_date_can_only_have_its_reminder_cleared() {
        let picks = options(&task(None, None), at("2026-09-07T12:00:00Z"));
        assert_eq!(keys(&picks), vec!["reminder.none"]);
    }

    /// A choice whose time has already passed would store a reminder the loop steps straight over.
    #[test]
    fn a_choice_already_in_the_past_is_not_offered() {
        let picks = options(
            &task(Some("2026-09-07T12:30:00Z"), None),
            at("2026-09-07T12:00:00Z"),
        );
        // Half an hour to go: "at the time", 5 and 15 minutes before are still ahead; 30 minutes
        // before is now, and everything longer is behind.
        assert_eq!(
            keys(&picks),
            vec![
                "reminder.none",
                "reminder.at_due_time",
                "reminder.5_minutes_before",
                "reminder.15_minutes_before",
            ]
        );
    }

    #[test]
    fn the_choice_the_task_already_holds_is_marked() {
        let picks = options(
            &task(Some("2026-09-20T09:00:00Z"), Some("2026-09-20T08:00:00Z")),
            at("2026-09-07T12:00:00Z"),
        );
        let selected: Vec<&str> = picks
            .iter()
            .filter(|pick| pick.is_selected)
            .map(|pick| pick.title_key)
            .collect();
        assert_eq!(selected, vec!["reminder.hour_before"]);
    }

    #[test]
    fn no_reminder_is_the_marked_choice_when_there_is_none() {
        let picks = options(
            &task(Some("2026-09-20T09:00:00Z"), None),
            at("2026-09-07T12:00:00Z"),
        );
        assert!(picks[0].is_selected);
    }
}
