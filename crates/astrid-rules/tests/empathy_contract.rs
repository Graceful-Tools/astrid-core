//! Replays `contracts/fixtures/empathy-art.json` — generated from astrid-web's
//! `lib/empathy-art/rules.ts` — against this crate's port (ADEV-221).

use astrid_rules::empathy;
use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/empathy-art.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    metaphors: Vec<String>,
    states: Vec<String>,
    hashes: Vec<Hash>,
    rotation: Vec<Rotation>,
    columns: Vec<Column>,
}

#[derive(Deserialize)]
struct Hash {
    text: String,
    fnv1a32: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rotation {
    list_id: String,
    at: String,
    #[serde(rename = "daysSinceEpochUTC")]
    days_since_epoch_utc: i64,
    metaphor: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Column {
    completed: bool,
    status_role: Option<String>,
    state: Option<String>,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("empathy-art.json parses")
}

/// `2026-11-19T23:59:59.999Z` as epoch milliseconds, without a date library.
fn epoch_ms(iso: &str) -> i64 {
    let (date, time) = iso.trim_end_matches('Z').split_once('T').unwrap();
    let mut d = date.split('-').map(|part| part.parse::<i64>().unwrap());
    let (year, month, day) = (d.next().unwrap(), d.next().unwrap(), d.next().unwrap());
    // Days from civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let (clock, millis) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = clock.split(':').map(|part| part.parse::<i64>().unwrap());
    let seconds = t.next().unwrap() * 3600 + t.next().unwrap() * 60 + t.next().unwrap();
    days * 86_400_000 + seconds * 1000 + millis.parse::<i64>().unwrap()
}

#[test]
fn the_metaphors_and_states_are_web_s_in_web_s_order() {
    let fixture = fixture();
    assert_eq!(fixture.metaphors, empathy::METAPHORS);
    assert_eq!(fixture.states, empathy::STATES);
}

#[test]
fn fnv1a_matches_web() {
    for hash in fixture().hashes {
        assert_eq!(
            empathy::fnv1a32(&hash.text),
            hash.fnv1a32,
            "{:?}",
            hash.text
        );
    }
}

#[test]
fn the_daily_rotation_matches_web() {
    for row in fixture().rotation {
        let at = epoch_ms(&row.at);
        assert_eq!(
            empathy::days_since_epoch_utc(at),
            row.days_since_epoch_utc,
            "{}",
            row.at
        );
        assert_eq!(
            empathy::metaphor_for(&row.list_id, at),
            row.metaphor,
            "{} at {}",
            row.list_id,
            row.at
        );
    }
}

#[test]
fn a_column_maps_to_the_state_web_maps_it_to() {
    for column in fixture().columns {
        assert_eq!(
            empathy::state_for_column(column.completed, column.status_role.as_deref()),
            column.state.as_deref(),
            "{:?}/{:?}",
            column.completed,
            column.status_role
        );
    }
}
