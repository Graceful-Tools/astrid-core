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
//! ## All-day is not a time zone
//!
//! An all-day task's `dueDateTime` is an instant like any other, but only its **calendar day** is
//! meaningful, and the day that matters is the one the person who set it was looking at. The wire
//! format cannot express that, so the rule the clients share is: an all-day date is written at
//! **noon UTC** and read back by taking its UTC calendar day. Noon, not midnight, is what keeps a
//! reader in UTC-11 or UTC+13 on the same day as the writer — midnight UTC is the previous day in
//! the Americas, which is how a "due today" task used to render as overdue before anyone had done
//! anything wrong.

use chrono::{DateTime, NaiveDate, SecondsFormat, TimeZone, Utc};
use serde::de::{self, Deserializer, Unexpected};
use serde::{Deserialize, Serializer};

/// The hour an all-day date is anchored at, in UTC. See the module note: this is what keeps every
/// reader on the writer's calendar day regardless of their offset.
pub const ALL_DAY_ANCHOR_HOUR: u32 = 12;

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
            .expect("noon is a valid time on every day"),
    )
}

/// The calendar day an all-day instant means. Always read in UTC — reading it in the device's zone
/// is the bug the noon anchor exists to survive, and doing both would defeat it.
pub fn all_day_date(at: DateTime<Utc>) -> NaiveDate {
    at.date_naive()
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

    /// Noon, not midnight. At midnight UTC a reader in New York is still on the previous day, and
    /// "due today" renders as overdue for a third of the planet.
    #[test]
    fn an_all_day_date_is_anchored_at_noon_utc() {
        let day = NaiveDate::from_ymd_opt(2026, 9, 7).expect("a real day");
        assert_eq!(format(all_day_instant(day)), "2026-09-07T12:00:00Z");
    }

    #[test]
    fn an_all_day_instant_round_trips_through_its_day() {
        let day = NaiveDate::from_ymd_opt(2028, 2, 29).expect("2028 is a leap year");
        assert_eq!(all_day_date(all_day_instant(day)), day);
    }
}
