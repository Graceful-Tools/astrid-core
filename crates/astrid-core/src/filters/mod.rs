//! How a list's saved filters and sort setting turn every task into what is shown.
//!
//! Ported from `astrid-ios/Astrid App/Core/Filters/ListTaskFiltering.swift`, which is itself the
//! shared module iOS and Mac call so neither reimplements it. This crate is the third
//! implementation of these rules and the last one that should ever be written: the shell calls
//! these functions and renders the answer.
//!
//! Pure throughout. The current user id, the manual order, the clock and the reader's UTC offset
//! are all passed in, so every rule is a test rather than a screen somebody has to set up.
//!
//! ## One rule worth stating twice
//!
//! **An unrecognised filter value keeps everything.** These values are saved on the server and
//! synced between clients, so a build from six months ago will see values it has never heard of.
//! Treating an unknown value as "matches nothing" empties the list — the user's list, on their
//! screen, for no reason they can see. Every `match` here therefore ends in "keep it".
//!
//! The due-date filter has one gap in that rule, inherited rather than chosen: a task with no due
//! date is decided before the filter is read, so an unrecognised value hides undated tasks while
//! keeping dated ones. It is reproduced here rather than fixed, because fixing it on one client
//! makes three clients disagree about what a list contains. See `docs/CONTRACTS.md` D7.

pub mod my_tasks;
pub mod recently_completed;
pub mod subtasks;

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};

use crate::model::{Privacy, Task, TaskList};

/// Apply every saved filter on `list` to `tasks`, keeping what passes.
///
/// Borrowed, because the caller that matters — one list's rows, on every refresh — has ten
/// thousand tasks and no use for a second copy of them. [`filter_for_list`] is the owned version
/// for anything that does.
pub fn filter_refs<'a>(
    tasks: &'a [Task],
    list: &TaskList,
    current_user_id: Option<&str>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<&'a Task> {
    let completion = list.filter_completion.as_deref().unwrap_or("default");
    let window = list.recently_completed_window.as_ref();

    tasks
        .iter()
        .filter(|task| {
            // Completion first: it is the one that hides most of what it hides, and every other
            // filter is cheaper to skip than to run.
            if task.completed
                && !recently_completed::should_show_completed(
                    Some(completion),
                    task.completed_at,
                    task.updated_at,
                    window,
                    now,
                    offset,
                )
            {
                return false;
            }
            matches_priority(task, list.filter_priority.as_deref())
                && matches_due_date(task, list.filter_due_date.as_deref(), now, offset)
                && matches_assignee(task, list.filter_assignee.as_deref(), current_user_id)
                && matches_repeating(task, list.filter_repeating.as_deref())
                && matches_assigned_by(task, list.filter_assigned_by.as_deref(), current_user_id)
                && matches_in_lists(task, list.filter_in_lists.as_deref())
        })
        .collect()
}

