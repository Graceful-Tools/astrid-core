//! Timing a task.
//!
//! Ports the Mac's `MacTimerSection` rules and the fields both Apple clients write. What is new
//! here is *where the running timer lives*: on Apple it is in memory, so quitting the app loses a
//! timer somebody started an hour ago and never notices until they look. Here the start time is
//! written to the cache, so a timer survives a restart, a crash, and a laptop lid.
//!
//! ## Two fields, and what each is for
//!
//! `timerDuration` is minutes accumulated on the task and is shared with everybody who can see it.
//! `lastTimerValue` is what the last session recorded, which is the caption a task keeps once the
//! timer is stopped — so hiding the timer section never hides the data.
//!
//! ## Minutes, and never zero
//!
//! The server's field is minutes, so a session shorter than one is either rounded up to a minute or
//! discarded. It is rounded up: somebody who started a timer, did a thing and stopped it should not
//! be told they did nothing.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// What a task's timer is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerState {
    pub is_running: bool,
    /// When the running session started, so the shell can count up from it rather than being told
    /// the elapsed time and having to keep it fresh itself.
    pub started_at: Option<DateTime<Utc>>,
    /// Minutes recorded on the task before this session.
    pub logged_minutes: i64,
    /// What the last session recorded, if anything.
    pub last_value: Option<String>,
}

/// The metadata key a running timer is remembered under.
pub fn started_key(task_id: &str) -> String {
    format!("timer.started.{task_id}")
}

/// How long a session lasted, in the minutes the server stores.
///
/// Rounded up, and never zero for a session that actually happened: somebody who started a timer,
/// did the thing and stopped it should not be told they did nothing.
pub fn minutes_between(started: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    let seconds = (now - started).num_seconds().max(0);
    if seconds == 0 {
        return 0;
    }
    (seconds + 59) / 60
}

/// The caption a stopped timer leaves behind.
///
/// Written here rather than in the shell because it is stored on the task and read by every client
/// — a Mac showing "1h 5m" beside a Windows "65 minutes" for the same session is the kind of
/// difference nobody can explain.
pub fn last_value(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn at(text: &str) -> DateTime<Utc> {
        date::parse(text).expect("a date")
    }

    /// A minute is the smallest thing the server can record, so a short session is a minute rather
    /// than nothing at all.
    #[test]
    fn a_session_shorter_than_a_minute_still_counts_as_one() {
        let started = at("2026-09-07T09:00:00Z");
        assert_eq!(minutes_between(started, at("2026-09-07T09:00:20Z")), 1);
        assert_eq!(minutes_between(started, at("2026-09-07T09:01:00Z")), 1);
        assert_eq!(minutes_between(started, at("2026-09-07T09:01:01Z")), 2);
    }

    /// A timer that never ran recorded nothing, which is different from a short session.
    #[test]
    fn no_elapsed_time_is_no_minutes() {
        let started = at("2026-09-07T09:00:00Z");
        assert_eq!(minutes_between(started, started), 0);
    }

    /// A clock that went backwards — a machine syncing its time, a laptop waking — must not record
    /// a negative session.
    #[test]
    fn a_clock_that_went_backwards_records_nothing() {
        assert_eq!(
            minutes_between(at("2026-09-07T09:00:00Z"), at("2026-09-07T08:00:00Z")),
            0
        );
    }

    #[test]
    fn the_caption_reads_the_way_a_person_says_it() {
        assert_eq!(last_value(5), "5m");
        assert_eq!(last_value(60), "1h");
        assert_eq!(last_value(65), "1h 5m");
        assert_eq!(last_value(0), "0m");
    }
}
