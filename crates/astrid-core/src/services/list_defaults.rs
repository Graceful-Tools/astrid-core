//! What a new task looks like in a list that has said so (task c4102c67).
//!
//! A list's admin tab on the web carries defaults for the tasks added to it — assignee, priority,
//! repeat, *When* and *When Time* — and the web's quick-add applies them
//! (`lib/task-creation-utils.ts`, `applyListDefaults`). Windows quick-add sent whatever was
//! typed and nothing else, so a list set up as *High, assigned to me, due today* behaved
//! differently depending on which client added the task.
//!
//! The rule lives here rather than in the shell because of rule 4 in `docs/ASTRID.md`: the
//! shell must not decide what a new task looks like. It is applied at the door, to whatever the
//! caller did NOT say — a priority typed into quick-add wins over the list's default, exactly as
//! the web's `taskData.priority ?? targetList.defaultPriority` reads.
//!
//! One deliberate difference from the web, recorded in `docs/CONTRACTS.md` as D10: a *When*
//! default with no *When Time* makes an **all-day** task here. The web stamps the creation
//! instant on it — a "due today" default yields a task due at 14:37 — because its
//! `parseRelativeDate("today")` is `new Date()` and `isAllDay` falls to `false`. That is the
//! bug the task itself warns about ("an all-day default is `all_day_instant`, not a local
//! midnight"), so this client stores the calendar day.

use chrono::{DateTime, Duration, FixedOffset, NaiveTime, Utc};

use super::TaskDraft;
use crate::model::{Priority, Repeating, TaskList};
use crate::rows::due_picks;

/// Which fields the caller actually said something about. A default never overrides a choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Given {
    pub priority: bool,
    pub due: bool,
    pub assignee: bool,
    pub repeating: bool,
    pub is_private: bool,
}

/// Fill in what the list defaults and the caller left unsaid.
///
/// `me` is the signed-in user's id, which is the fallback assignee for a list that has named no
/// default — see the assignee arm below. `None` when nobody is signed in yet.
pub fn apply(
    draft: &mut TaskDraft,
    given: Given,
    list: &TaskList,
    me: Option<&str>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) {
    if !given.priority {
        if let Some(priority) = list.default_priority {
            draft.priority = Priority::from_i64(priority);
        }
    }

    if !given.repeating {
        if let Some(repeating) = list.default_repeating.as_deref() {
            draft.repeating = match repeating {
                "daily" => Some(Repeating::Daily),
                "weekly" => Some(Repeating::Weekly),
                "monthly" => Some(Repeating::Monthly),
                "yearly" => Some(Repeating::Yearly),
                // "never", and "custom" — which carries no pattern to repeat by.
                _ => None,
            };
        }
    }

    if !given.is_private {
        if let Some(private) = list.default_is_private {
            draft.is_private = private;
        }
    }

    if !given.assignee {
        match list.default_assignee_id.as_deref() {
            // Unset means the task's creator — `resolveAssignee` in astrid-web's
            // `services/assignee-authorization.ts`: `if (listDefault === null) return actorId`.
            //
            // This used to be a no-op that left the server to fill it in. The server does, but
            // deferring showed: the optimistic row drawn the instant somebody hits + was
            // unassigned, so its leading control was the unassigned mark instead of a checkbox
            // until a sync came back — and offline, never (task e2505d10). Resolving it here makes
            // the local answer the same as the server's, with no network, for every client.
            //
            // The empty string is folded in with "unset", as it was before: it is not a user id,
            // and no list's admin tab can produce one.
            None | Some("") => draft.assignee_id = me.map(str::to_string),
            // A list that says `unassigned` means it, and that is not the same as saying nothing.
            Some("unassigned") => draft.assignee_id = None,
            Some(id) => draft.assignee_id = Some(id.to_string()),
        }
    }

    if !given.due {
        let days = days_for(list.default_due_date.as_deref());
        let time = list.default_due_time.as_deref().and_then(parse_time);
        match (days, time) {
            (Some(days), Some(time)) => {
                draft.due_date_time = Some(at(days, time, now, offset));
                draft.is_all_day = false;
            }
            (Some(days), None) => {
                draft.due_date_time = Some(due_picks::all_day_pick(days, now, offset));
                draft.is_all_day = true;
            }
            // A time with no day: the web makes it today at that time.
            (None, Some(time)) => {
                draft.due_date_time = Some(at(0, time, now, offset));
                draft.is_all_day = false;
            }
            (None, None) => {}
        }
    }
}

/// The *When* default as days from today, or `None` for no date.
fn days_for(default_due_date: Option<&str>) -> Option<i64> {
    match default_due_date.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "today" => Some(0),
        Some(value) if value == "tomorrow" => Some(1),
        Some(value) if value == "next_week" || value == "next week" => Some(7),
        _ => None,
    }
}

