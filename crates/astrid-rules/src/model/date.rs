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

use jiff::civil::Date;
use jiff::tz::Offset;
use jiff::Timestamp as Instant;
use serde::de::{self, Deserializer, Unexpected};
use serde::{Deserialize, Serializer};

/// The hour an all-day date is stored at, in UTC. Midnight, matching
/// `astrid-web/lib/date-comparison.ts` and the Google Calendar convention the sync providers use.
pub const ALL_DAY_ANCHOR_HOUR: i8 = 0;

/// What a field written by [`optional`] or [`required`] looks like **to the schema** — an
/// ISO-8601 instant.
///
/// A `#[serde(with = "date::optional")]` names a MODULE, and `schemars`' derive reads serde's
/// attributes: handed a module path where it expects a type, it fails to compile. So every such
/// field says what it is in schema terms, and this alias is what it says. It is the field's real
/// type, so the schema that comes out is `{"type":"string","format":"date-time"}` — which is what
/// goes on the wire.
///
/// Only under `schema`: nothing else in this crate needs a second name for an instant.
#[cfg(feature = "schema")]
pub struct Timestamp;

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Timestamp {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DateTime".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "format": "date-time" })
    }
}

/// What a calendar date (`2026-09-07`) looks like to the schema.
#[cfg(feature = "schema")]
pub struct CalendarDate;

#[cfg(feature = "schema")]
impl schemars::JsonSchema for CalendarDate {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "NaiveDate".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "format": "date" })
    }
}

/// Parse an instant the way the Apple client's decoder does: fractional seconds first, then
/// without.
///
/// One reader accepts both spellings, so the two-step fallback collapses into one call — but the
/// leniency it stands for is the point, not the number of attempts.
///
/// RFC 3339 and nothing wider: `YYYY-MM-DD`, `T` (or `t`, or a space), `HH:MM:SS`, an optional
/// fraction of any length, and `Z` or `±HH:MM`. Written out here rather than handed to jiff's
/// `FromStr`, which also reads zone annotations (`[America/New_York]`), ordinals and week dates —
/// and carries the zone database's lookup code into the browser build to do it
/// (`scripts/wasm-size.mjs`). A leap second reads as the last second of the minute.
pub fn parse(text: &str) -> Option<Instant> {
    let bytes = text.as_bytes();
    let number = |from: usize, len: usize| -> Option<i32> {
        let digits = bytes.get(from..from + len)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse().ok()
    };
    let at = |index: usize| bytes.get(index).copied();

    let year = number(0, 4)?;
    let month = number(5, 2)?;
    let day = number(8, 2)?;
    let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
    if at(4) != Some(b'-') || at(7) != Some(b'-') || at(13) != Some(b':') || at(16) != Some(b':') {
        return None;
    }
    if !matches!(at(10), Some(b'T' | b't' | b' ')) {
        return None;
    }

    let mut index = 19;
    let mut nanos: i32 = 0;
    if at(index) == Some(b'.') {
        let start = index + 1;
        index = start;
        let mut scale = 100_000_000;
        while let Some(digit @ b'0'..=b'9') = at(index) {
            nanos += i32::from(digit - b'0') * scale;
            scale /= 10;
            index += 1;
        }
        if index == start {
            return None;
        }
    }

    let offset_seconds = match at(index)? {
        b'Z' | b'z' => {
            index += 1;
            0
        }
        sign @ (b'+' | b'-') => {
            let hours = number(index + 1, 2)?;
            let minutes = number(index + 4, 2)?;
            if at(index + 3) != Some(b':') || hours > 23 || minutes > 59 {
                return None;
            }
            index += 6;
            let seconds = hours * 3600 + minutes * 60;
            if sign == b'-' {
                -seconds
            } else {
                seconds
            }
        }
        _ => return None,
    };
    if index != bytes.len() {
        return None;
    }

    let date = Date::new(
        year.try_into().ok()?,
        month.try_into().ok()?,
        day.try_into().ok()?,
    )
    .ok()?;
    let time = jiff::civil::Time::new(
        hour.try_into().ok()?,
        minute.try_into().ok()?,
        second.min(59).try_into().ok()?,
        nanos,
    )
    .ok()?;
    Offset::from_seconds(offset_seconds)
        .ok()?
        .to_timestamp(date.to_datetime(time))
        .ok()
}

/// Format an instant the way this client writes one: `Z`, and whole seconds unless the instant
/// carries milliseconds — then those too, so two completions made in the same second still sort
/// (AITD-369). Nothing finer than a millisecond: the server stores no more.
pub fn format(at: Instant) -> String {
    match subsec_nanos(at) / 1_000_000 {
        0 => format!("{}Z", civil_stamp(at)),
        millis => format!("{}.{millis:03}Z", civil_stamp(at)),
    }
}

