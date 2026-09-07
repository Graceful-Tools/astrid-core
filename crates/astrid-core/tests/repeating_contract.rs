//! Replays `contracts/fixtures/repeating.json` — every case run through astrid-web's own
//! calculator — against this crate's port.
//!
//! The unit tests in `src/repeating` say what the rules are. This says the rules agree with the
//! server's, which is the only thing a user notices: two clients that disagree about when a
//! repeating task is next due will overwrite each other, and the last writer wins.
//!
//! Regenerate with `node contracts/export-from-web.mjs`; `cargo xtask check-contracts` fails when
//! web has moved and this has not.

use astrid_core::repeating::{
    calculate_custom_next_occurrence, calculate_simple_next_occurrence,
    check_simple_pattern_end_condition, CustomRepeatingPattern, RepeatFrom, Repeating,
    SimplePatternEndCondition,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/repeating.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    simple: Vec<SimpleCase>,
    simple_end_conditions: Vec<EndConditionCase>,
    custom: Vec<CustomCase>,
    custom_progressions: Vec<ProgressionCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimpleCase {
    name: String,
    repeating_type: Repeating,
    current_due_date: Option<DateTime<Utc>>,
    completion_date: DateTime<Utc>,
    repeat_from: RepeatFrom,
    next_due_date: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndConditionCase {
    name: String,
    next_due_date: DateTime<Utc>,
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
    current_due_date: DateTime<Utc>,
    completion_date: DateTime<Utc>,
    repeat_from: RepeatFrom,
    current_occurrence_count: i32,
    next_due_date: Option<DateTime<Utc>>,
    should_terminate: bool,
    new_occurrence_count: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgressionCase {
    name: String,
    pattern: CustomRepeatingPattern,
    start: DateTime<Utc>,
    repeat_from: RepeatFrom,
    steps: usize,
    dates: Vec<DateTime<Utc>>,
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
