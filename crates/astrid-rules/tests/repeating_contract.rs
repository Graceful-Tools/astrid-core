//! Replays `contracts/fixtures/repeating.json` — every case run through astrid-web's own
//! calculator — against this crate's port.
//!
//! The unit tests in `src/repeating` say what the rules are. This says the rules agree with the
//! server's, which is the only thing a user notices: two clients that disagree about when a
//! repeating task is next due will overwrite each other, and the last writer wins.
//!
//! Regenerate in astrid-web (`npm run export:contract-fixtures`) and copy them here; `cargo xtask check-contracts` fails when
//! web has moved and this has not.
//!
//! `completions` and `zonedProgressions` go through the rules door itself (`nextOccurrence`, the
//! request astrid-web's server sends since AWTD-1063) and carry the person's zone: web adopted this
//! crate's answers for D1, D3 and D4 on 2026-10-03, so those cases are no longer disputed.

// The two tests that go through the rules door need `rules-repeating`; without it (the browser
// build) the fixture sections only they read are unused.
#![cfg_attr(not(feature = "rules-repeating"), allow(dead_code))]

use astrid_rules::repeating::{
    calculate_custom_next_occurrence, calculate_simple_next_occurrence,
    check_simple_pattern_end_condition, CustomRepeatingPattern, RepeatFrom, Repeating,
    SimplePatternEndCondition,
};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/repeating.json");

