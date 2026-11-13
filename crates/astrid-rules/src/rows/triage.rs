//! What a reminder puts in front of somebody (AATD-54) — iOS's `Tasks/ReminderTriage`, so every
//! client triages the same tasks in the same order.
//!
//! Mine, open, and due today or already overdue, soonest first — then the task the reminder
//! actually fired for, floated to the front so the notification lands on what it was about.

use chrono::{DateTime, FixedOffset, Utc};

use crate::filters;
use crate::model::Task;
use crate::rows::day_offset;

/// The deck, in the order it is dealt.
///
/// `tasks` is everything the caller has; this picks from it rather than fetching, so the App door
/// and the stateless door run one copy of the decision.
///
/// `reminded_task_id` is the task the notification fired for. It goes first even when it is not
/// due — that is the point of a reminder — but it is **not** forced in when it is completed or
/// belongs to somebody else: a done task is nothing to triage, and someone else's is theirs.
pub fn deck<'a>(
    tasks: &'a [Task],
    me: &str,
    reminded_task_id: Option<&str>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<&'a Task> {
    let due_now = |task: &Task| {
        task.due_date_time
            .is_some_and(|due| day_offset(due, task.is_all_day, now, offset) == 0)
            || filters::is_overdue(task, now, offset)
    };
    let mut deck: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.assignee_id.as_deref() == Some(me) && !task.completed && due_now(task))
        .collect();
    deck.sort_by_key(|task| task.due_date_time);
    if let Some(reminded) = reminded_task_id
        .and_then(|id| tasks.iter().find(|task| task.id == id))
        .filter(|task| !task.completed)
        // Somebody else's task is theirs to triage, not mine.
        .filter(|task| {
            task.assignee_id
                .as_deref()
                .is_none_or(|assignee| assignee == me)
        })
    {
        deck.retain(|task| task.id != reminded.id);
        deck.insert(0, reminded);
    }
    deck
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn task(id: &str, assignee: Option<&str>, due: Option<&str>) -> Task {
        let mut task = Task::new(id, id);
        task.assignee_id = assignee.map(str::to_string);
        task.due_date_time = due.map(|due| {
            DateTime::parse_from_rfc3339(due)
                .expect("an RFC 3339 date")
                .with_timezone(&Utc)
        });
        task.is_all_day = true;
        task
    }

    fn ids(deck: &[&Task]) -> Vec<String> {
        deck.iter().map(|task| task.id.clone()).collect()
    }

    /// Task `75a27f7f`: the deck is what is due for ME — never somebody else's, never a done one,
    /// and never one that is not due yet.
    #[test]
    fn task_75a27f7f_the_deck_is_my_open_tasks_due_today_or_overdue() {
        let mut done = task("done", Some("me"), Some("2000-01-01T00:00:00Z"));
        done.completed = true;
        let tasks = [
            task("late", Some("me"), Some("2000-01-01T00:00:00Z")),
            task("theirs", Some("bo"), Some("2000-01-01T00:00:00Z")),
            done,
            task("later", Some("me"), Some("2099-01-01T00:00:00Z")),
            task("today", Some("me"), Some("2026-06-15T00:00:00Z")),
            task("undated", Some("me"), None),
        ];
        assert_eq!(
            ids(&deck(&tasks, "me", None, now(), utc())),
            vec!["late", "today"]
        );
    }

    /// Task `75a27f7f`: soonest first.
    #[test]
    fn task_75a27f7f_the_deck_is_ordered_by_due_date() {
        let tasks = [
            task("later", Some("me"), Some("2026-06-15T00:00:00Z")),
            task("earlier", Some("me"), Some("2020-01-01T00:00:00Z")),
        ];
        assert_eq!(
            ids(&deck(&tasks, "me", None, now(), utc())),
            vec!["earlier", "later"]
        );
    }

    /// Task `75a27f7f`: the task the reminder fired for goes first even though it is not due, and
    /// is not repeated further down the deck.
    #[test]
    fn task_75a27f7f_the_reminded_task_leads_even_when_it_is_not_due() {
        let tasks = [
            task("late", Some("me"), Some("2000-01-01T00:00:00Z")),
            task("pinged", Some("me"), Some("2099-06-01T00:00:00Z")),
        ];
        assert_eq!(
            ids(&deck(&tasks, "me", Some("pinged"), now(), utc())),
            vec!["pinged", "late"]
        );

        // Already in the deck on its own merit: floated, not duplicated.
        assert_eq!(
            ids(&deck(&tasks, "me", Some("late"), now(), utc())),
            vec!["late"]
        );
    }

    /// Task `75a27f7f`: a completed or somebody else's reminded task is not forced in. An
    /// unassigned one is, since nobody else owns it.
    #[test]
    fn task_75a27f7f_a_done_or_foreign_reminded_task_is_not_forced_in() {
        let mut done = task("done", Some("me"), Some("2099-01-01T00:00:00Z"));
        done.completed = true;
        let tasks = [
            task("late", Some("me"), Some("2000-01-01T00:00:00Z")),
            done,
            task("theirs", Some("bo"), Some("2099-01-01T00:00:00Z")),
            task("nobodys", None, Some("2099-01-01T00:00:00Z")),
        ];
        assert_eq!(
            ids(&deck(&tasks, "me", Some("done"), now(), utc())),
            vec!["late"]
        );
        assert_eq!(
            ids(&deck(&tasks, "me", Some("theirs"), now(), utc())),
            vec!["late"]
        );
        assert_eq!(
            ids(&deck(&tasks, "me", Some("nobodys"), now(), utc())),
            vec!["nobodys", "late"]
        );
        // A reminded id nobody sent is not an error: the deck is still the deck.
        assert_eq!(
            ids(&deck(&tasks, "me", Some("gone"), now(), utc())),
            vec!["late"]
        );
    }

    /// Task `75a27f7f`: "due today" is the READER's today. West of UTC, a task dated tomorrow in
    /// UTC is still tomorrow — the offset decides, not the server's clock.
    #[test]
    fn task_75a27f7f_due_today_is_the_readers_today() {
        let tasks = [task("tonight", Some("me"), Some("2026-06-16T00:00:00Z"))];
        let behind = FixedOffset::east_opt(-8 * 3600).unwrap();
        assert!(deck(&tasks, "me", None, now(), behind).is_empty());
        assert_eq!(
            ids(&deck(
                &tasks,
                "me",
                None,
                Utc.with_ymd_and_hms(2026, 6, 16, 12, 0, 0).unwrap(),
                behind,
            )),
            vec!["tonight"]
        );
    }
}
