//! How far back "Recently completed" reaches.
//!
//! Ported from `astrid-ios/Astrid App/Models/RecentlyCompletedWindow.swift`, which mirrors
//! `astrid-web/lib/recently-completed-window.ts`. The same setting drives the list view's default
//! completion filter and the board's Done column, so the two can never disagree about what
//! "recently" means — which they did, before this was one function.
//!
//! A list with no window falls back to 24 hours, the behaviour every list had before the setting
//! existed.

use chrono::{DateTime, Datelike, Duration, FixedOffset, TimeZone, Utc};

use crate::model::{DurationUnit, RecentlyCompletedWindow};

/// The legacy default: completed within the last day.
pub const DEFAULT_WINDOW_HOURS: i64 = 24;

/// The instant before which completed tasks are hidden.
///
/// `offset` is the reader's own offset from UTC — the day-boundary windows are about *their*
/// calendar, and computing them in UTC would move the boundary by a working day for anyone far
/// enough east or west.
pub fn cutoff(
    window: Option<&RecentlyCompletedWindow>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> DateTime<Utc> {
    let Some(window) = window else {
        return now - Duration::hours(DEFAULT_WINDOW_HOURS);
    };

    match window {
        RecentlyCompletedWindow::Duration { amount, unit } => {
            let seconds = match unit {
                DurationUnit::Hour => 3_600,
                DurationUnit::Day => 86_400,
                DurationUnit::Week => 604_800,
                // Thirty days, the same approximation web makes. A calendar month here and
                // thirty days there would put the two views a day or two apart every month.
                DurationUnit::Month => 30 * 86_400,
            };
            now - Duration::seconds(amount.saturating_mul(seconds))
        }
        RecentlyCompletedWindow::SinceWeekday { weekday } => {
            // 0 = Sunday, matching web.
            let local = now.with_timezone(&offset);
            let today = local.weekday().num_days_from_sunday() as i64;
            let target = weekday.rem_euclid(7);
            let back = (today - target).rem_euclid(7);
            midnight_local(local.date_naive(), offset) - Duration::days(back)
        }
        RecentlyCompletedWindow::SinceDayOfMonth { day } => {
            let local = now.with_timezone(&offset);
            let day = (*day).clamp(1, 31) as u32;
            let (year, month) = if (local.day() as i64) < day as i64 {
                // Before that day this month: the most recent occurrence was last month.
                previous_month(local.year(), local.month())
            } else {
                (local.year(), local.month())
            };
            // A day the month does not have clamps to its last, so "the 31st" still means
            // something in February rather than nothing at all.
            let day = day.min(days_in_month(year, month));
            match chrono::NaiveDate::from_ymd_opt(year, month, day) {
                Some(date) => midnight_local(date, offset),
                None => now,
            }
        }
        RecentlyCompletedWindow::SinceDate { date } => {
            match chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") {
                Ok(date) => midnight_local(date, offset),
                // A date nobody can read is not a reason to hide everything, nor to show
                // everything: fall back to "now", which hides only what was completed before this
                // moment and is the least surprising of the three.
                Err(_) => now,
            }
        }
    }
}

