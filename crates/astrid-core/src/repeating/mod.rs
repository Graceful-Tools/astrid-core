//! Next-occurrence math for repeating tasks — the single source of truth on this client.
//!
//! Ported from `astrid-ios/Astrid App/Utilities/RepeatingTaskHandler.swift`
//! (`RepeatingTaskCalculator`), which mirrors `astrid-web/types/repeating.ts` and
//! `astrid-web/lib/repeating-task-handler.ts`. All three must stay behaviour-compatible.
//!
//! **Do not add pattern math anywhere else.** `TaskService::complete_task` is the only production
//! entry point and delegates here. An inline copy in the iOS `TaskService` once ignored `weekdays`,
//! `month_repeat_type`, `month_weekday` and the yearly `month`/`day`, so a weekly Mon/Wed/Fri task
//! added seven days instead of picking the next selected weekday. Collapsing the logic into one
//! place is what fixed it.
//!
//! ## Everything here is UTC
//!
//! Web does its arithmetic with `setUTC*` and all-day tasks are stored as UTC midnight, so UTC is
//! the contract. The Swift port uses a UTC calendar for the time-preserving step but `Calendar
//! .current` for the month and year steps; for a user east or west of UTC that can land a monthly
//! rollover on a different date than web computes. That divergence is recorded in
//! `docs/CONTRACTS.md` rather than reproduced here — this crate follows web.
//!
//! ## Adding a field to `CustomRepeatingPattern`
//!
//! 1. Teach this calculator to honour it.
//! 2. Add a test here that walks the pattern through several completions — single-step tests
//!    routinely miss what multi-step ones catch.
//! 3. Add a matching test at the `TaskService::complete_task` level, so the production path is
//!    covered and not just the calculator in isolation.
//! 4. Mirror the change in astrid-web and astrid-ios.

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};

/// How often a task repeats. `Custom` defers to [`CustomRepeatingPattern`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeating {
    Never,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    Custom,
}

/// Which date the next occurrence is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatFrom {
    #[serde(rename = "DUE_DATE")]
    DueDate,
    #[serde(rename = "COMPLETION_DATE")]
    CompletionDate,
}

/// When a repeating series stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndCondition {
    Never,
    AfterOccurrences,
    UntilDate,
}

/// A day of the week, named the way the wire names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

/// How a monthly pattern picks its day: the same date each month, or the same weekday-of-week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonthRepeatType {
    SameDate,
    SameWeekday,
}

/// "The third Tuesday", as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthWeekday {
    pub weekday: Weekday,
    /// 1-5 (first through fifth week of the month).
    pub week_of_month: u32,
}

/// The stored custom pattern.
///
/// Every field is optional because the server's column is free-form JSON and older clients wrote
/// subsets of it. A pattern missing what its unit needs yields no next occurrence rather than a
/// guess — see [`calculate_custom_next_occurrence`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomRepeatingPattern {
    /// Always `"custom"` on the wire; carried so a round-trip is lossless.
    pub r#type: Option<String>,
    /// `"days"`, `"weeks"`, `"months"`, `"years"`.
    pub unit: Option<String>,
    /// Every X units.
    pub interval: Option<i32>,
    pub end_condition: Option<EndCondition>,
    pub end_after_occurrences: Option<i32>,
    pub end_until_date: Option<DateTime<Utc>>,
    /// Weekly patterns: which days are selected.
    pub weekdays: Option<Vec<Weekday>>,
    pub month_repeat_type: Option<MonthRepeatType>,
    /// 1-31, for `SameDate`.
    pub month_day: Option<u32>,
    pub month_weekday: Option<MonthWeekday>,
    /// Yearly patterns: 1-12.
    pub month: Option<u32>,
    /// Yearly patterns: 1-31.
    pub day: Option<u32>,
}

/// End-condition data a simple (non-custom) pattern can also carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimplePatternEndCondition {
    pub end_condition: EndCondition,
    pub end_after_occurrences: Option<i32>,
    pub end_until_date: Option<DateTime<Utc>>,
}

