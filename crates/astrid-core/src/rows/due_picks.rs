//! The quick date and time choices.
//!
//! Ported from `astrid-ios/Astrid App/Core/Layout/DueDateQuickPicks.swift` (task ea4f5124).
//!
//! iOS kept these as private arrays inside its date and time pickers. That was fine while iOS was
//! the only place they existed — but the Mac detail offered a bare picker with no quick choices at
//! all, and the obvious way to fix that is to retype the lists over there, which is how two
//! platforms come to disagree about what "Next week" means. So they live in one place, with the
//! arithmetic beside them, and this crate is the third client to read the same list.
//!
//! **The order is part of the contract.** The clearing choice comes first — "No due date" before
//! "Today" — because it is a choice like any other rather than an escape hatch in a toolbar. That
//! was decided deliberately on iOS; this is not the place to re-litigate it.
//!
//! Titles are keys, never words. The shell resolves them, the same way it resolves
//! [`super::DueLabel`].

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};

/// A quick date choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateOption {
    /// A resource key. Never a literal: these reach the screen on three platforms.
    pub title_key: &'static str,
    /// Whole days from today. `None` is the clearing choice.
    pub days_from_today: Option<i64>,
}

/// The choices, in order. Clearing first — see the module note.
pub const DATE_OPTIONS: [DateOption; 5] = [
    DateOption {
        title_key: "picker.no_due_date",
        days_from_today: None,
    },
    DateOption {
        title_key: "picker.today",
        days_from_today: Some(0),
    },
    DateOption {
        title_key: "picker.tomorrow",
        days_from_today: Some(1),
    },
    DateOption {
        title_key: "picker.in_3_days",
        days_from_today: Some(3),
    },
    DateOption {
        title_key: "picker.next_week",
        days_from_today: Some(7),
    },
];

/// A quick time choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeOption {
    pub title_key: &'static str,
    /// On a 24-hour clock.
    pub hour: u32,
}

pub const TIME_OPTIONS: [TimeOption; 4] = [
    TimeOption {
        title_key: "picker.morning",
        hour: 9,
    },
    TimeOption {
        title_key: "picker.afternoon",
        hour: 14,
    },
    TimeOption {
        title_key: "picker.evening",
        hour: 18,
    },
    TimeOption {
        title_key: "picker.night",
        hour: 21,
    },
];

/// The instant a quick date pick means, for an **all-day** task.
///
/// The reader's calendar day, `days` on, stored the way all-day dates are stored: midnight UTC.
/// Adding days to the day rather than seconds to the instant is what keeps "Tomorrow" on the right
/// date across a daylight-saving boundary, where a day is 23 or 25 hours.
pub fn all_day_pick(days: i64, now: DateTime<Utc>, offset: FixedOffset) -> DateTime<Utc> {
    let day = now.with_timezone(&offset).date_naive() + Duration::days(days);
    crate::model::date::all_day_instant(day)
}

/// The instant a quick date pick means for a **timed** task, keeping the time of day it already
/// had.
///
/// Picking a date must not silently discard a time the person already set — which is what
/// replacing the whole instant would do.
pub fn timed_pick(days: i64, current: DateTime<Utc>, offset: FixedOffset) -> DateTime<Utc> {
    let local = current.with_timezone(&offset);
    let day = local.date_naive() + Duration::days(days);
    to_utc(day, local.time(), offset)
}

/// Set the hour on a due date, zeroing minutes: "Morning" means 09:00, not 09:37.
///
/// In the reader's zone, because that is where "morning" means anything. A task set to the morning
/// in Auckland is not due at 09:00 UTC.
pub fn with_hour(hour: u32, current: DateTime<Utc>, offset: FixedOffset) -> DateTime<Utc> {
    let local = current.with_timezone(&offset);
    let time = chrono::NaiveTime::from_hms_opt(hour.min(23), 0, 0).expect("a valid hour");
    to_utc(local.date_naive(), time, offset)
}

fn to_utc(day: chrono::NaiveDate, time: chrono::NaiveTime, offset: FixedOffset) -> DateTime<Utc> {
    let local = day.and_time(time);
    offset
        .from_local_datetime(&local)
        .single()
        .map(|at| at.with_timezone(&Utc))
        // A fixed offset has no gaps, so this branch is unreachable; it keeps the function total
        // rather than panicking on something that cannot happen.
        .unwrap_or_else(|| Utc.from_utc_datetime(&local))
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

    /// The order is the contract, and clearing comes first. A change here is a change on three
    /// clients.
    #[test]
    fn the_choices_are_in_the_agreed_order_with_clearing_first() {
        let keys: Vec<&str> = DATE_OPTIONS.iter().map(|option| option.title_key).collect();
        assert_eq!(
            keys,
            vec![
                "picker.no_due_date",
                "picker.today",
                "picker.tomorrow",
                "picker.in_3_days",
                "picker.next_week"
            ]
        );
        assert_eq!(DATE_OPTIONS[0].days_from_today, None);
    }

    #[test]
    fn the_times_are_the_four_the_other_clients_offer() {
        let hours: Vec<u32> = TIME_OPTIONS.iter().map(|option| option.hour).collect();
        assert_eq!(hours, vec![9, 14, 18, 21]);
    }

    #[test]
    fn an_all_day_pick_lands_on_the_readers_day_at_midnight_utc() {
        let california = FixedOffset::east_opt(-7 * 3600).expect("an offset");
        // 22:00 on the 7th in California is 05:00 on the 8th in UTC.
        let evening = at("2026-09-08T05:00:00Z");

        assert_eq!(
            date::format(all_day_pick(0, evening, california)),
            "2026-09-07T00:00:00Z",
            "today is the 7th where the person is"
        );
        assert_eq!(
            date::format(all_day_pick(1, evening, california)),
            "2026-09-08T00:00:00Z"
        );
        assert_eq!(
            date::format(all_day_pick(7, evening, california)),
            "2026-09-14T00:00:00Z"
        );
    }

    /// Picking a date must not silently discard a time the person already set.
    #[test]
    fn a_timed_pick_keeps_the_time_of_day() {
        let current = at("2026-09-07T14:30:00Z");
        assert_eq!(
            date::format(timed_pick(1, current, utc())),
            "2026-09-08T14:30:00Z"
        );
    }

    /// Morning means 09:00 where the reader is. A task set to the morning in Auckland is not due
    /// at 09:00 UTC.
    #[test]
    fn a_time_pick_is_in_the_readers_zone_and_zeroes_the_minutes() {
        let auckland = FixedOffset::east_opt(12 * 3600).expect("an offset");
        let current = at("2026-09-07T02:37:00Z"); // 14:37 on the 7th in Auckland

        assert_eq!(
            date::format(with_hour(9, current, auckland)),
            "2026-09-06T21:00:00Z",
            "09:00 on the 7th in Auckland is 21:00 on the 6th in UTC"
        );
    }

    /// Day arithmetic goes through the calendar rather than adding seconds: across a
    /// daylight-saving boundary a day is 23 or 25 hours, and "Tomorrow" lands on the wrong date
    /// twice a year.
    #[test]
    fn adding_a_day_moves_the_calendar_day_not_a_fixed_number_of_seconds() {
        // The Sunday US clocks go forward in 2026.
        let california = FixedOffset::east_opt(-8 * 3600).expect("an offset");
        let before = at("2026-03-08T06:00:00Z"); // 22:00 on the 7th in California
        assert_eq!(
            all_day_pick(1, before, california).date_naive(),
            chrono::NaiveDate::from_ymd_opt(2026, 3, 8).expect("a real day")
        );
    }
}
