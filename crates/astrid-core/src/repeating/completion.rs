//! What completing a task does to it — the whole rule, as a value.
//!
//! [`crate::services::TaskService::complete`] applies it to the cache and the Outbox. A shell that
//! still keeps its own write path — the Apple apps, until they drive this crate's services — asks
//! for it through [`crate::rules`] and applies the same answer to its own store. Either way there
//! is one place that decides whether a completion rolls a series forward, ends it, or just sets
//! the flag, and it is this one.
//!
//! The date math is [`super`]'s; this module only chooses between the outcomes and says what each
//! one changes.

use chrono::{DateTime, FixedOffset, Utc};
use serde::Serialize;

use super::{
    calculate_custom_next_occurrence, calculate_simple_next_occurrence, pattern_from_wire,
    NextOccurrence, RepeatFrom, Repeating, SimplePatternEndCondition,
};
use crate::model::{date, RepeatFromMode, Repeating as WireRepeating, Task};

/// The three things a completion can do.
///
/// Tagged by `outcome` on the wire so a shell switches on a name rather than inferring the case
/// from which fields are present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Completion {
    /// No rollover: the flag, and when it happened, is the whole change.
    ///
    /// `clear_closed_reason` is set when a task is reopened: a task that is open again is not
    /// "won't do" either, and the web's reopen clears both (task 016ce981).
    Toggle {
        completed: bool,
        #[serde(with = "date::optional")]
        completed_at: Option<DateTime<Utc>>,
        clear_closed_reason: bool,
    },
    /// A repeating task rolls forward: it stays open, due next time round, one occurrence on.
    RollForward {
        #[serde(with = "date::required")]
        due_date_time: DateTime<Utc>,
        is_all_day: bool,
        occurrence_count: i32,
    },
    /// The series has reached its end condition: completed, and no longer repeating — a task that
    /// showed a repeat chip after its last occurrence would look like it was coming back.
    SeriesEnded {
        #[serde(with = "date::required")]
        completed_at: DateTime<Utc>,
    },
}