/// Whether a completed task falls inside the window.
///
/// `updated_at` stands in when there is no `completed_at`: tasks completed before the column
/// existed have only the one timestamp, and treating them as never-recently-completed would empty
/// the section for anybody with an old account.
pub fn is_recently_completed(
    completed_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    window: Option<&RecentlyCompletedWindow>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> bool {
    match completed_at.or(updated_at) {
        Some(at) => at >= cutoff(window, now, offset),
        None => false,
    }
}

/// Whether a completed task should be shown under a given filter mode.
///
/// `default` applies the window; `show` and `all` always show; `hide` never does. An unrecognised
/// mode shows — a filter value from a newer build must not blank out somebody's list.
pub fn should_show_completed(
    filter_mode: Option<&str>,
    completed_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    window: Option<&RecentlyCompletedWindow>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> bool {
    match filter_mode {
        Some("hide") => false,
        Some("default") => is_recently_completed(completed_at, updated_at, window, now, offset),
        _ => true,
    }
}

fn midnight_local(date: chrono::NaiveDate, offset: FixedOffset) -> DateTime<Utc> {
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is a valid time");
    offset
        .from_local_datetime(&midnight)
        .single()
        // A fixed offset has no gaps, so `single` always resolves; the fallback keeps the function
        // total rather than panicking on an impossible branch.
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
}

fn previous_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_of_next =
        chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("a real month");
    first_of_next
        .pred_opt()
        .expect("every month has a last day")
        .day()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn at(instant: &str) -> DateTime<Utc> {
        date::parse(instant).expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("UTC")
    }

    /// Every list had this behaviour before the setting existed, and most still have no setting.
    #[test]
    fn no_window_means_the_last_twenty_four_hours() {
        let now = at("2026-09-07T12:00:00Z");
        assert_eq!(cutoff(None, now, utc()), at("2026-09-06T12:00:00Z"));
    }

    #[test]
    fn a_duration_window_counts_back_from_now() {
        let now = at("2026-09-07T12:00:00Z");
        let cases = [
            (DurationUnit::Hour, 3, "2026-09-07T09:00:00Z"),
            (DurationUnit::Day, 2, "2026-09-05T12:00:00Z"),
            (DurationUnit::Week, 1, "2026-08-31T12:00:00Z"),
            // Thirty days, matching web's approximation.
            (DurationUnit::Month, 1, "2026-08-08T12:00:00Z"),
        ];
        for (unit, amount, expected) in cases {
            let window = RecentlyCompletedWindow::Duration { amount, unit };
            assert_eq!(cutoff(Some(&window), now, utc()), at(expected), "{unit:?}");
        }
    }

    /// "Since Monday" on a Wednesday reaches back two days, to local midnight.
    #[test]
    fn a_weekday_window_reaches_back_to_that_days_midnight() {
        // 2026-09-09 is a Wednesday.
        let now = at("2026-09-09T15:00:00Z");
        let monday = RecentlyCompletedWindow::SinceWeekday { weekday: 1 };
        assert_eq!(
            cutoff(Some(&monday), now, utc()),
            at("2026-09-07T00:00:00Z")
        );
    }

    /// Asked on the day itself, it reaches back to this morning rather than a week.
    #[test]
    fn a_weekday_window_on_that_very_day_means_this_morning() {
        let now = at("2026-09-07T15:00:00Z"); // a Monday
        let monday = RecentlyCompletedWindow::SinceWeekday { weekday: 1 };
        assert_eq!(
            cutoff(Some(&monday), now, utc()),
            at("2026-09-07T00:00:00Z")
        );
    }

    /// The reader's calendar, not UTC's. In Auckland it is already the 8th when UTC says the 7th,
    /// and a window computed in UTC would be a day out for them all afternoon.
    #[test]
    fn day_boundaries_are_the_readers_own() {
        let now = at("2026-09-07T20:00:00Z"); // 08:00 on the 8th in UTC+12
        let auckland = FixedOffset::east_opt(12 * 3600).expect("an offset");
        let window = RecentlyCompletedWindow::SinceDayOfMonth { day: 8 };
        assert_eq!(
            cutoff(Some(&window), now, auckland),
            at("2026-09-07T12:00:00Z"),
            "midnight on the 8th in Auckland is noon on the 7th in UTC"
        );
    }

    #[test]
    fn a_day_of_month_window_walks_back_a_month_when_it_has_not_come_round_yet() {
        let now = at("2026-09-07T12:00:00Z");
        let fifteenth = RecentlyCompletedWindow::SinceDayOfMonth { day: 15 };
        assert_eq!(
            cutoff(Some(&fifteenth), now, utc()),
            at("2026-08-15T00:00:00Z")
        );
    }

    /// "The 31st" in a month that has thirty days still has to mean something.
    #[test]
    fn a_day_the_month_does_not_have_clamps_to_its_last() {
        let now = at("2026-03-10T12:00:00Z");
        let thirty_first = RecentlyCompletedWindow::SinceDayOfMonth { day: 31 };
        assert_eq!(
            cutoff(Some(&thirty_first), now, utc()),
            at("2026-02-28T00:00:00Z")
        );
    }

    #[test]
    fn a_fixed_date_window_is_that_days_local_midnight() {
        let now = at("2026-09-07T12:00:00Z");
        let window = RecentlyCompletedWindow::SinceDate {
            date: "2026-01-01".into(),
        };
        assert_eq!(
            cutoff(Some(&window), now, utc()),
            at("2026-01-01T00:00:00Z")
        );
    }

    /// Tasks completed before the column existed carry only `updatedAt`. Ignoring it would empty
    /// the section for anyone with an old account.
    #[test]
    fn a_task_with_no_completion_time_falls_back_to_when_it_was_last_touched() {
        let now = at("2026-09-07T12:00:00Z");
        assert!(is_recently_completed(
            None,
            Some(at("2026-09-07T11:00:00Z")),
            None,
            now,
            utc()
        ));
        assert!(!is_recently_completed(
            None,
            Some(at("2026-09-01T11:00:00Z")),
            None,
            now,
            utc()
        ));
        assert!(!is_recently_completed(None, None, None, now, utc()));
    }

    #[test]
    fn the_filter_modes_do_what_they_say() {
        let now = at("2026-09-07T12:00:00Z");
        let old = Some(at("2026-01-01T12:00:00Z"));
        let show = |mode: Option<&str>| should_show_completed(mode, old, None, None, now, utc());
        assert!(show(Some("show")));
        assert!(show(Some("all")));
        assert!(show(None));
        assert!(!show(Some("hide")));
        assert!(!show(Some("default")), "old, so outside the window");
    }

    /// A filter value from a newer build must not blank out somebody's list.
    #[test]
    fn an_unrecognised_filter_mode_shows_rather_than_hides() {
        let now = at("2026-09-07T12:00:00Z");
        assert!(should_show_completed(
            Some("somethingLater"),
            Some(at("2020-01-01T12:00:00Z")),
            None,
            None,
            now,
            utc()
        ));
    }
}