/// The answer to "this task was just completed — when is it next due?".
///
/// `next_due_date` is `None` exactly when `should_terminate` is true: the series is over and the
/// task stays completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextOccurrence {
    pub next_due_date: Option<DateTime<Utc>>,
    pub should_terminate: bool,
    pub new_occurrence_count: i32,
}

/// Next occurrence for a simple pattern (daily, weekly, monthly, yearly).
///
/// `Never` and `Custom` are not simple patterns; they yield the anchor unchanged, matching the
/// Swift `default` arm, because the caller routes custom patterns to
/// [`calculate_custom_next_occurrence`] and never asks about `Never`.
pub fn calculate_simple_next_occurrence(
    repeating_type: Repeating,
    current_due_date: Option<DateTime<Utc>>,
    completion_date: DateTime<Utc>,
    repeat_from: RepeatFrom,
    current_occurrence_count: i32,
    end_data: Option<&SimplePatternEndCondition>,
) -> NextOccurrence {
    let anchor = anchor_date(current_due_date, completion_date, repeat_from);

    let next = match repeating_type {
        Repeating::Daily => adding_days(anchor, 1),
        Repeating::Weekly => adding_days(anchor, 7),
        Repeating::Monthly => adding_months(anchor, 1),
        Repeating::Yearly => adding_years(anchor, 1),
        // Neither is a simple pattern: custom patterns route to
        // `calculate_custom_next_occurrence`, and a task that never repeats is not completed
        // through this path at all. Returning the anchor mirrors the Swift default arm.
        Repeating::Never | Repeating::Custom => anchor,
    };

    let new_occurrence_count = current_occurrence_count + 1;

    let Some(end_data) = end_data else {
        return NextOccurrence {
            next_due_date: Some(next),
            should_terminate: false,
            new_occurrence_count,
        };
    };

    let (should_terminate, new_occurrence_count) =
        check_simple_pattern_end_condition(next, new_occurrence_count, end_data);
    NextOccurrence {
        next_due_date: (!should_terminate).then_some(next),
        should_terminate,
        new_occurrence_count,
    }
}

/// Whether a simple pattern's end condition has been reached.
pub fn check_simple_pattern_end_condition(
    next_due_date: DateTime<Utc>,
    new_occurrence_count: i32,
    end_data: &SimplePatternEndCondition,
) -> (bool, i32) {
    let should_terminate = match end_data.end_condition {
        // "Never" outranks the other fields even when they carry values: a series the user said
        // never ends must not be stopped by a stale limit left behind by an earlier edit.
        EndCondition::Never => false,
        EndCondition::AfterOccurrences => end_data
            .end_after_occurrences
            .is_some_and(|max| new_occurrence_count >= max),
        EndCondition::UntilDate => end_data
            .end_until_date
            .is_some_and(|end| is_after_date(next_due_date, end)),
    };
    (should_terminate, new_occurrence_count)
}

/// Next occurrence for a custom pattern.
pub fn calculate_custom_next_occurrence(
    pattern: &CustomRepeatingPattern,
    current_due_date: Option<DateTime<Utc>>,
    completion_date: DateTime<Utc>,
    repeat_from: RepeatFrom,
    current_occurrence_count: i32,
) -> NextOccurrence {
    let new_occurrence_count = current_occurrence_count + 1;
    let terminated = NextOccurrence {
        next_due_date: None,
        should_terminate: true,
        new_occurrence_count,
    };

    if pattern.end_condition == Some(EndCondition::AfterOccurrences)
        && pattern
            .end_after_occurrences
            .is_some_and(|max| new_occurrence_count >= max)
    {
        return terminated;
    }

    let anchor = anchor_date(current_due_date, completion_date, repeat_from);

    let (Some(unit), Some(interval)) = (pattern.unit.as_deref(), pattern.interval) else {
        return terminated;
    };

    let next = match unit {
        "days" => Some(adding_days(anchor, i64::from(interval))),
        "weeks" => pattern
            .weekdays
            .as_deref()
            .and_then(|weekdays| next_weekday_occurrence(anchor, weekdays)),
        "months" => next_month_occurrence(anchor, pattern, interval),
        "years" => Some(next_year_occurrence(anchor, pattern, interval)),
        _ => None,
    };

    let Some(next) = next else {
        return terminated;
    };

    if pattern.end_condition == Some(EndCondition::UntilDate)
        && pattern
            .end_until_date
            .is_some_and(|end| is_after_date(next, end))
    {
        return terminated;
    }

    NextOccurrence {
        next_due_date: Some(next),
        should_terminate: false,
        new_occurrence_count,
    }
}