/// The same, owned. One clone of everything that passed.
pub fn filter_for_list(
    tasks: &[Task],
    list: &TaskList,
    current_user_id: Option<&str>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<Task> {
    filter_refs(tasks, list, current_user_id, now, offset)
        .into_iter()
        .cloned()
        .collect()
}

fn matches_priority(task: &Task, filter: Option<&str>) -> bool {
    match filter {
        Some(value) if value != "all" => match value.parse::<i64>() {
            Ok(priority) => task.priority.as_i64() == priority,
            // A priority filter that is not a number is a value this build does not understand.
            Err(_) => true,
        },
        _ => true,
    }
}

/// The time-bound due-date buckets.
///
/// All-day tasks compare in UTC and timed tasks in the reader's local time, because that is what
/// each of them means: an all-day task is a calendar day (see [`crate::model::date`]) and a timed
/// one is an instant that belongs to the day the reader sees it on.
///
/// Overdue incomplete tasks surface in every forward-looking bucket. A "today" list that hid what
/// was due yesterday and never done is a list that helps you forget.
pub(crate) fn matches_due_date(
    task: &Task,
    filter: Option<&str>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> bool {
    let Some(filter) = filter.filter(|value| *value != "all") else {
        return true;
    };
    // Undated tasks are answered before the filter is examined, which is what makes an
    // unrecognised value hide them. Faithful to the Swift and web implementations; see the module
    // note and `docs/CONTRACTS.md` D7.
    let Some(due) = task.due_date_time else {
        return filter == "no_date";
    };
    if filter == "no_date" {
        return false;
    }

    let (today, due_day) = if task.is_all_day {
        (
            start_of_day_utc(now.with_timezone(&offset).date_naive()),
            start_of_day_utc(due.date_naive()),
        )
    } else {
        (start_of_day_at(now, offset), start_of_day_at(due, offset))
    };

    let overdue_incomplete = due_day < today && !task.completed;
    match filter {
        "overdue" => due_day < today && !task.completed,
        "today" => due_day == today || overdue_incomplete,
        "this_week" => {
            (due_day >= today && due_day <= today + Duration::days(7)) || overdue_incomplete
        }
        "this_month" => {
            (due_day >= today && due_day <= today + Duration::days(30)) || overdue_incomplete
        }
        _ => true,
    }
}

fn matches_assignee(task: &Task, filter: Option<&str>, current_user_id: Option<&str>) -> bool {
    let Some(filter) = filter.filter(|value| *value != "all") else {
        return true;
    };
    let assignee = task
        .assignee_id
        .as_deref()
        .or(task.assignee.as_ref().map(|user| user.id.as_str()));
    match filter {
        // Signed out, "assigned to me" matches nothing rather than everything. Showing every task
        // as if it were yours is the more alarming of the two failures.
        "current_user" => current_user_id.is_some() && assignee == current_user_id,
        "not_current_user" => assignee.is_some() && assignee != current_user_id,
        "unassigned" => assignee.is_none(),
        id => assignee == Some(id),
    }
}

/// The repeating filter.
///
/// It was persisted and synced by every client and applied by none, so the control did nothing
/// anywhere until this was written.
fn matches_repeating(task: &Task, filter: Option<&str>) -> bool {
    let Some(filter) = filter.filter(|value| *value != "all") else {
        return true;
    };
    let cadence = task
        .repeating
        .and_then(|repeating| serde_json::to_value(repeating).ok())
        .and_then(|value| value.as_str().map(str::to_string));
    match filter {
        "not_repeating" => !task.is_repeating(),
        "daily" | "weekly" | "monthly" | "yearly" | "custom" => cadence.as_deref() == Some(filter),
        _ => true,
    }
}

fn matches_assigned_by(task: &Task, filter: Option<&str>, current_user_id: Option<&str>) -> bool {
    let Some(filter) = filter.filter(|value| *value != "all") else {
        return true;
    };
    match filter {
        "current_user" => current_user_id.is_some_and(|id| task.is_created_by(id)),
        "not_current_user" => current_user_id.is_none_or(|id| !task.is_created_by(id)),
        id => task.is_created_by(id),
    }
}

fn matches_in_lists(task: &Task, filter: Option<&str>) -> bool {
    let Some(filter) = filter.filter(|value| *value != "dont_filter") else {
        return true;
    };
    let in_a_list = !task.effective_list_ids().is_empty();
    match filter {
        "not_in_list" => !in_a_list,
        "in_list" => in_a_list,
        "public_lists" => task
            .lists
            .iter()
            .flatten()
            .any(|list| list.privacy == Some(Privacy::Public)),
        _ => true,
    }
}

/// The sort orders a list can be set to.
///
/// Completed tasks sink to the bottom in the value-based orders: a list where a finished task sits
/// between two live ones reads as unsorted, whatever the setting says.
///
/// Generic over what the slice holds so a caller that has references — which the row pipeline does,
/// to avoid copying ten thousand tasks per refresh — sorts them without owning them first.
pub fn sort_by_setting<T: std::borrow::Borrow<Task>>(
    tasks: &mut [T],
    sort_by: Option<&str>,
    manual_order: Option<&[String]>,
) {
    let completed_last = |a: &T, b: &T| completed_last(a.borrow(), b.borrow());
    let due_date_order = |a: &T, b: &T| due_date_order(a.borrow(), b.borrow());
    let created_at_of = |task: &T| created_at_of(task.borrow());
    let priority_of = |task: &T| task.borrow().priority;
    let id_of = |task: &T| task.borrow().id.clone();
    match sort_by.unwrap_or("auto") {
        "priority" => tasks.sort_by(|a, b| {
            completed_last(a, b)
                .then_with(|| priority_of(b).cmp(&priority_of(a)))
                .then_with(|| due_date_order(a, b))
        }),
        "when" => tasks.sort_by(|a, b| {
            completed_last(a, b)
                .then_with(|| due_date_order(a, b))
                .then_with(|| priority_of(b).cmp(&priority_of(a)))
        }),
        "createdAt" => tasks.sort_by_key(|task| std::cmp::Reverse(created_at_of(task))),
        "manual" => match manual_order.filter(|order| !order.is_empty()) {
            Some(order) => {
                let position = |task: &T| order.iter().position(|id| id == &id_of(task));
                tasks.sort_by(|a, b| match (position(a), position(b)) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    // A task the saved order has never seen — created since, or on another
                    // device — goes after the arranged ones rather than to the top, where it
                    // would look like the arrangement had been lost.
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => created_at_of(b).cmp(&created_at_of(a)),
                })
            }
            // Set to manual with nothing arranged yet: newest first, the same as `createdAt`.
            None => tasks.sort_by_key(|task| std::cmp::Reverse(created_at_of(task))),
        },
        // "auto", and anything a newer build introduced.
        _ => tasks.sort_by(|a, b| {
            completed_last(a, b)
                .then_with(|| priority_of(b).cmp(&priority_of(a)))
                .then_with(|| due_date_order(a, b))
                .then_with(|| created_at_of(a).cmp(&created_at_of(b)))
        }),
    }
}

fn completed_last(a: &Task, b: &Task) -> std::cmp::Ordering {
    a.completed.cmp(&b.completed)
}

/// Earlier dates first; a task with no date sorts after every task that has one.
fn due_date_order(a: &Task, b: &Task) -> std::cmp::Ordering {
    match (a.due_date_time, b.due_date_time) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn created_at_of(task: &Task) -> DateTime<Utc> {
    task.created_at.unwrap_or(DateTime::<Utc>::MIN_UTC)
}

fn start_of_day_utc(date: chrono::NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight exists"))
}

fn start_of_day_at(instant: DateTime<Utc>, offset: FixedOffset) -> DateTime<Utc> {
    let local = instant.with_timezone(&offset);
    let midnight = local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists");
    offset
        .from_local_datetime(&midnight)
        .single()
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
}

/// The day part of an instant as the reader sees it. Used by anything that groups by day.
pub fn local_day(instant: DateTime<Utc>, offset: FixedOffset) -> chrono::NaiveDate {
    instant.with_timezone(&offset).date_naive()
}

/// Whether a task is overdue for the reader right now.
pub fn is_overdue(task: &Task, now: DateTime<Utc>, offset: FixedOffset) -> bool {
    if task.completed {
        return false;
    }
    let Some(due) = task.due_date_time else {
        return false;
    };
    if task.is_all_day {
        // An all-day task is late once the reader's day has moved past its day — not at midnight
        // UTC, which is mid-afternoon for a third of the world.
        return crate::model::date::all_day_date(due) < local_day(now, offset);
    }
    due < now
}

/// Is this task due on the reader's today?
pub fn is_due_today(task: &Task, now: DateTime<Utc>, offset: FixedOffset) -> bool {
    let Some(due) = task.due_date_time else {
        return false;
    };
    let today = local_day(now, offset);
    if task.is_all_day {
        crate::model::date::all_day_date(due) == today
    } else {
        local_day(due, offset) == today
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{date, Priority, Repeating};

    fn at(instant: &str) -> DateTime<Utc> {
        date::parse(instant).expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("UTC")
    }

    fn now() -> DateTime<Utc> {
        at("2026-09-07T12:00:00Z")
    }

    fn list(filters: serde_json::Value) -> TaskList {
        let mut value = serde_json::json!({ "id": "l1", "name": "Home" });
        for (key, filter) in filters.as_object().expect("an object") {
            value[key] = filter.clone();
        }
        serde_json::from_value(value).expect("decodes")
    }

    fn task(id: &str) -> Task {
        Task::new(id, format!("Task {id}"))
    }

    fn ids(tasks: &[Task]) -> Vec<&str> {
        tasks.iter().map(|task| task.id.as_str()).collect()
    }

    // ── Completion ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_default_completion_filter_hides_what_was_finished_yesterday() {
        let mut fresh = task("fresh");
        fresh.completed = true;
        fresh.completed_at = Some(at("2026-09-07T11:00:00Z"));
        let mut stale = task("stale");
        stale.completed = true;
        stale.completed_at = Some(at("2026-09-01T11:00:00Z"));

        let filtered = filter_for_list(
            &[task("open"), fresh, stale],
            &list(serde_json::json!({})),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&filtered), vec!["open", "fresh"]);
    }

    #[test]
    fn hide_and_show_do_what_they_say() {
        let mut done = task("done");
        done.completed = true;
        done.completed_at = Some(at("2020-01-01T00:00:00Z"));

        let hidden = filter_for_list(
            std::slice::from_ref(&done),
            &list(serde_json::json!({ "filterCompletion": "hide" })),
            None,
            now(),
            utc(),
        );
        assert!(hidden.is_empty());

        let shown = filter_for_list(
            std::slice::from_ref(&done),
            &list(serde_json::json!({ "filterCompletion": "show" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(shown.len(), 1);
    }

    // ── The rule that keeps lists from emptying ──────────────────────────────────────────────

    /// These values are synced between clients. A build that has never heard of one must show
    /// everything rather than nothing — the alternative is somebody's list going blank for a
    /// reason they cannot see.
    #[test]
    fn a_filter_value_this_build_does_not_know_keeps_everything() {
        let tasks = vec![task("a"), task("b")];
        for filter in [
            serde_json::json!({ "filterPriority": "somethingLater" }),
            serde_json::json!({ "filterRepeating": "somethingLater" }),
            serde_json::json!({ "filterInLists": "somethingLater" }),
            serde_json::json!({ "filterCompletion": "somethingLater" }),
            serde_json::json!({ "filterAssignee": "somebody-elses-id" }),
        ] {
            let filtered = filter_for_list(&tasks, &list(filter.clone()), None, now(), utc());
            assert_eq!(
                filtered.len(),
                if filter.get("filterAssignee").is_some() {
                    0
                } else {
                    2
                },
                "for {filter}"
            );
        }
    }

    /// The one gap in that rule, and it is inherited: the due-date filter decides undated tasks
    /// before it reads the filter value, so an unrecognised value keeps dated tasks and hides
    /// undated ones. Reproduced rather than fixed — see `docs/CONTRACTS.md` D7.
    #[test]
    fn an_unknown_due_date_filter_keeps_dated_tasks_and_drops_undated_ones() {
        let mut dated = task("dated");
        dated.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        let filtered = filter_for_list(
            &[dated, task("undated")],
            &list(serde_json::json!({ "filterDueDate": "somethingLater" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&filtered), vec!["dated"]);
    }

    // ── Priority ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_priority_filter_matches_the_number_the_server_stores() {
        let mut high = task("high");
        high.priority = Priority::High;
        let filtered = filter_for_list(
            &[task("none"), high],
            &list(serde_json::json!({ "filterPriority": "3" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&filtered), vec!["high"]);
    }

    // ── Due date ─────────────────────────────────────────────────────────────────────────────

    /// A "today" list that hides what was due yesterday and never done is a list that helps you
    /// forget.
    #[test]
    fn overdue_work_surfaces_in_every_forward_looking_bucket() {
        let mut overdue = task("overdue");
        overdue.due_date_time = Some(at("2026-09-01T12:00:00Z"));
        let mut today = task("today");
        today.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        let mut later = task("later");
        later.due_date_time = Some(at("2026-10-30T12:00:00Z"));

        for bucket in ["today", "this_week", "this_month"] {
            let filtered = filter_for_list(
                &[overdue.clone(), today.clone(), later.clone()],
                &list(serde_json::json!({ "filterDueDate": bucket })),
                None,
                now(),
                utc(),
            );
            assert!(ids(&filtered).contains(&"overdue"), "for {bucket}");
        }
    }

    /// A completed task that is past its date is not overdue — it is done.
    #[test]
    fn a_finished_task_is_never_overdue() {
        let mut done = task("done");
        done.completed = true;
        done.due_date_time = Some(at("2026-09-01T12:00:00Z"));
        let filtered = filter_for_list(
            &[done],
            &list(serde_json::json!({ "filterDueDate": "overdue", "filterCompletion": "show" })),
            None,
            now(),
            utc(),
        );
        assert!(filtered.is_empty());
    }

    #[test]
    fn no_date_selects_exactly_the_tasks_with_no_date() {
        let mut dated = task("dated");
        dated.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        let filtered = filter_for_list(
            &[task("undated"), dated],
            &list(serde_json::json!({ "filterDueDate": "no_date" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&filtered), vec!["undated"]);
    }

    /// A timed task at 23:00 on the 7th in UTC-5 is due today for its reader, and tomorrow for
    /// UTC. The list has to agree with their calendar.
    #[test]
    fn a_timed_task_belongs_to_the_readers_day() {
        let new_york = FixedOffset::east_opt(-5 * 3600).expect("an offset");
        let mut tonight = task("tonight");
        // 23:00 on the 7th in New York is 04:00 on the 8th in UTC.
        tonight.is_all_day = false;
        tonight.due_date_time = Some(at("2026-09-08T04:00:00Z"));

        // 18:00 on the 7th in New York.
        let evening = at("2026-09-07T23:00:00Z");
        let filtered = filter_for_list(
            &[tonight],
            &list(serde_json::json!({ "filterDueDate": "today" })),
            None,
            evening,
            new_york,
        );
        assert_eq!(ids(&filtered), vec!["tonight"]);
    }

    // ── Assignee ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_assignee_filters_read_both_shapes_the_assignee_arrives_in() {
        let mut mine = task("mine");
        mine.assignee_id = Some("me".into());
        let embedded: Task =
            serde_json::from_str(r#"{"id":"also-mine","assignee":{"id":"me"}}"#).expect("decodes");
        let mut theirs = task("theirs");
        theirs.assignee_id = Some("them".into());

        let tasks = vec![mine, embedded, theirs, task("unassigned")];
        let filtered = filter_for_list(
            &tasks,
            &list(serde_json::json!({ "filterAssignee": "current_user" })),
            Some("me"),
            now(),
            utc(),
        );
        assert_eq!(ids(&filtered), vec!["mine", "also-mine"]);

        let others = filter_for_list(
            &tasks,
            &list(serde_json::json!({ "filterAssignee": "not_current_user" })),
            Some("me"),
            now(),
            utc(),
        );
        assert_eq!(ids(&others), vec!["theirs"]);

        let unassigned = filter_for_list(
            &tasks,
            &list(serde_json::json!({ "filterAssignee": "unassigned" })),
            Some("me"),
            now(),
            utc(),
        );
        assert_eq!(ids(&unassigned), vec!["unassigned"]);
    }

    /// Signed out, "assigned to me" matches nothing. Showing every task as if it were yours is the
    /// more alarming of the two ways to be wrong.
    #[test]
    fn assigned_to_me_with_nobody_signed_in_matches_nothing() {
        let mut assigned = task("assigned");
        assigned.assignee_id = Some("someone".into());
        let filtered = filter_for_list(
            &[assigned],
            &list(serde_json::json!({ "filterAssignee": "current_user" })),
            None,
            now(),
            utc(),
        );
        assert!(filtered.is_empty());
    }

    // ── Repeating ────────────────────────────────────────────────────────────────────────────

    /// Saved and synced by every client, applied by none — the control did nothing anywhere.
    #[test]
    fn the_repeating_filter_finally_does_something() {
        let mut daily = task("daily");
        daily.repeating = Some(Repeating::Daily);
        let mut never = task("never");
        never.repeating = Some(Repeating::Never);
        let tasks = vec![daily, never, task("plain")];

        let repeating = filter_for_list(
            &tasks,
            &list(serde_json::json!({ "filterRepeating": "daily" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&repeating), vec!["daily"]);

        let one_offs = filter_for_list(
            &tasks,
            &list(serde_json::json!({ "filterRepeating": "not_repeating" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&one_offs), vec!["never", "plain"]);
    }

    // ── In lists ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_in_lists_filter_powers_the_virtual_lists() {
        let mut filed = task("filed");
        filed.list_ids = Some(vec!["l1".into()]);
        let tasks = vec![filed, task("loose")];

        let loose = filter_for_list(
            &tasks,
            &list(serde_json::json!({ "filterInLists": "not_in_list" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&loose), vec!["loose"]);

        let filed_only = filter_for_list(
            &tasks,
            &list(serde_json::json!({ "filterInLists": "in_list" })),
            None,
            now(),
            utc(),
        );
        assert_eq!(ids(&filed_only), vec!["filed"]);
    }

    // ── Sorting ──────────────────────────────────────────────────────────────────────────────

    /// A finished task sitting between two live ones reads as unsorted, whatever the setting says.
    #[test]
    fn completed_tasks_sink_in_the_value_based_orders() {
        let mut done = task("done");
        done.completed = true;
        done.priority = Priority::High;
        let mut open = task("open");
        open.priority = Priority::Low;

        for order in ["auto", "priority", "when"] {
            let mut tasks = vec![done.clone(), open.clone()];
            sort_by_setting(&mut tasks, Some(order), None);
            assert_eq!(ids(&tasks), vec!["open", "done"], "for {order}");
        }
    }

    #[test]
    fn auto_is_priority_then_due_date_then_age() {
        let mut high = task("high");
        high.priority = Priority::High;
        let mut soon = task("soon");
        soon.due_date_time = Some(at("2026-09-08T12:00:00Z"));
        let mut later = task("later");
        later.due_date_time = Some(at("2026-09-30T12:00:00Z"));

        let mut tasks = vec![later, soon, high];
        sort_by_setting(&mut tasks, Some("auto"), None);
        assert_eq!(ids(&tasks), vec!["high", "soon", "later"]);
    }

    /// A task with no date sorts after every task that has one, in every order that considers
    /// dates. Undated work at the top of a "when" list is the complaint this prevents.
    #[test]
    fn an_undated_task_sorts_after_every_dated_one() {
        let mut dated = task("dated");
        dated.due_date_time = Some(at("2026-12-31T12:00:00Z"));
        let mut tasks = vec![task("undated"), dated];
        sort_by_setting(&mut tasks, Some("when"), None);
        assert_eq!(ids(&tasks), vec!["dated", "undated"]);
    }

    /// A task created since the arrangement was saved goes after the arranged ones. Putting it
    /// first would look like the arrangement had been lost.
    #[test]
    fn manual_order_puts_unarranged_tasks_after_the_arranged_ones() {
        let order = vec!["b".to_string(), "a".to_string()];
        let mut new_one = task("new");
        new_one.created_at = Some(at("2026-09-07T12:00:00Z"));
        let mut tasks = vec![task("a"), new_one, task("b")];
        sort_by_setting(&mut tasks, Some("manual"), Some(&order));
        assert_eq!(ids(&tasks), vec!["b", "a", "new"]);
    }

    #[test]
    fn manual_with_nothing_arranged_yet_is_newest_first() {
        let mut old = task("old");
        old.created_at = Some(at("2026-01-01T12:00:00Z"));
        let mut recent = task("recent");
        recent.created_at = Some(at("2026-09-07T12:00:00Z"));
        let mut tasks = vec![old, recent];
        sort_by_setting(&mut tasks, Some("manual"), None);
        assert_eq!(ids(&tasks), vec!["recent", "old"]);
    }

    /// A sort order from a newer build falls back to auto rather than leaving the list in whatever
    /// order the database happened to return.
    #[test]
    fn an_unknown_sort_order_falls_back_to_auto() {
        let mut high = task("high");
        high.priority = Priority::High;
        let mut tasks = vec![task("none"), high];
        sort_by_setting(&mut tasks, Some("somethingLater"), None);
        assert_eq!(ids(&tasks), vec!["high", "none"]);
    }

    // ── Overdue and today ────────────────────────────────────────────────────────────────────

    /// An all-day task is late once the reader's day has moved past its day. Comparing instants
    /// would make it overdue at noon UTC, which is breakfast in California.
    #[test]
    fn an_all_day_task_is_overdue_by_the_readers_calendar() {
        let california = FixedOffset::east_opt(-7 * 3600).expect("an offset");
        let mut task = task("t1");
        task.is_all_day = true;
        task.due_date_time = Some(date::all_day_instant(
            chrono::NaiveDate::from_ymd_opt(2026, 9, 7).expect("a real day"),
        ));

        // 08:00 on the 7th in California: due today, not overdue, even though the anchor instant
        // (noon UTC) has already passed.
        let breakfast = at("2026-09-07T15:00:00Z");
        assert!(!is_overdue(&task, breakfast, california));
        assert!(is_due_today(&task, breakfast, california));

        // The next morning, it is overdue.
        assert!(is_overdue(&task, at("2026-09-08T15:00:00Z"), california));
    }
}