/// `HH:MM`, as the list stores its time default.
fn parse_time(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M").ok()
}

/// The reader's calendar day `days` on, at `time` in the reader's zone.
fn at(days: i64, time: NaiveTime, now: DateTime<Utc>, offset: FixedOffset) -> DateTime<Utc> {
    let day = now.with_timezone(&offset).date_naive() + Duration::days(days);
    day.and_time(time)
        .and_local_timezone(offset)
        .single()
        .map(|at| at.with_timezone(&Utc))
        // The hour a daylight-saving jump skips: the day itself rather than no task.
        .unwrap_or_else(|| crate::model::date::all_day_instant(day))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn california() -> FixedOffset {
        FixedOffset::west_opt(7 * 3600).expect("an offset")
    }

    fn noon_utc() -> DateTime<Utc> {
        "2026-09-07T12:00:00Z".parse().expect("a time")
    }

    fn list_with_defaults() -> TaskList {
        let mut list = TaskList::new("l1", "Work");
        list.default_priority = Some(3);
        list.default_repeating = Some("weekly".into());
        list.default_assignee_id = Some("dana".into());
        list.default_due_date = Some("tomorrow".into());
        list.default_due_time = Some("17:00".into());
        list
    }

    /// A list set up as high, weekly, Dana's, due tomorrow at five gives quick-add exactly that
    /// (task c4102c67).
    #[test]
    fn a_task_takes_every_default_its_list_has_task_c4102c67() {
        let mut draft = TaskDraft::new("Buy milk");

        apply(
            &mut draft,
            Given::default(),
            &list_with_defaults(),
            Some("me"),
            noon_utc(),
            california(),
        );

        assert_eq!(draft.priority, Priority::High);
        assert_eq!(draft.repeating, Some(Repeating::Weekly));
        assert_eq!(draft.assignee_id.as_deref(), Some("dana"));
        // Tomorrow in California is Sep 8; 17:00 there is 00:00 UTC on the 9th.
        assert_eq!(
            draft.due_date_time.map(|at| at.to_rfc3339()),
            Some("2026-09-09T00:00:00+00:00".into())
        );
        assert!(!draft.is_all_day);
    }

    /// What the person typed wins over what the list would have given.
    #[test]
    fn a_default_never_overrides_what_was_said() {
        let mut draft = TaskDraft::new("Buy milk");
        draft.priority = Priority::Low;
        draft.assignee_id = Some("me".into());
        let chosen_due = "2026-10-01T09:00:00Z".parse().expect("a time");
        draft.due_date_time = Some(chosen_due);
        draft.is_all_day = false;

        apply(
            &mut draft,
            Given {
                priority: true,
                due: true,
                assignee: true,
                repeating: false,
                is_private: false,
            },
            &list_with_defaults(),
            Some("me"),
            noon_utc(),
            california(),
        );

        assert_eq!(draft.priority, Priority::Low);
        assert_eq!(draft.assignee_id.as_deref(), Some("me"));
        assert_eq!(draft.due_date_time, Some(chosen_due));
        assert_eq!(
            draft.repeating,
            Some(Repeating::Weekly),
            "not said, so the list's"
        );
    }

    /// A date default without a time is an all-day task on that calendar day — stored as the
    /// day, which reads back as the same day west of UTC (D10 against the web's creation stamp).
    #[test]
    fn a_when_default_without_a_time_is_all_day_on_the_reader_s_day() {
        let mut list = TaskList::new("l1", "Work");
        list.default_due_date = Some("today".into());
        let mut draft = TaskDraft::new("Buy milk");

        apply(
            &mut draft,
            Given::default(),
            &list,
            Some("me"),
            noon_utc(),
            california(),
        );

        assert_eq!(
            draft.due_date_time.map(|at| at.to_rfc3339()),
            Some("2026-09-07T00:00:00+00:00".into())
        );
        assert!(draft.is_all_day);
    }

    /// A time default with no date is today at that time, as the web does it.
    #[test]
    fn a_time_default_alone_is_today_at_that_time() {
        let mut list = TaskList::new("l1", "Work");
        list.default_due_time = Some("09:30".into());
        let mut draft = TaskDraft::new("Buy milk");

        apply(
            &mut draft,
            Given::default(),
            &list,
            Some("me"),
            noon_utc(),
            california(),
        );

        assert_eq!(
            draft.due_date_time.map(|at| at.to_rfc3339()),
            Some("2026-09-07T16:30:00+00:00".into())
        );
        assert!(!draft.is_all_day);
    }

    #[test]
    fn unassigned_clears_and_unset_means_the_creator() {
        let mut list = TaskList::new("l1", "Work");
        list.default_assignee_id = Some("unassigned".into());
        let mut draft = TaskDraft::new("Buy milk");
        draft.assignee_id = Some("carried".into());
        apply(
            &mut draft,
            Given::default(),
            &list,
            Some("me"),
            noon_utc(),
            california(),
        );
        assert_eq!(
            draft.assignee_id, None,
            "a list that says `unassigned` means it — that is not the same as saying nothing"
        );

        let untouched = TaskList::new("l2", "Home");
        let mut draft = TaskDraft::new("Buy milk");
        apply(
            &mut draft,
            Given::default(),
            &untouched,
            Some("me"),
            noon_utc(),
            california(),
        );
        assert_eq!(
            draft.assignee_id.as_deref(),
            Some("me"),
            "a list with no default assigns the creator — `resolveAssignee`'s \
             `if (listDefault === null) return actorId`"
        );
        assert_eq!(draft.priority, Priority::None);
        assert_eq!(draft.due_date_time, None);
        assert!(draft.is_all_day);
    }

    /// The bug behind task `e2505d10`: *"new tasks in windows don't have a checkbox on list view"*.
    ///
    /// A list with no default assignee used to leave the draft unassigned and let the SERVER fill
    /// the creator in. The server does do that — `resolveAssignee` returns `actorId` for a null
    /// list default — but deferring means the optimistic row this client draws immediately is
    /// unassigned, so its leading control is the unassigned mark rather than a checkbox, and stays
    /// that way until a sync brings the server's answer back. Offline it never arrives at all.
    ///
    /// So the rule is resolved HERE, where every client gets it and it holds with no network.
    #[test]
    fn a_new_task_in_a_list_with_no_default_is_mine_task_e2505d10() {
        let mut draft = TaskDraft::new("Buy milk");

        apply(
            &mut draft,
            Given::default(),
            &TaskList::new("l1", "Work"),
            Some("jon"),
            noon_utc(),
            california(),
        );

        assert_eq!(
            draft.assignee_id.as_deref(),
            Some("jon"),
            "a task I just made in a list with no default is mine, so the row draws a checkbox"
        );
    }

    /// Signed out, or before the account is known, there is no creator to assign. Unassigned is the
    /// honest answer — and naming a user id that is not the reader's would draw somebody else's
    /// avatar on the row.
    #[test]
    fn with_no_known_creator_the_task_stays_unassigned() {
        let mut draft = TaskDraft::new("Buy milk");

        apply(
            &mut draft,
            Given::default(),
            &TaskList::new("l1", "Work"),
            None,
            noon_utc(),
            california(),
        );

        assert_eq!(draft.assignee_id, None);
    }

    /// A list default still wins over the creator, and a stated assignee still wins over both.
    #[test]
    fn the_creator_is_only_the_fallback() {
        let mut list = TaskList::new("l1", "Work");
        list.default_assignee_id = Some("dana".into());

        let mut draft = TaskDraft::new("Buy milk");
        apply(
            &mut draft,
            Given::default(),
            &list,
            Some("jon"),
            noon_utc(),
            california(),
        );
        assert_eq!(
            draft.assignee_id.as_deref(),
            Some("dana"),
            "the list said who, so not the creator"
        );

        let mut draft = TaskDraft::new("Buy milk");
        draft.assignee_id = Some("sam".into());
        apply(
            &mut draft,
            Given {
                assignee: true,
                ..Given::default()
            },
            &list,
            Some("jon"),
            noon_utc(),
            california(),
        );
        assert_eq!(
            draft.assignee_id.as_deref(),
            Some("sam"),
            "somebody was picked, so neither the list nor the creator"
        );
    }

    #[test]
    fn next_week_is_seven_days_and_none_is_no_date() {
        let mut list = TaskList::new("l1", "Work");
        list.default_due_date = Some("next_week".into());
        let mut draft = TaskDraft::new("Buy milk");
        apply(
            &mut draft,
            Given::default(),
            &list,
            Some("me"),
            noon_utc(),
            california(),
        );
        assert_eq!(
            draft.due_date_time.map(|at| at.to_rfc3339()),
            Some("2026-09-14T00:00:00+00:00".into())
        );

        list.default_due_date = Some("none".into());
        let mut draft = TaskDraft::new("Buy milk");
        apply(
            &mut draft,
            Given::default(),
            &list,
            Some("me"),
            noon_utc(),
            california(),
        );
        assert_eq!(draft.due_date_time, None);
    }

    #[test]
    fn a_custom_repeat_default_carries_no_pattern_and_so_does_not_repeat() {
        let mut list = TaskList::new("l1", "Work");
        list.default_repeating = Some("custom".into());
        let mut draft = TaskDraft::new("Buy milk");
        apply(
            &mut draft,
            Given::default(),
            &list,
            Some("me"),
            noon_utc(),
            california(),
        );
        assert_eq!(draft.repeating, None);
    }
}