// Helpers. Every one of these works in UTC — see the module docs.

/// Add whole days as elapsed time rather than by calendar arithmetic, so an all-day task stored at
/// UTC midnight lands on UTC midnight again.
fn adding_days(date: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    date + chrono::Duration::days(days)
}

/// Add months, clamping the day to what the target month has: January 31st plus a month is the
/// 28th or 29th of February, never the 2nd or 3rd of March.
fn adding_months(date: DateTime<Utc>, months: u32) -> DateTime<Utc> {
    date.checked_add_months(chrono::Months::new(months))
        .unwrap_or(date)
}

/// Add months the way `Date.setMonth` does: the day of the month is kept, and a day the target
/// month does not have spills forward. January 31st plus a month is **March 2nd**, not February
/// 29th — February is skipped entirely.
///
/// That is not a typo, and it is not this crate's choice. `adding_months` above clamps because
/// web's SIMPLE monthly step clamps, with an explicit `setUTCDate(0)`; web's CUSTOM monthly step a
/// few files away does not, and the two are reachable from the same product question. See D5.
fn adding_months_overflowing(date: DateTime<Utc>, months: u32) -> DateTime<Utc> {
    let zero_based = date.month0() + months;
    let year = date.year() + (zero_based / 12) as i32;
    let month = zero_based % 12 + 1;
    from_ymd_overflowing(year, month, date.day())
        .map(|d| d.and_time(date.time()).and_utc())
        .unwrap_or(date)
}

/// Add years the way `Date.setUTCFullYear` does: the day of the month is kept, and February 29th in
/// a year that has no February 29th spills over into March.
///
/// Deliberately NOT the month clamp above, even though clamping to February 28th is the friendlier
/// answer and is what both Apple apps do. Web is the contract, and a yearly task must roll over to
/// the same date whichever client the user completes it on. Recorded as D5 in docs/CONTRACTS.md,
/// with web's own clamp/overflow inconsistency, because it is worth fixing everywhere at once.
fn adding_years(date: DateTime<Utc>, years: i32) -> DateTime<Utc> {
    from_ymd_overflowing(date.year() + years, date.month(), date.day())
        .map(|d| d.and_time(date.time()).and_utc())
        .unwrap_or(date)
}

/// Build a date the way JavaScript's date setters do: a day number past the end of the month rolls
/// forward into the next one rather than failing. `from_ymd_opt` would return `None` there, and
/// falling back to "leave the date alone" would silently stop a series.
fn from_ymd_overflowing(year: i32, month: u32, day: u32) -> Option<chrono::NaiveDate> {
    let first = chrono::NaiveDate::from_ymd_opt(year, month, 1)?;
    first.checked_add_signed(chrono::Duration::days(i64::from(day).saturating_sub(1)))
}

/// The date the next occurrence is measured from: the chosen base date, wearing the time of day the
/// task was due. Both modes preserve the due time — completing a 9am task at 11pm must not move it
/// to 11pm forever.
fn anchor_date(
    current_due_date: Option<DateTime<Utc>>,
    completion_date: DateTime<Utc>,
    repeat_from: RepeatFrom,
) -> DateTime<Utc> {
    let Some(due) = current_due_date else {
        return completion_date;
    };
    let base = match repeat_from {
        RepeatFrom::DueDate => due,
        RepeatFrom::CompletionDate => completion_date,
    };
    base.date_naive().and_time(due.time()).and_utc()
}