/// How the fixture's instants are read: through the model's own reader, because `jiff` is built
/// without its `serde` feature (a field that forgot its adapter must not compile).
mod at {
    use astrid_rules::model::date;
    use jiff::Timestamp;
    use serde::{de, Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Timestamp, D::Error> {
        date::required::deserialize(deserializer)
    }

    pub mod option {
        use super::*;

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<Timestamp>, D::Error> {
            date::exact_optional::deserialize(deserializer)
        }
    }

    pub mod many {
        use super::*;

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Vec<Timestamp>, D::Error> {
            Vec::<String>::deserialize(deserializer)?
                .iter()
                .map(|text| date::parse(text).ok_or_else(|| de::Error::custom("an instant")))
                .collect()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    simple: Vec<SimpleCase>,
    simple_end_conditions: Vec<EndConditionCase>,
    custom: Vec<CustomCase>,
    custom_progressions: Vec<ProgressionCase>,
    completions: Vec<CompletionCase>,
    zoned_progressions: Vec<ZonedProgressionCase>,
}

/// One `nextOccurrence` request, as web's server sends it, and web's answer.
#[derive(Deserialize)]
struct CompletionCase {
    name: String,
    request: serde_json::Map<String, serde_json::Value>,
    answer: Answer,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Answer {
    #[serde(default, with = "at::option")]
    next_due_date: Option<Timestamp>,
    should_terminate: bool,
    new_occurrence_count: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZonedProgressionCase {
    name: String,
    repeating: String,
    pattern: Option<serde_json::Value>,
    start: String,
    time_zone: String,
    is_all_day: bool,
    steps: usize,
    #[serde(with = "at::many")]
    dates: Vec<Timestamp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimpleCase {
    name: String,
    repeating_type: Repeating,
    #[serde(default, with = "at::option")]
    current_due_date: Option<Timestamp>,
    #[serde(with = "at")]
    completion_date: Timestamp,
    repeat_from: RepeatFrom,
    #[serde(default, with = "at::option")]
    next_due_date: Option<Timestamp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndConditionCase {
    name: String,
    #[serde(with = "at")]
    next_due_date: Timestamp,
    new_occurrence_count: i32,
    end_data: SimplePatternEndCondition,
    should_terminate: bool,
    result_occurrence_count: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CustomCase {
    name: String,
    pattern: CustomRepeatingPattern,
    #[serde(with = "at")]
    current_due_date: Timestamp,
    #[serde(with = "at")]
    completion_date: Timestamp,
    repeat_from: RepeatFrom,
    current_occurrence_count: i32,
    #[serde(default, with = "at::option")]
    next_due_date: Option<Timestamp>,
    should_terminate: bool,
    new_occurrence_count: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgressionCase {
    name: String,
    pattern: CustomRepeatingPattern,
    #[serde(with = "at")]
    start: Timestamp,
    repeat_from: RepeatFrom,
    steps: usize,
    #[serde(with = "at::many")]
    dates: Vec<Timestamp>,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("contracts/fixtures/repeating.json is malformed")
}

#[test]
fn simple_patterns_match_web() {
    for case in fixture().simple {
        let result = calculate_simple_next_occurrence(
            case.repeating_type,
            case.current_due_date,
            case.completion_date,
            case.repeat_from,
            0,
            None,
            &TimeZone::UTC,
        );
        assert_eq!(
            result.next_due_date, case.next_due_date,
            "simple pattern diverged from web: {}",
            case.name
        );
    }
}

#[test]
fn simple_end_conditions_match_web() {
    for case in fixture().simple_end_conditions {
        let (should_terminate, count) = check_simple_pattern_end_condition(
            case.next_due_date,
            case.new_occurrence_count,
            &case.end_data,
            &TimeZone::UTC,
        );
        assert_eq!(
            should_terminate, case.should_terminate,
            "end condition diverged from web: {}",
            case.name
        );
        assert_eq!(
            count, case.result_occurrence_count,
            "occurrence count diverged from web: {}",
            case.name
        );
    }
}

#[test]
fn custom_patterns_match_web() {
    for case in fixture().custom {
        let result = calculate_custom_next_occurrence(
            &case.pattern,
            Some(case.current_due_date),
            case.completion_date,
            case.repeat_from,
            case.current_occurrence_count,
            &TimeZone::UTC,
        );
        assert_eq!(
            result.next_due_date, case.next_due_date,
            "custom pattern diverged from web: {}",
            case.name
        );
        assert_eq!(
            result.should_terminate, case.should_terminate,
            "termination diverged from web: {}",
            case.name
        );
        assert_eq!(
            result.new_occurrence_count, case.new_occurrence_count,
            "occurrence count diverged from web: {}",
            case.name
        );
    }
}

/// The case that matters most. A single step can agree by accident; six in a row cannot, and the
/// weekly Mon/Wed/Fri bug that prompted this whole module only appeared on the second step.
#[test]
fn custom_progressions_match_web() {
    for case in fixture().custom_progressions {
        let mut dates = Vec::new();
        let mut current = case.start;
        let mut occurrences = 0;
        for _ in 0..case.steps {
            let result = calculate_custom_next_occurrence(
                &case.pattern,
                Some(current),
                current,
                case.repeat_from,
                occurrences,
                &TimeZone::UTC,
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
        assert_eq!(
            dates, case.dates,
            "progression diverged from web: {}",
            case.name
        );
    }
}

/// Ask the rules door, as astrid-web's server does.
#[cfg(feature = "rules-repeating")]
fn next_occurrence(mut request: serde_json::Map<String, serde_json::Value>) -> Answer {
    request.insert("kind".into(), "nextOccurrence".into());
    let reply: serde_json::Value = serde_json::from_str(&astrid_rules::rules::run_json(
        &serde_json::Value::Object(request).to_string(),
    ))
    .expect("the door answers JSON");
    assert_eq!(reply["ok"], true, "the door refused: {reply}");
    serde_json::from_value(reply["value"].clone()).expect("a next occurrence")
}

#[cfg(feature = "rules-repeating")]
#[test]
fn completions_through_the_rules_door_match_web() {
    let cases = fixture().completions;
    assert!(cases.len() >= 40, "the completions section shrank");
    for case in cases {
        assert_eq!(
            next_occurrence(case.request),
            case.answer,
            "nextOccurrence diverged from web: {}",
            case.name
        );
    }
}

/// Several completions in a row on the person's calendar, through a daylight-saving change.
#[cfg(feature = "rules-repeating")]
#[test]
fn zoned_progressions_match_web() {
    for case in fixture().zoned_progressions {
        let mut dates = Vec::new();
        let mut current = case.start.clone();
        let mut occurrences = 0;
        for _ in 0..case.steps {
            let request = serde_json::json!({
                "repeating": case.repeating,
                "pattern": case.pattern,
                "currentDueDate": current,
                "completion": current,
                "repeatFrom": "DUE_DATE",
                "occurrenceCount": occurrences,
                "timeZone": case.time_zone,
                "isAllDay": case.is_all_day,
            });
            let serde_json::Value::Object(request) = request else {
                unreachable!()
            };
            let answer = next_occurrence(request);
            let Some(next) = answer.next_due_date else {
                break;
            };
            dates.push(next);
            current = next.to_string();
            occurrences = answer.new_occurrence_count;
        }
        assert_eq!(
            dates, case.dates,
            "zoned progression diverged from web: {}",
            case.name
        );
    }
}