/// Decide what marking `task` completed (or not) at `now` does.
///
/// `task` is the task as the person sees it — a view that let them edit the due date or the
/// repeat first must pass its edited copy (rule 3 of the README), because the rollover anchors on
/// those fields. `offset` is the device's offset from UTC: an all-day task repeating from its
/// completion anchors on the person's own calendar day, not UTC's.
pub fn completion(
    task: &Task,
    completed: bool,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Completion {
    // Un-completing, completing something already completed, or a task that does not repeat: the
    // flag is the whole operation. An already-completed repeating task does not roll again.
    if !completed || task.completed || !task.is_repeating() {
        return Completion::Toggle {
            completed,
            completed_at: completed.then_some(now),
            clear_closed_reason: !completed && task.closed_reason.is_some(),
        };
    }

    let outcome = next_occurrence(task, now, offset);
    match outcome.next_due_date {
        Some(due_date_time) => Completion::RollForward {
            due_date_time,
            is_all_day: task.is_all_day,
            occurrence_count: outcome.new_occurrence_count,
        },
        None => Completion::SeriesEnded { completed_at: now },
    }
}

/// Where a repeating task goes next, read off the task's own fields.
///
/// Delegates to the calculators and does no pattern math of its own. An inline copy in the iOS
/// service once ignored `weekdays`, so a Mon/Wed/Fri task jumped a whole week instead of moving to
/// the next selected day — rule 3 of the README exists because of it.
pub fn next_occurrence(task: &Task, now: DateTime<Utc>, offset: FixedOffset) -> NextOccurrence {
    let repeat_from = match task.repeat_from.unwrap_or(RepeatFromMode::CompletionDate) {
        RepeatFromMode::DueDate => RepeatFrom::DueDate,
        RepeatFromMode::CompletionDate => RepeatFrom::CompletionDate,
    };
    let completion = effective_completion_date(task, repeat_from, now, offset);
    let occurrence_count = task.occurrence_count.unwrap_or(0) as i32;

    if task.repeating == Some(WireRepeating::Custom) {
        if let Some(pattern) = &task.repeating_data {
            return calculate_custom_next_occurrence(
                &pattern_from_wire(pattern),
                task.due_date_time,
                completion,
                repeat_from,
                occurrence_count,
            );
        }
    }

    // A simple pattern may still carry an end condition, piggybacked on the same column.
    let end_data = task.repeating_data.as_ref().and_then(|pattern| {
        let wire = pattern_from_wire(pattern);
        wire.end_condition
            .map(|end_condition| SimplePatternEndCondition {
                end_condition,
                end_after_occurrences: wire.end_after_occurrences,
                end_until_date: wire.end_until_date,
            })
    });

    let simple = match task.repeating {
        Some(WireRepeating::Daily) => Repeating::Daily,
        Some(WireRepeating::Weekly) => Repeating::Weekly,
        Some(WireRepeating::Monthly) => Repeating::Monthly,
        Some(WireRepeating::Yearly) => Repeating::Yearly,
        _ => Repeating::Never,
    };
    calculate_simple_next_occurrence(
        simple,
        task.due_date_time,
        completion,
        repeat_from,
        occurrence_count,
        end_data.as_ref(),
    )
}

/// The instant a completion anchors on.
///
/// For an all-day task repeating from its completion, the anchor is the calendar day the person
/// completed it on — **their** day, stored the way all-day dates are stored. Ticking one off at
/// 21:00 in California is 04:00 UTC the next day; anchoring on that instant would move every
/// evening completion a day past their own calendar, and the next occurrence with it.
fn effective_completion_date(
    task: &Task,
    repeat_from: RepeatFrom,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> DateTime<Utc> {
    if task.is_all_day && repeat_from == RepeatFrom::CompletionDate {
        return date::all_day_today(now, offset);
    }
    now
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(instant: &str) -> DateTime<Utc> {
        date::parse(instant).expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("UTC")
    }

    fn task(json: serde_json::Value) -> Task {
        serde_json::from_value(json).expect("a task in the wire shape")
    }

    #[test]
    fn a_one_off_task_is_just_marked_done_at_the_moment_it_happened() {
        let one_off = task(serde_json::json!({ "id": "t1", "title": "Buy milk" }));
        let now = at("2026-09-28T12:00:00Z");
        assert_eq!(
            completion(&one_off, true, now, utc()),
            Completion::Toggle {
                completed: true,
                completed_at: Some(now),
                clear_closed_reason: false
            }
        );
    }

    #[test]
    fn a_daily_task_rolls_forward_and_counts_the_occurrence() {
        let daily = task(serde_json::json!({
            "id": "t1", "repeating": "daily", "repeatFrom": "DUE_DATE",
            "dueDateTime": "2026-09-28T09:00:00Z", "isAllDay": false, "occurrenceCount": 2
        }));
        assert_eq!(
            completion(&daily, true, at("2026-09-28T12:00:00Z"), utc()),
            Completion::RollForward {
                due_date_time: at("2026-09-29T09:00:00Z"),
                is_all_day: false,
                occurrence_count: 3
            }
        );
    }

    /// The Apple apps rolled a series forward without counting, so "end after 3 times" never
    /// ended there. Counting is part of the rule, not an afterthought of one client's service.
    #[test]
    fn a_series_with_an_occurrence_limit_ends_on_its_last_completion() {
        let limited = task(serde_json::json!({
            "id": "t1", "repeating": "daily", "repeatFrom": "DUE_DATE",
            "dueDateTime": "2026-09-28T09:00:00Z", "occurrenceCount": 2,
            "repeatingData": { "endCondition": "after_occurrences", "endAfterOccurrences": 3 }
        }));
        let now = at("2026-09-28T12:00:00Z");
        assert_eq!(
            completion(&limited, true, now, utc()),
            Completion::SeriesEnded { completed_at: now }
        );
    }

    #[test]
    fn reopening_a_wont_do_task_clears_its_reason() {
        let closed = task(serde_json::json!({
            "id": "t1", "completed": true, "closedReason": "wont_do"
        }));
        assert_eq!(
            completion(&closed, false, at("2026-09-28T12:00:00Z"), utc()),
            Completion::Toggle {
                completed: false,
                completed_at: None,
                clear_closed_reason: true
            }
        );
    }

    #[test]
    fn an_already_completed_repeating_task_does_not_roll_again() {
        let done = task(serde_json::json!({
            "id": "t1", "repeating": "daily", "completed": true,
            "dueDateTime": "2026-09-28T09:00:00Z"
        }));
        assert!(matches!(
            completion(&done, true, at("2026-09-28T12:00:00Z"), utc()),
            Completion::Toggle {
                completed: true,
                ..
            }
        ));
    }

    /// Completing an all-day daily task at 21:00 in California is 04:00 UTC the next day. The
    /// anchor is the person's day, so the next occurrence is the day after *their* today.
    #[test]
    fn an_all_day_completion_anchors_on_the_persons_own_day() {
        let all_day = task(serde_json::json!({
            "id": "t1", "repeating": "daily", "repeatFrom": "COMPLETION_DATE",
            "dueDateTime": "2026-09-28T00:00:00Z", "isAllDay": true
        }));
        let pacific = FixedOffset::west_opt(7 * 3600).expect("PDT");
        assert_eq!(
            completion(&all_day, true, at("2026-09-29T04:00:00Z"), pacific),
            Completion::RollForward {
                due_date_time: at("2026-09-29T00:00:00Z"),
                is_all_day: true,
                occurrence_count: 1
            }
        );
    }

    #[test]
    fn the_outcome_is_named_on_the_wire() {
        let json = serde_json::to_value(Completion::RollForward {
            due_date_time: at("2026-09-29T09:00:00Z"),
            is_all_day: false,
            occurrence_count: 1,
        })
        .expect("serialises");
        assert_eq!(
            json,
            serde_json::json!({
                "outcome": "rollForward",
                "dueDateTime": "2026-09-29T09:00:00Z",
                "isAllDay": false,
                "occurrenceCount": 1
            })
        );
    }
}
