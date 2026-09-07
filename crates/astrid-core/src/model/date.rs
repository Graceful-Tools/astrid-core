//! Reading and writing the instants the server sends.
//!
//! Ported from the decoder configured in
//! `astrid-ios/Astrid App/Core/Networking/AstridAPIClient.swift`, which tries ISO-8601 **with**
//! fractional seconds first and falls back to without. Prisma emits milliseconds
//! (`2026-09-07T12:00:00.000Z`); hand-written server responses and the sync providers emit whole
//! seconds. Both are the same instant and both have been seen on the same endpoint.
//!
//! Writing is the narrower of the two on purpose: whole seconds with a `Z`, matching Swift's
//! `.iso8601` encoder. The server stores what it is given, so a client that wrote milliseconds
//! would make otherwise identical round-trips differ in the database and in every `updatedAt`
//! comparison sync makes.
//!
//! ## All-day is a calendar date wearing an instant's clothes
//!
//! An all-day task's `dueDateTime` is an instant like any other on the wire, but only its
//! **calendar day** is meaningful. The convention all three clients follow is Google Calendar's,
//! and `astrid-web/lib/date-comparison.ts` states it: an all-day date is stored at **midnight
//! UTC**, and the day it means is its **UTC** calendar day.
//!
//! The half that is easy to get wrong is the comparison, not the storage. "Is this due today?" is
//! answered by taking the reader's **local** calendar day, re-expressing that day as midnight UTC,
//! and comparing it with the stored value — `getLocalDateAsUTCMidnight` against
//! `getUTCDateMidnight` on web, and the same pairing here. Comparing the stored instant with `now`
//! instead makes a task due today read as overdue from midnight UTC onwards, which is
//! mid-afternoon in California.
//!
//! So: [`all_day_instant`] writes a day, [`all_day_date`] reads one back, and anything asking
//! "which day is it where the reader is?" goes through the reader's offset and says so.

use chrono::{DateTime, NaiveDate, SecondsFormat, TimeZone, Utc};
use serde::de::{self, Deserializer, Unexpected};
use serde::{Deserialize, Serializer};

/// The hour an all-day date is stored at, in UTC. Midnight, matching
/// `astrid-web/lib/date-comparison.ts` and the Google Calendar convention the sync providers use.
pub const ALL_DAY_ANCHOR_HOUR: u32 = 0;

/// Parse an instant the way the Apple client's decoder does: fractional seconds first, then
/// without.
///
/// `DateTime::parse_from_rfc3339` accepts both spellings, so the two-step fallback collapses into
/// one call — but the leniency it stands for is the point, not the number of attempts.
pub fn parse(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Format an instant the way this client always writes one: whole seconds, `Z`.
pub fn format(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The instant that represents an all-day task due on `day`.
pub fn all_day_instant(day: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(
        &day.and_hms_opt(ALL_DAY_ANCHOR_HOUR, 0, 0)
            .expect("midnight is a valid time on every day"),
    )
}

/// The calendar day an all-day instant means.
///
/// **Always read in UTC**, whatever the reader's offset. Reading it locally moves the date for
/// anyone west of UTC — a 25 December task prints as the 24th. The reader's own day enters the
/// comparison on the other side, not here; see the module note.
pub fn all_day_date(at: DateTime<Utc>) -> NaiveDate {
    at.date_naive()
}

/// The instant to store for an all-day task due on the reader's today.
///
/// Their calendar day, re-expressed as midnight UTC — `getLocalDateAsUTCMidnight` on web. Using
/// `now`'s UTC date instead would file a task created at 22:00 in California under tomorrow, every
/// evening.
pub fn all_day_today(now: DateTime<Utc>, offset: chrono::FixedOffset) -> DateTime<Utc> {
    all_day_instant(now.with_timezone(&offset).date_naive())
}

/// serde adapter for `Option<DateTime<Utc>>` fields, which is nearly all of them.
///
/// Absent, `null`, an empty string and an unparseable string all read as `None`. That last one is
/// deliberate leniency: one malformed `createdAt` on one comment must not fail the decode of the
/// response it arrived in. The field it lands in is optional on every model for exactly that
/// reason.
pub mod optional {
    use super::*;

    pub fn serialize<S: Serializer>(
        value: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(at) => serializer.serialize_str(&super::format(*at)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw.as_deref().and_then(super::parse))
    }
}

/// serde adapter for the rare field that must be present, used by the sync cursor.
pub mod required {
    use super::*;

    pub fn serialize<S: Serializer>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::format(*value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let raw = String::deserialize(deserializer)?;
        super::parse(&raw)
            .ok_or_else(|| de::Error::invalid_value(Unexpected::Str(&raw), &"an ISO-8601 instant"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_the_milliseconds_prisma_writes() {
        let at = parse("2026-09-07T12:34:56.789Z").expect("parses");
        assert_eq!(format(at), "2026-09-07T12:34:56Z");
    }

    #[test]
    fn it_reads_whole_seconds_too() {
        assert!(parse("2026-09-07T12:34:56Z").is_some());
    }

    /// The sync providers and some hand-written routes send an offset rather than `Z`. Same
    /// instant; a client that rejected it would drop the task.
    #[test]
    fn it_reads_an_offset_and_normalises_it_to_utc() {
        let at = parse("2026-09-07T08:34:56-04:00").expect("parses");
        assert_eq!(format(at), "2026-09-07T12:34:56Z");
    }

    #[test]
    fn it_writes_whole_seconds_with_a_z_like_the_apple_encoder() {
        let at = parse("2026-01-02T03:04:05.123456Z").expect("parses");
        assert_eq!(format(at), "2026-01-02T03:04:05Z");
    }

    #[test]
    fn nonsense_is_none_rather_than_a_failed_response() {
        assert!(parse("").is_none());
        assert!(parse("yesterday").is_none());
        assert!(parse("2026-13-45T99:99:99Z").is_none());
    }

    /// Midnight UTC, matching web and the convention the sync providers use. Storing anything else
    /// makes every date this client writes land a day out on one of the other two clients.
    #[test]
    fn an_all_day_date_is_stored_at_midnight_utc() {
        let day = NaiveDate::from_ymd_opt(2026, 9, 7).expect("a real day");
        assert_eq!(format(all_day_instant(day)), "2026-09-07T00:00:00Z");
    }

    /// The reader's day, not UTC's. A task created at 22:00 in California is due today for the
    /// person creating it; `now`'s UTC date would file it under tomorrow every evening.
    #[test]
    fn todays_all_day_date_is_the_readers_day_rather_than_utcs() {
        let california = chrono::FixedOffset::east_opt(-7 * 3600).expect("an offset");
        // 22:00 on the 7th in California is 05:00 on the 8th in UTC.
        let evening = parse("2026-09-08T05:00:00Z").expect("an instant");
        assert_eq!(
            format(all_day_today(evening, california)),
            "2026-09-07T00:00:00Z"
        );
    }

    #[test]
    fn an_all_day_instant_round_trips_through_its_day() {
        let day = NaiveDate::from_ymd_opt(2028, 2, 29).expect("2028 is a leap year");
        assert_eq!(all_day_date(all_day_instant(day)), day);
    }
}