/// Date-only comparison. "Repeat until Dec 15" means an occurrence ON Dec 15 still runs, so the
/// times of day must not decide it.
fn is_after_date(candidate: DateTime<Utc>, end: DateTime<Utc>) -> bool {
    candidate.date_naive() > end.date_naive()
}

/// The next selected weekday strictly after `date`.
///
/// Note that `interval` plays no part: web and the Swift port both ignore it for weekly patterns,
/// so "every 2 weeks on Mon/Wed" advances weekly on all three clients. Reproduced deliberately —
/// this is a shared quirk to fix everywhere at once, not a Windows bug to fix here. Recorded in
/// `docs/CONTRACTS.md`.
fn next_weekday_occurrence(date: DateTime<Utc>, weekdays: &[Weekday]) -> Option<DateTime<Utc>> {
    if weekdays.is_empty() {
        return None;
    }
    // A non-empty selection always matches within seven days, so one pass is enough.
    (1..=7)
        .map(|offset| adding_days(date, offset))
        .find(|candidate| weekdays.iter().any(|w| w.matches(candidate.weekday())))
}

fn next_month_occurrence(
    date: DateTime<Utc>,
    pattern: &CustomRepeatingPattern,
    interval: i32,
) -> Option<DateTime<Utc>> {
    let months = u32::try_from(interval).ok()?;
    match pattern.month_repeat_type? {
        // Overflowing, not clamping: see `adding_months_overflowing`. A task set to the 31st skips
        // February on every client, which is a shared bug rather than one to fix here alone.
        MonthRepeatType::SameDate => Some(adding_months_overflowing(date, months)),
        MonthRepeatType::SameWeekday => {
            let month_weekday = pattern.month_weekday?;
            // The intermediate step decides only WHICH month to search, but it overflows too — so
            // a fifth weekday sitting on the 31st searches the month after next. Matching web
            // matters more than being right on its own here.
            let target_month = adding_months_overflowing(date, months);
            let first_of_month =
                chrono::NaiveDate::from_ymd_opt(target_month.year(), target_month.month(), 1)?;
            let offset = (i64::from(month_weekday.weekday.number())
                - i64::from(first_of_month.weekday().num_days_from_sunday())
                + 7)
                % 7
                + i64::from(month_weekday.week_of_month.saturating_sub(1)) * 7;
            // The time of day is lost here rather than carried, matching web and Swift: both build
            // this date from the first of the month, which is midnight. Changing it would put this
            // client's occurrences an hour or nine out of step with the others.
            Some(
                (first_of_month + chrono::Duration::days(offset))
                    .and_time(chrono::NaiveTime::MIN)
                    .and_utc(),
            )
        }
    }
}

fn next_year_occurrence(
    date: DateTime<Utc>,
    pattern: &CustomRepeatingPattern,
    interval: i32,
) -> DateTime<Utc> {
    let year = date.year() + interval;
    let month = pattern.month.unwrap_or_else(|| date.month());
    let day = pattern.day.unwrap_or_else(|| date.day());
    // Overflowing rather than failing, for the same reason as `adding_years`: web's date setters
    // roll a day past the month's end forward, and a pattern saying "February 30th" passes web's
    // own validity check, so it has to land somewhere rather than stalling the series.
    from_ymd_overflowing(year, month, day)
        .map(|d| d.and_time(date.time()).and_utc())
        .unwrap_or(date)
}

impl Weekday {
    /// Sunday is 0, matching the numbering web and the Swift port both index with.
    fn number(self) -> u32 {
        match self {
            Weekday::Sunday => 0,
            Weekday::Monday => 1,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
        }
    }

    fn matches(self, weekday: chrono::Weekday) -> bool {
        self.number() == weekday.num_days_from_sunday()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("test date is unambiguous")
    }

