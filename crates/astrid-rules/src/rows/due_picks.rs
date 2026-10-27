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
//! **The set and order are iOS's** (`DueDateQuickPicks.dateOptions`): Today, Tomorrow, In 3 days,
//! Next week — and no "No due date", which iOS does not offer among the quick picks
//! (`docs/CONTRACTS.md` D26, resolved toward iOS 2026-10-03). Clearing a date is still a write the
//! shell can make (`dueDateTime: null`); it is just not one of these rows.
//!
//! Storage is unchanged by that decision: iOS's `setQuickDate` writes the reader's calendar day at
//! UTC midnight, exactly as [`all_day_pick`] does.
//!
//! Titles are keys, never words. The shell resolves them, the same way it resolves
//! [`super::DueLabel`].

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

/// A quick date choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateOption {
    /// A resource key. Never a literal: these reach the screen on three platforms.
    pub title_key: &'static str,
    /// Whole days from today. 0 is today.
    pub days_from_today: i64,
}

/// The choices, in iOS's order — see the module note.
pub const DATE_OPTIONS: [DateOption; 4] = [
    DateOption {
        title_key: "picker.today",
        days_from_today: 0,
    },
    DateOption {
        title_key: "picker.tomorrow",
        days_from_today: 1,
    },
    DateOption {
        title_key: "picker.in_3_days",
        days_from_today: 3,
    },
    DateOption {
        title_key: "picker.next_week",
        days_from_today: 7,
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
pub fn all_day_pick<Z: TimeZone + Copy>(days: i64, now: DateTime<Utc>, zone: Z) -> DateTime<Utc> {
    let day = now.with_timezone(&zone).date_naive() + Duration::days(days);
    crate::model::date::all_day_instant(day)
}

/// The instant a quick date pick means for a **timed** task, keeping the time of day it already
/// had.
///
/// Picking a date must not silently discard a time the person already set — which is what
/// replacing the whole instant would do.
pub fn timed_pick<Z: TimeZone + Copy>(days: i64, current: DateTime<Utc>, zone: Z) -> DateTime<Utc> {
    let local = current.with_timezone(&zone);
    let day = local.date_naive() + Duration::days(days);
    to_utc(day, local.time(), zone)
}

/// Set the hour on a due date, zeroing minutes: "Morning" means 09:00, not 09:37.
///
/// In the reader's zone, because that is where "morning" means anything. A task set to the morning
/// in Auckland is not due at 09:00 UTC.
/// The instant a chosen calendar day means for this task.
///
/// A calendar hands back a DAY. What that day means depends on the task, and getting it wrong is
/// the bug this exists to prevent twice over:
///
/// - An **all-day** task stores a calendar date, so the day is written as one. Reading it back
///   anywhere west of UTC must give the same date, which is what [`crate::model::date`] is for.
/// - A **timed** task keeps its time of day. Picking "the 14th" on a task due at 17:00 means
///   the 14th at 17:00, not the 14th at midnight — silently discarding a time somebody set is
///   the same mistake the quick picks already avoid.
///
/// A timed task with no date yet has no time to keep, so it falls back to the day itself.
pub fn on_day<Z: TimeZone + Copy>(
    day: NaiveDate,
    current: Option<DateTime<Utc>>,
    is_all_day: bool,
    zone: Z,
) -> DateTime<Utc> {
    if is_all_day {
        return crate::model::date::all_day_instant(day);
    }
    let Some(current) = current else {
        return crate::model::date::all_day_instant(day);
    };
    let time = current.with_timezone(&zone).time();
    to_utc(day, time, zone)
}

pub fn with_hour<Z: TimeZone + Copy>(hour: u32, current: DateTime<Utc>, zone: Z) -> DateTime<Utc> {
    with_time(hour, 0, current, zone)
}

/// Taking the time off a timed due date, keeping the day the reader sees it on (task 0616f2d1).
///
/// The task's **local** day, re-expressed as UTC midnight — the way an all-day date is stored.
/// Neither of the two obvious shortcuts works:
///
/// - Keeping the instant and flipping `isAllDay` moves the date. Nothing normalises the pair —
///   not `TaskChanges::apply`, not web's PATCH route — and every all-day consumer reads the
///   **UTC** day through [`crate::model::date::all_day_date`]. A task due 20:00 in California is
///   `03:00Z` the next day, so it would read back as tomorrow; one due 07:00 in Tokyo, as
///   yesterday.
/// - Local midnight is the mistake web makes today (`setHours(0,0,0,0)`), and the one the Apple
///   apps made before `docs/CONTRACTS.md` D12.
///
/// This is `push_due`'s timed branch (`external::decisions`), which has pushed the same day to
/// Google all along.
///
/// **Takes a timed instant.** An instant that is already an all-day date would go backwards west
/// of UTC, where UTC midnight reads as the previous afternoon; [`time_anchor`] is what turns an
/// all-day task into a valid input, and the caller that has an `is_all_day` flag should simply
/// leave such a task alone.
pub fn to_all_day<Z: TimeZone + Copy>(due: DateTime<Utc>, zone: Z) -> DateTime<Utc> {
    crate::model::date::all_day_instant(due.with_timezone(&zone).date_naive())
}

/// As [`with_hour`], keeping a minute too — what a time picker hands back (AATD-25).
pub fn with_time<Z: TimeZone + Copy>(
    hour: u32,
    minute: u32,
    current: DateTime<Utc>,
    zone: Z,
) -> DateTime<Utc> {
    let local = current.with_timezone(&zone);
    let time = NaiveTime::from_hms_opt(hour.min(23), minute.min(59), 0).expect("a valid time");
    to_utc(local.date_naive(), time, zone)
}

/// The instant whose local day a chosen time goes on: an all-day task's calendar date (noon of
/// it, so the local reading of its UTC midnight — the day before, west of UTC — never wins), a
/// timed task's own instant, or now when there is no date.
pub fn time_anchor<Z: TimeZone + Copy>(
    due: Option<DateTime<Utc>>,
    is_all_day: bool,
    now: DateTime<Utc>,
    zone: Z,
) -> DateTime<Utc> {
    match due {
        Some(due) if is_all_day => {
            let noon = NaiveTime::from_hms_opt(12, 0, 0).expect("noon");
            to_utc(crate::model::date::all_day_date(due), noon, zone)
        }
        Some(due) => due,
        None => now,
    }
}

/// A local time on a calendar day, as an instant — with that day's offset in `zone` (AITD-561).
/// For callers outside the picks that set a time on a day, such as a list's default due time.
pub fn local_time_on<Z: TimeZone>(day: NaiveDate, time: NaiveTime, zone: Z) -> DateTime<Utc> {
    to_utc(day, time, zone)
}

fn to_utc<Z: TimeZone>(day: NaiveDate, time: NaiveTime, zone: Z) -> DateTime<Utc> {
    let local = day.and_time(time);
    resolve(local, &zone)
        // Every zone chrono-tz knows has a reading within a few hours of any local time, so this
        // is unreachable; it keeps the function total rather than panicking.
        .unwrap_or_else(|| Utc.from_utc_datetime(&local))
}

/// A local time on its own day, in a named zone (AITD-561).
///
/// With **that day's** offset rather than today's: "09:00 on Sunday" read on the Tuesday before
/// US clocks go back is 09:00 PST, and today's PDT would put it at 08:00. A zone, not a
/// `FixedOffset`, is what knows the difference; a `FixedOffset` has no changes and reads as before.
///
/// The hour a change repeats takes its first reading — the one the clock shows first. The hour
/// it skips does not exist, so the time is read with the offset in force just before the gap,
/// which moves it on by the gap's length (02:30 on a spring-forward night is 03:30), as
/// Foundation and JavaScript's `Date` do, rather than the choice being dropped.
fn resolve<Z: TimeZone>(local: NaiveDateTime, zone: &Z) -> Option<DateTime<Utc>> {
    if let Some(at) = zone.from_local_datetime(&local).earliest() {
        return Some(at.with_timezone(&Utc));
    }
    (1..=6)
        .map(|half_hours| Duration::minutes(30 * half_hours))
        .find_map(|back| {
            let before = zone.from_local_datetime(&(local - back)).earliest()?;
            Some(before.with_timezone(&Utc) + back)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;
    use chrono::FixedOffset;

    fn california() -> FixedOffset {
        FixedOffset::east_opt(-7 * 3600).expect("an offset")
    }

    fn day(year: i32, month: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, d).expect("a real day")
    }

    /// AATD-25. A time picker hands back an hour AND a minute; both are kept.
    #[test]
    fn aatd_25_a_chosen_time_keeps_its_minutes_on_the_tasks_day() {
        // A timed task on the 14th at 17:00 California, moved to 08:45 — still the 14th.
        let current = date::parse("2026-12-15T01:00:00Z").expect("a date");
        let picked = with_time(8, 45, current, california());
        assert_eq!(date::format(picked), "2026-12-14T15:45:00Z");
    }

    /// The day a time goes on: an all-day task's calendar date (not the local reading of its UTC
    /// midnight, which is the day before west of UTC), a timed task's own day, or today.
    #[test]
    fn aatd_25_the_time_goes_on_the_tasks_own_day() {
        let now = date::parse("2026-12-01T18:00:00Z").expect("a date");
        let christmas = date::all_day_instant(day(2026, 12, 25));
        let anchor = time_anchor(Some(christmas), true, now, california());
        assert_eq!(
            date::format(with_time(14, 30, anchor, california())),
            "2026-12-25T21:30:00Z"
        );
        let anchor = time_anchor(None, false, now, california());
        assert_eq!(
            date::format(with_time(14, 30, anchor, california())),
            "2026-12-01T21:30:00Z"
        );
    }

    /// An all-day task stores a calendar date, and it has to read back as the same date west of
    /// UTC — which is the whole reason `date::all_day_instant` exists.
    #[test]
    fn an_all_day_task_takes_the_day_itself() {
        let picked = on_day(day(2026, 12, 25), None, true, california());
        assert_eq!(date::all_day_date(picked), day(2026, 12, 25));
        assert_eq!(date::format(picked), "2026-12-25T00:00:00Z");
    }

    /// Picking "the 14th" on a task due at 17:00 means the 14th at 17:00. Discarding the time
    /// silently is the mistake the quick picks already avoid.
    #[test]
    fn a_timed_task_keeps_its_time_of_day() {
        // 17:00 in California on the 7th.
        let current = date::parse("2026-09-08T00:00:00Z").expect("an instant");
        let picked = on_day(day(2026, 9, 14), Some(current), false, california());

        assert_eq!(
            picked
                .with_timezone(&california())
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2026-09-14 17:00",
        );
    }

    /// A timed task with no date has no time to keep, so the day itself is the honest answer
    /// rather than midnight-in-some-zone.
    #[test]
    fn a_timed_task_with_no_date_takes_the_day() {
        let picked = on_day(day(2026, 9, 14), None, false, california());
        assert_eq!(date::all_day_date(picked), day(2026, 9, 14));
    }

    /// Task 0616f2d1. Taking the time off a task keeps the day the reader sees it on — which is
    /// their LOCAL day re-expressed as UTC midnight, not the raw instant with a flag flipped.
    /// A task due 20:00 in California is 03:00Z the next day; written back unchanged it reads as
    /// tomorrow, because every all-day consumer goes through `date::all_day_date`, which reads UTC.
    #[test]
    fn task_0616f2d1_clearing_a_time_west_of_utc_keeps_the_readers_day() {
        // 20:00 on the 14th in California is 03:00 on the 15th in UTC.
        let evening = at("2026-12-15T03:00:00Z");
        assert_eq!(
            date::format(to_all_day(evening, california())),
            "2026-12-14T00:00:00Z"
        );
        assert_eq!(
            date::all_day_date(to_all_day(evening, california())),
            day(2026, 12, 14),
            "and it reads back as the same day it was shown on"
        );
    }

    /// The mirror image, east of UTC: a task due 07:00 in Tokyo is the previous day in UTC, and
    /// keeping the raw instant would move it back a day.
    #[test]
    fn task_0616f2d1_clearing_a_time_east_of_utc_keeps_the_readers_day() {
        let tokyo = FixedOffset::east_opt(9 * 3600).expect("an offset");
        // 07:00 on the 15th in Tokyo is 22:00 on the 14th in UTC.
        let morning = at("2026-12-14T22:00:00Z");
        assert_eq!(
            date::format(to_all_day(morning, tokyo)),
            "2026-12-15T00:00:00Z"
        );
    }

    /// The hazard the doc comment names, pinned so nobody "simplifies" the caller's guard away:
    /// handed an instant that is ALREADY an all-day date, this walks the date backwards west of
    /// UTC, because UTC midnight reads as the previous afternoon there. Only a timed instant goes
    /// in — [`time_anchor`] is what turns an all-day task into one.
    #[test]
    fn task_0616f2d1_an_all_day_instant_is_not_a_valid_input() {
        let christmas = date::all_day_instant(day(2026, 12, 25));
        assert_eq!(
            date::all_day_date(to_all_day(christmas, california())),
            day(2026, 12, 24),
            "which is why the caller must not pass one"
        );
        // Through the anchor, which is how the command reaches it, the day is kept.
        let anchor = time_anchor(
            Some(christmas),
            true,
            at("2026-12-01T18:00:00Z"),
            california(),
        );
        assert_eq!(
            date::format(to_all_day(anchor, california())),
            "2026-12-25T00:00:00Z"
        );
    }

    /// The reader's zone decides, not the server's.
    #[test]
    fn the_time_is_kept_in_the_readers_zone() {
        let current = date::parse("2026-09-08T09:30:00Z").expect("an instant");
        let picked = on_day(day(2026, 9, 20), Some(current), false, utc());
        assert_eq!(
            picked.with_timezone(&utc()).format("%H:%M").to_string(),
            "09:30",
        );
    }

    fn at(instant: &str) -> DateTime<Utc> {
        date::parse(instant).expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("UTC")
    }

    /// D26, resolved toward iOS (`DueDateQuickPicks.dateOptions`): four dates and no clearing
    /// choice. Storage is unchanged — iOS's `setQuickDate` also writes the reader's day at UTC
    /// midnight.
    #[test]
    fn d26_due_date_quick_picks_are_ios_four() {
        let picks: Vec<(&str, i64)> = DATE_OPTIONS
            .iter()
            .map(|option| (option.title_key, option.days_from_today))
            .collect();
        assert_eq!(
            picks,
            vec![
                ("picker.today", 0),
                ("picker.tomorrow", 1),
                ("picker.in_3_days", 3),
                ("picker.next_week", 7),
            ]
        );
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

    /// AITD-561. A time goes on its day with THAT day's offset, not today's. Read in PDT on
    /// 2026-10-27, "Morning" on Sunday 2026-11-01 — the day US clocks go back — is 09:00 PST,
    /// 17:00Z. With today's −07:00 it was 16:00Z, 08:00 on the phone.
    #[test]
    fn aitd_561_morning_across_a_dst_change_is_nine_local() {
        let la = chrono_tz::America::Los_Angeles;
        let current = at("2026-11-01T10:37:00Z"); // 03:37 PST on the 1st
        assert_eq!(
            date::format(with_hour(9, current, la)),
            "2026-11-01T17:00:00Z"
        );
        assert_eq!(
            date::format(with_time(18, 0, current, la)),
            "2026-11-02T02:00:00Z"
        );
    }

    /// AITD-561. "Tomorrow" on a timed task keeps the local time of day across the change.
    #[test]
    fn aitd_561_a_timed_pick_keeps_the_local_time_across_a_dst_change() {
        let la = chrono_tz::America::Los_Angeles;
        let saturday_5pm_pdt = at("2026-11-01T00:00:00Z");
        assert_eq!(
            date::format(timed_pick(1, saturday_5pm_pdt, la)),
            "2026-11-02T01:00:00Z",
            "17:00 PST on Sunday"
        );
        // And a calendar day chosen across it.
        assert_eq!(
            date::format(on_day(day(2026, 11, 2), Some(saturday_5pm_pdt), false, la)),
            "2026-11-03T01:00:00Z"
        );
    }

    /// AITD-561. The hour a change repeats takes its first reading; the hour it skips moves on to
    /// the first time that exists, rather than dropping the time.
    #[test]
    fn aitd_561_repeated_and_skipped_hours_still_give_a_time() {
        let la = chrono_tz::America::Los_Angeles;
        let fall_back = at("2026-11-01T17:00:00Z");
        assert_eq!(
            date::format(with_time(1, 30, fall_back, la)),
            "2026-11-01T08:30:00Z",
            "01:30 PDT, the first of the two"
        );
        let spring_forward = at("2026-03-08T17:00:00Z");
        assert_eq!(
            date::format(with_time(2, 30, spring_forward, la)),
            "2026-03-08T10:30:00Z",
            "02:30 does not exist; 03:30 PDT"
        );
    }
}