/// `2026-09-07T12:34:56`, the UTC reading of an instant to the second. Written out rather than
/// through `strftime`: the formatter's machinery is a measurable part of the browser build
/// (`scripts/wasm-size.mjs`), and this is the only shape the door ever writes.
fn civil_stamp(at: Instant) -> String {
    let reading = Offset::UTC.to_datetime(at);
    let year = i32::from(reading.year());
    let sign = if year < 0 { "-" } else { "" };
    format!(
        "{sign}{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year.abs(),
        reading.month(),
        reading.day(),
        reading.hour(),
        reading.minute(),
        reading.second()
    )
}

/// A calendar day written `YYYY-MM-DD`, with a month or day that may lack its leading zero — what a
/// list's "since this date" setting stores. Nothing after the day: a time on the end is not a day.
pub fn parse_day(text: &str) -> Option<Date> {
    let mut parts = text.splitn(3, '-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    let digits = |part: &str, longest: usize| {
        (1..=longest).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    if !(digits(year, 4) && digits(month, 2) && digits(day, 2)) {
        return None;
    }
    Date::new(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?).ok()
}

/// A span of whole calendar days, or `None` past what a span can hold.
///
/// `Span::days` panics there, and the panic's error formatting is a measurable part of the browser
/// build (`scripts/wasm-size.mjs`) — so every span in this crate is made with the `try_` builders.
pub fn span_days(days: i64) -> Option<jiff::Span> {
    jiff::Span::new().try_days(days).ok()
}

/// A span of whole calendar months, or `None` past what a span can hold. See [`span_days`].
pub fn span_months(months: i64) -> Option<jiff::Span> {
    jiff::Span::new().try_months(months).ok()
}

/// `day` moved by whole calendar days, or `None` past the edge of the calendar.
pub fn days_after(day: Date, days: i64) -> Option<Date> {
    day.checked_add(span_days(days)?).ok()
}

/// [`days_after`], held at the day itself rather than failing.
pub fn days_after_or_same(day: Date, days: i64) -> Date {
    days_after(day, days).unwrap_or(day)
}

/// `at` moved on by elapsed time, held at the edge of the calendar rather than failing.
pub fn plus(at: Instant, by: jiff::SignedDuration) -> Instant {
    at.checked_add(by).unwrap_or(if by.is_negative() {
        Instant::MIN
    } else {
        Instant::MAX
    })
}

/// `at` moved back by elapsed time, held at the edge of the calendar rather than failing.
pub fn minus(at: Instant, by: jiff::SignedDuration) -> Instant {
    plus(at, -by)
}

/// The nanoseconds past the whole second, counted forward whichever side of 1970 the instant is on.
fn subsec_nanos(at: Instant) -> u32 {
    u32::try_from(at.as_nanosecond().rem_euclid(1_000_000_000)).unwrap_or(0)
}

/// The instant as RFC 3339 with a numeric `+00:00` offset and as many fractional digits as it
/// needs, in threes — what `DateTime<Utc>::to_rfc3339` wrote, which one string field of the rules
/// door still carries (`reminderPicks`), so the text it holds did not change with the library.
pub fn rfc3339_offset(at: Instant) -> String {
    format!("{}{}+00:00", civil_stamp(at), auto_fraction(at))
}

/// The instant as RFC 3339 with `Z` and as many fractional digits as it needs, in threes — what
/// `DateTime<Utc>`'s own serde wrote, kept for the few fields that never had an adapter.
pub fn rfc3339_z(at: Instant) -> String {
    format!("{}{}Z", civil_stamp(at), auto_fraction(at))
}

/// `""`, `.123`, `.123456` or `.123456789`: the shortest of those that holds the instant exactly.
fn auto_fraction(at: Instant) -> String {
    let nanos = subsec_nanos(at);
    if nanos == 0 {
        String::new()
    } else if nanos.is_multiple_of(1_000_000) {
        format!(".{:03}", nanos / 1_000_000)
    } else if nanos.is_multiple_of(1_000) {
        format!(".{:06}", nanos / 1_000)
    } else {
        format!(".{nanos:09}")
    }
}

/// serde adapter for a plain `Timestamp` field that is written as [`rfc3339_z`] and read strictly.
pub mod exact {
    use super::*;

    pub fn serialize<S: Serializer>(value: &Instant, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::rfc3339_z(*value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Instant, D::Error> {
        let raw = String::deserialize(deserializer)?;
        super::parse(&raw)
            .ok_or_else(|| de::Error::invalid_value(Unexpected::Str(&raw), &"an ISO-8601 instant"))
    }
}

/// [`exact`] for an `Option`, written `null` when absent.
pub mod exact_optional {
    use super::*;

    pub fn serialize<S: Serializer>(
        value: &Option<Instant>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(at) => serializer.serialize_str(&super::rfc3339_z(*at)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Instant>, D::Error> {
        let Some(raw) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };
        super::parse(&raw)
            .map(Some)
            .ok_or_else(|| de::Error::invalid_value(Unexpected::Str(&raw), &"an ISO-8601 instant"))
    }
}

/// `2026-09-07`, written the way a calendar day goes on the wire.
pub fn format_day(day: Date) -> String {
    let year = i32::from(day.year());
    let sign = if year < 0 { "-" } else { "" };
    format!(
        "{sign}{:04}-{:02}-{:02}",
        year.abs(),
        day.month(),
        day.day()
    )
}

/// serde adapter for a calendar day (`2026-09-07`) that must be present.
pub mod day {
    use super::*;

    pub fn serialize<S: Serializer>(value: &Date, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::format_day(*value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
        let raw = String::deserialize(deserializer)?;
        super::parse_day(&raw)
            .ok_or_else(|| de::Error::invalid_value(Unexpected::Str(&raw), &"a YYYY-MM-DD day"))
    }
}

/// [`day`] for an `Option`, written `null` when absent.
pub mod day_optional {
    use super::*;

    pub fn serialize<S: Serializer>(
        value: &Option<Date>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(day) => serializer.serialize_str(&super::format_day(*day)),
            None => serializer.serialize_none(),
        }
    }
}

/// The instant that represents an all-day task due on `day`.
pub fn all_day_instant(day: Date) -> Instant {
    Offset::UTC
        .to_timestamp(day.to_datetime(jiff::civil::Time::midnight()))
        // Midnight UTC is an instant on every day jiff can name but its first (the year -9999 starts
        // a few hours west of what it can represent), so that edge reads as the earliest instant
        // rather than panicking.
        .unwrap_or(Instant::MIN)
}

/// The calendar day an all-day instant means.
///
/// **Always read in UTC**, whatever the reader's offset. Reading it locally moves the date for
/// anyone west of UTC — a 25 December task prints as the 24th. The reader's own day enters the
/// comparison on the other side, not here; see the module note.
pub fn all_day_date(at: Instant) -> Date {
    Offset::UTC.to_datetime(at).date()
}

/// The instant to store for an all-day task due on the reader's today.
///
/// Their calendar day, re-expressed as midnight UTC — `getLocalDateAsUTCMidnight` on web. Using
/// `now`'s UTC date instead would file a task created at 22:00 in California under tomorrow, every
/// evening.
pub fn all_day_today(now: Instant, offset: Offset) -> Instant {
    all_day_instant(offset.to_datetime(now).date())
}

/// serde adapter for `Option<Instant>` fields, which is nearly all of them.
///
/// Absent, `null`, an empty string and an unparseable string all read as `None`. That last one is
/// deliberate leniency: one malformed `createdAt` on one comment must not fail the decode of the
/// response it arrived in. The field it lands in is optional on every model for exactly that
/// reason.
pub mod optional {
    use super::*;

    pub fn serialize<S: Serializer>(
        value: &Option<Instant>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(at) => serializer.serialize_str(&super::format(*at)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Instant>, D::Error> {
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw.as_deref().and_then(super::parse))
    }
}

/// serde adapter for the rare field that must be present, used by the sync cursor.
pub mod required {
    use super::*;

    pub fn serialize<S: Serializer>(value: &Instant, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::format(*value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Instant, D::Error> {
        let raw = String::deserialize(deserializer)?;
        super::parse(&raw)
            .ok_or_else(|| de::Error::invalid_value(Unexpected::Str(&raw), &"an ISO-8601 instant"))
    }
}

#[cfg(test)]
mod tests {
    /// Two completions made in the same second must still sort — a completion stamped to the
    /// whole second collided with its neighbour on the Apple apps (AITD-369), so an instant that
    /// carries milliseconds keeps them. One that does not is written as it always was.
    #[test]
    fn an_instant_keeps_its_milliseconds_and_a_whole_second_stays_whole() {
        let precise = parse("2026-09-28T12:00:00.123Z").expect("parses");
        assert_eq!(format(precise), "2026-09-28T12:00:00.123Z");
        let whole = parse("2026-09-28T12:00:00Z").expect("parses");
        assert_eq!(format(whole), "2026-09-28T12:00:00Z");
    }

    use super::*;

    #[test]
    fn adev_80_a_day_is_read_with_or_without_leading_zeros_and_nothing_else() {
        assert_eq!(parse_day("2026-01-05"), Date::new(2026, 1, 5).ok());
        assert_eq!(parse_day("2026-1-5"), Date::new(2026, 1, 5).ok());
        assert_eq!(
            parse_day("2026-02-30"),
            None,
            "there is no 30th of February"
        );
        assert_eq!(parse_day("2026-01-05T00:00:00Z"), None);
        assert_eq!(parse_day("garbage"), None);
        assert_eq!(parse_day(""), None);
    }

    /// What `DateTime::parse_from_rfc3339` read, this reads — and what it refused, this refuses. The
    /// reader is written here rather than borrowed from jiff, so the edges are pinned.
    #[test]
    fn adev_80_the_reader_takes_rfc_3339_and_nothing_wider() {
        let utc = |text: &str| parse(text).map(format);
        assert_eq!(
            utc("2026-09-07T12:34:56z").as_deref(),
            Some("2026-09-07T12:34:56Z")
        );
        assert_eq!(
            utc("2026-09-07 12:34:56Z").as_deref(),
            Some("2026-09-07T12:34:56Z")
        );
        assert_eq!(
            utc("2026-09-07t05:04:56-07:30").as_deref(),
            Some("2026-09-07T12:34:56Z")
        );
        assert_eq!(
            utc("2026-09-07T12:34:56.1234567891Z").as_deref(),
            Some("2026-09-07T12:34:56.123Z"),
            "digits past a nanosecond are dropped, not refused"
        );
        assert_eq!(
            utc("2026-09-07T12:34:60Z").as_deref(),
            Some("2026-09-07T12:34:59Z")
        );
        for refused in [
            "2026-09-07T12:34:56",
            "2026-09-07T12:34:56Z trailing",
            "2026-09-07T12:34:56.Z",
            "2026-09-07T12:34:56+24:00",
            "2026-09-07T12:34:56+0100",
            "2026-09-07T12:34:56[America/New_York]",
            "2026-W37-1T12:34:56Z",
            "26-09-07T12:34:56Z",
        ] {
            assert_eq!(parse(refused), None, "{refused}");
        }
    }

    /// The text this module writes for the instants chrono wrote, down to the digits.
    #[test]
    fn adev_80_it_writes_the_fractions_chrono_wrote() {
        let at = |text: &str| parse(text).expect("an instant");
        assert_eq!(
            rfc3339_z(at("2026-09-07T12:00:00Z")),
            "2026-09-07T12:00:00Z"
        );
        assert_eq!(
            rfc3339_z(at("2026-09-07T12:00:00.12Z")),
            "2026-09-07T12:00:00.120Z"
        );
        assert_eq!(
            rfc3339_z(at("2026-09-07T12:00:00.123456Z")),
            "2026-09-07T12:00:00.123456Z"
        );
        assert_eq!(
            rfc3339_offset(at("2026-09-07T12:00:00.5Z")),
            "2026-09-07T12:00:00.500+00:00"
        );
        assert_eq!(
            rfc3339_offset(at("2026-09-07T12:00:00Z")),
            "2026-09-07T12:00:00+00:00"
        );
    }

    #[test]
    fn it_reads_the_milliseconds_prisma_writes_and_keeps_them() {
        let at = parse("2026-09-07T12:34:56.789Z").expect("parses");
        assert_eq!(format(at), "2026-09-07T12:34:56.789Z");
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

    /// Nothing finer than the millisecond the server stores; a whole second is written whole, as
    /// the Apple apps write a due date.
    #[test]
    fn it_writes_at_most_milliseconds_with_a_z() {
        let at = parse("2026-01-02T03:04:05.123456Z").expect("parses");
        assert_eq!(format(at), "2026-01-02T03:04:05.123Z");
        let whole = parse("2026-01-02T03:04:05Z").expect("parses");
        assert_eq!(format(whole), "2026-01-02T03:04:05Z");
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
        let day = Date::new(2026, 9, 7).expect("a real day");
        assert_eq!(format(all_day_instant(day)), "2026-09-07T00:00:00Z");
    }

    /// The reader's day, not UTC's. A task created at 22:00 in California is due today for the
    /// person creating it; `now`'s UTC date would file it under tomorrow every evening.
    #[test]
    fn todays_all_day_date_is_the_readers_day_rather_than_utcs() {
        let california = Offset::from_seconds(-7 * 3600).expect("an offset");
        // 22:00 on the 7th in California is 05:00 on the 8th in UTC.
        let evening = parse("2026-09-08T05:00:00Z").expect("an instant");
        assert_eq!(
            format(all_day_today(evening, california)),
            "2026-09-07T00:00:00Z"
        );
    }

    #[test]
    fn an_all_day_instant_round_trips_through_its_day() {
        let day = Date::new(2028, 2, 29).expect("2028 is a leap year");
        assert_eq!(all_day_date(all_day_instant(day)), day);
    }
}