    fn weekly_pattern(weekdays: &[Weekday], interval: i32) -> CustomRepeatingPattern {
        CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("weeks".into()),
            interval: Some(interval),
            end_condition: Some(EndCondition::Never),
            weekdays: Some(weekdays.to_vec()),
            ..Default::default()
        }
    }

    /// Walk a pattern through `count` completions, completing each one exactly on its due date —
    /// the shape that surfaces bugs single-step tests miss.
    fn progression(
        pattern: &CustomRepeatingPattern,
        start: DateTime<Utc>,
        repeat_from: RepeatFrom,
        count: usize,
    ) -> Vec<DateTime<Utc>> {
        let mut dates = Vec::new();
        let mut current = start;
        let mut occurrences = 0;
        for _ in 0..count {
            let result = calculate_custom_next_occurrence(
                pattern,
                Some(current),
                current,
                repeat_from,
                occurrences,
            );
            let Some(next) = result.next_due_date else {
                break;
            };
            dates.push(next);
            current = next;
            occurrences = result.new_occurrence_count;
            if result.should_terminate {
                break;
            }
        }
        dates
    }

    // Simple patterns.

    #[test]
    fn daily_from_the_due_date_keeps_the_due_time() {
        let result = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 15, 10, 30)),
            utc(2024, 1, 15, 14, 0),
            RepeatFrom::DueDate,
            0,
            None,
        );
        let next = result
            .next_due_date
            .expect("a daily task always has a next occurrence");
        assert_eq!((next.day(), next.hour(), next.minute()), (16, 10, 30));
        assert!(!result.should_terminate);
        assert_eq!(result.new_occurrence_count, 1);
    }

    /// Completed two days late: the next occurrence is one day after the COMPLETION, still at the
    /// time the task was due.
    #[test]
    fn daily_from_the_completion_date_measures_from_the_completion() {
        let result = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 15, 10, 30)),
            utc(2024, 1, 17, 14, 0),
            RepeatFrom::CompletionDate,
            0,
            None,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!((next.day(), next.hour(), next.minute()), (18, 10, 30));
    }

    #[test]
    fn weekly_adds_seven_days_and_counts_the_occurrence() {
        let result = calculate_simple_next_occurrence(
            Repeating::Weekly,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 10, 0),
            RepeatFrom::DueDate,
            2,
            None,
        );
        assert_eq!(result.next_due_date.unwrap().day(), 22);
        assert_eq!(result.new_occurrence_count, 3);
    }

    #[test]
    fn monthly_lands_on_the_same_date_next_month() {
        let result = calculate_simple_next_occurrence(
            Repeating::Monthly,
            Some(utc(2024, 1, 15, 14, 0)),
            utc(2024, 1, 15, 16, 0),
            RepeatFrom::DueDate,
            0,
            None,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!((next.month(), next.day()), (2, 15));
    }

    /// January 31st has no February counterpart, so the rollover clamps to the last day February
    /// has — the 29th in a leap year.
    #[test]
    fn monthly_clamps_at_the_end_of_a_short_month() {
        let result = calculate_simple_next_occurrence(
            Repeating::Monthly,
            Some(utc(2024, 1, 31, 9, 0)),
            utc(2024, 1, 31, 10, 0),
            RepeatFrom::DueDate,
            0,
            None,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!(next.month(), 2);
        assert_eq!(next.day(), 29, "2024 is a leap year");
    }

    #[test]
    fn yearly_advances_the_year_and_keeps_the_date() {
        let result = calculate_simple_next_occurrence(
            Repeating::Yearly,
            Some(utc(2024, 6, 15, 12, 0)),
            utc(2024, 6, 15, 14, 0),
            RepeatFrom::DueDate,
            0,
            None,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!((next.year(), next.month(), next.day()), (2025, 6, 15));
    }

    /// February 29th plus a year has no February 29th to land on, and web spills it into March
    /// rather than clamping to the 28th — the opposite of what web does for a monthly rollover, and
    /// the opposite of what both Apple apps do here. Matching web is the contract; see D5 in
    /// docs/CONTRACTS.md for why this is a cross-repo fix rather than a local one.
    #[test]
    fn a_leap_day_task_rolls_into_march_the_way_web_does() {
        let result = calculate_simple_next_occurrence(
            Repeating::Yearly,
            Some(utc(2024, 2, 29, 9, 0)),
            utc(2024, 2, 29, 9, 0),
            RepeatFrom::DueDate,
            0,
            None,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!((next.year(), next.month(), next.day()), (2025, 3, 1));
        assert_eq!(next.hour(), 9, "the time of day still survives");
    }

    #[test]
    fn with_no_due_date_the_completion_is_the_anchor() {
        let result = calculate_simple_next_occurrence(
            Repeating::Daily,
            None,
            utc(2024, 3, 10, 8, 0),
            RepeatFrom::DueDate,
            0,
            None,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!((next.month(), next.day(), next.hour()), (3, 11, 8));
    }

    // End conditions on simple patterns.

    #[test]
    fn a_simple_pattern_terminates_at_its_occurrence_limit() {
        let end = SimplePatternEndCondition {
            end_condition: EndCondition::AfterOccurrences,
            end_after_occurrences: Some(3),
            end_until_date: None,
        };
        let below = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::DueDate,
            1,
            Some(&end),
        );
        assert!(!below.should_terminate, "the second of three still repeats");
        assert!(below.next_due_date.is_some());

        let at_limit = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::DueDate,
            2,
            Some(&end),
        );
        assert!(
            at_limit.should_terminate,
            "the third completion ends the series"
        );
        assert_eq!(at_limit.next_due_date, None);
        assert_eq!(at_limit.new_occurrence_count, 3);
    }

    /// "Repeat until Jan 16" means an occurrence ON Jan 16 still runs — the comparison is by date,
    /// not by instant, so a later time of day on the final date does not cut the series short.
    #[test]
    fn until_date_includes_the_end_date_itself() {
        let end = SimplePatternEndCondition {
            end_condition: EndCondition::UntilDate,
            end_after_occurrences: None,
            end_until_date: Some(utc(2024, 1, 16, 0, 0)),
        };
        let on_the_date = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 15, 23, 0)),
            utc(2024, 1, 15, 23, 0),
            RepeatFrom::DueDate,
            0,
            Some(&end),
        );
        assert!(
            !on_the_date.should_terminate,
            "an occurrence on the end date still runs"
        );

        let past_the_date = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 16, 9, 0)),
            utc(2024, 1, 16, 9, 0),
            RepeatFrom::DueDate,
            0,
            Some(&end),
        );
        assert!(past_the_date.should_terminate);
        assert_eq!(past_the_date.next_due_date, None);
    }

    #[test]
    fn an_end_condition_of_never_never_terminates() {
        let end = SimplePatternEndCondition {
            end_condition: EndCondition::Never,
            end_after_occurrences: Some(2),
            end_until_date: Some(utc(2020, 1, 1, 0, 0)),
        };
        let result = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::DueDate,
            99,
            Some(&end),
        );
        assert!(
            !result.should_terminate,
            "'never' outranks the other fields being set"
        );
        assert!(result.next_due_date.is_some());
    }

    // Custom patterns.

    #[test]
    fn a_custom_day_interval_adds_that_many_days() {
        let pattern = CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("days".into()),
            interval: Some(3),
            end_condition: Some(EndCondition::Never),
            ..Default::default()
        };
        let result = calculate_custom_next_occurrence(
            &pattern,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::DueDate,
            0,
        );
        assert_eq!(result.next_due_date.unwrap().day(), 18);
    }

    /// The reported bug this module exists to prevent: "every week on Mon, Wed, Fri" must walk the
    /// selected days, not add seven days each time.
    #[test]
    fn weekly_mon_wed_fri_walks_the_selected_days() {
        let pattern = weekly_pattern(&[Weekday::Monday, Weekday::Wednesday, Weekday::Friday], 1);
        let dates = progression(&pattern, utc(2024, 1, 15, 9, 0), RepeatFrom::DueDate, 6);

        assert_eq!(dates.len(), 6);
        // Starting Monday Jan 15: Wed 17, Fri 19, Mon 22, Wed 24, Fri 26, Mon 29.
        let expected_days = [17, 19, 22, 24, 26, 29];
        let expected_weekdays = [
            chrono::Weekday::Wed,
            chrono::Weekday::Fri,
            chrono::Weekday::Mon,
            chrono::Weekday::Wed,
            chrono::Weekday::Fri,
            chrono::Weekday::Mon,
        ];
        for (i, date) in dates.iter().enumerate() {
            assert_eq!(date.day(), expected_days[i], "step {}", i + 1);
            assert_eq!(date.weekday(), expected_weekdays[i], "step {}", i + 1);
            assert_eq!(
                date.hour(),
                9,
                "step {}: the time of day is preserved",
                i + 1
            );
        }
    }

    /// Completing on the due date each time, the completion-anchored variant walks the same days.
    #[test]
    fn weekly_mon_wed_fri_walks_the_same_days_from_the_completion() {
        let pattern = weekly_pattern(&[Weekday::Monday, Weekday::Wednesday, Weekday::Friday], 1);
        let dates = progression(
            &pattern,
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::CompletionDate,
            6,
        );
        let days: Vec<u32> = dates.iter().map(|d| d.day()).collect();
        assert_eq!(days, vec![17, 19, 22, 24, 26, 29]);
    }

    /// One selected day means wrapping a whole week rather than finding a nearer match.
    #[test]
    fn a_single_selected_weekday_wraps_to_the_next_week() {
        let pattern = weekly_pattern(&[Weekday::Monday], 1);
        let result = calculate_custom_next_occurrence(
            &pattern,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::DueDate,
            0,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!(next.day(), 22);
        assert_eq!(next.weekday(), chrono::Weekday::Mon);
    }

    #[test]
    fn monthly_same_date_holds_the_day_through_a_year() {
        let pattern = CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("months".into()),
            interval: Some(1),
            end_condition: Some(EndCondition::Never),
            month_repeat_type: Some(MonthRepeatType::SameDate),
            month_day: Some(15),
            ..Default::default()
        };
        let dates = progression(&pattern, utc(2024, 1, 15, 14, 0), RepeatFrom::DueDate, 12);

        assert_eq!(dates.len(), 12);
        for (i, date) in dates.iter().enumerate() {
            assert_eq!(date.day(), 15, "step {}", i + 1);
            assert_eq!(date.month(), ((1 + i as u32) % 12) + 1, "step {}", i + 1);
            assert_eq!(
                date.hour(),
                14,
                "step {}: the time of day is preserved",
                i + 1
            );
        }
    }

    /// A custom monthly pattern on the 31st **skips February**, because web overflows here rather
    /// than clamping the way its own simple monthly step does. Matching that is deliberate; the
    /// reasoning, and why it is a cross-repo fix, is D5 in docs/CONTRACTS.md.
    #[test]
    fn a_custom_monthly_on_the_31st_overflows_the_way_web_does() {
        let pattern = CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("months".into()),
            interval: Some(1),
            end_condition: Some(EndCondition::Never),
            month_repeat_type: Some(MonthRepeatType::SameDate),
            month_day: Some(31),
            ..Default::default()
        };
        let result = calculate_custom_next_occurrence(
            &pattern,
            Some(utc(2024, 1, 31, 9, 0)),
            utc(2024, 1, 31, 9, 0),
            RepeatFrom::DueDate,
            0,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!(
            (next.month(), next.day()),
            (3, 2),
            "web rolls the overflow forward into March; February gets no occurrence at all"
        );
    }

    /// "The third Tuesday of every month" — the date moves, the weekday and week-of-month do not.
    #[test]
    fn monthly_same_weekday_finds_the_third_tuesday() {
        let pattern = CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("months".into()),
            interval: Some(1),
            end_condition: Some(EndCondition::Never),
            month_repeat_type: Some(MonthRepeatType::SameWeekday),
            month_weekday: Some(MonthWeekday {
                weekday: Weekday::Tuesday,
                week_of_month: 3,
            }),
            ..Default::default()
        };
        // Jan 16 2024 is the third Tuesday of January.
        let dates = progression(&pattern, utc(2024, 1, 16, 10, 0), RepeatFrom::DueDate, 6);

        assert_eq!(dates.len(), 6);
        let expected = [(2, 20), (3, 19), (4, 16), (5, 21), (6, 18), (7, 16)];
        for (i, date) in dates.iter().enumerate() {
            assert_eq!(
                (date.month(), date.day()),
                expected[i],
                "step {}: expected the third Tuesday",
                i + 1
            );
            assert_eq!(date.weekday(), chrono::Weekday::Tue, "step {}", i + 1);
        }
    }

    #[test]
    fn a_custom_year_interval_sets_the_month_and_day() {
        let pattern = CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("years".into()),
            interval: Some(2),
            end_condition: Some(EndCondition::Never),
            month: Some(3),
            day: Some(10),
            ..Default::default()
        };
        let result = calculate_custom_next_occurrence(
            &pattern,
            Some(utc(2024, 6, 15, 12, 0)),
            utc(2024, 6, 15, 12, 0),
            RepeatFrom::DueDate,
            0,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!((next.year(), next.month(), next.day()), (2026, 3, 10));
    }

    #[test]
    fn a_custom_pattern_terminates_at_its_occurrence_limit() {
        let mut pattern = weekly_pattern(&[Weekday::Monday, Weekday::Wednesday], 1);
        pattern.end_condition = Some(EndCondition::AfterOccurrences);
        pattern.end_after_occurrences = Some(4);

        let dates = progression(&pattern, utc(2024, 1, 15, 9, 0), RepeatFrom::DueDate, 10);
        assert_eq!(
            dates.len(),
            3,
            "the fourth completion ends the series, producing no date"
        );
    }

    /// A pattern with no unit cannot be computed. Terminating is the honest answer; inventing a
    /// fallback interval would silently reschedule the task to the wrong day.
    #[test]
    fn an_incomplete_pattern_terminates_rather_than_guessing() {
        let pattern = CustomRepeatingPattern {
            r#type: Some("custom".into()),
            end_condition: Some(EndCondition::Never),
            ..Default::default()
        };
        let result = calculate_custom_next_occurrence(
            &pattern,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::DueDate,
            0,
        );
        assert_eq!(result.next_due_date, None);
        assert!(result.should_terminate);
    }

    /// A weekly pattern with no days selected has nothing to advance to.
    #[test]
    fn a_weekly_pattern_with_no_selected_days_terminates() {
        let pattern = CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("weeks".into()),
            interval: Some(1),
            end_condition: Some(EndCondition::Never),
            weekdays: Some(vec![]),
            ..Default::default()
        };
        let result = calculate_custom_next_occurrence(
            &pattern,
            Some(utc(2024, 1, 15, 9, 0)),
            utc(2024, 1, 15, 9, 0),
            RepeatFrom::DueDate,
            0,
        );
        assert_eq!(result.next_due_date, None);
        assert!(result.should_terminate);
    }

    /// All-day tasks are stored as UTC midnight; the rollover must keep them there rather than
    /// drifting into the previous or next day.
    #[test]
    fn an_all_day_task_stays_at_utc_midnight() {
        let result = calculate_simple_next_occurrence(
            Repeating::Daily,
            Some(utc(2024, 1, 15, 0, 0)),
            utc(2024, 1, 15, 19, 30),
            RepeatFrom::CompletionDate,
            0,
            None,
        );
        let next = result.next_due_date.unwrap();
        assert_eq!(
            (next.month(), next.day(), next.hour(), next.minute()),
            (1, 16, 0, 0)
        );
    }
}
