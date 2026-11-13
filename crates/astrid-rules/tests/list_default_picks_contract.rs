//! Replays `contracts/fixtures/list-default-picks.json` through the rules door (task 92830d81).
//!
//! A **core-first** fixture, like `waiting.json`: generated from this crate
//! (`cargo xtask gen-core-contracts`) rather than from astrid-web, because web spells its repeat
//! and when options out as inline `SelectItem`s rather than as a table an exporter could read, so
//! there is nothing there to execute. Each case is a request to `rules::run_json` — the function
//! the WebAssembly build exports as `runJson` — and the answer it must give. The xtask's own test
//! fails when the file is stale; this one fails when the door answers a recorded case differently,
//! whatever the generator says.

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/list-default-picks.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    request: serde_json::Value,
    answer: serde_json::Value,
}

#[test]
fn every_list_default_picks_case_answers_as_recorded() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 8, "the fixture lost its cases");
    for case in fixture.cases {
        let answer: serde_json::Value =
            serde_json::from_str(&astrid_rules::rules::run_json(&case.request.to_string()))
                .expect("the door answers JSON");
        assert_eq!(answer, case.answer, "{}", case.name);
    }
}

/// Every picker a client used to hold a table for is in the fixture, with a tick in each. A file
/// that dropped one would pass the replay above while leaving that client's table unlocked.
#[test]
fn all_four_pickers_are_covered_and_each_has_exactly_one_tick() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("the fixture parses");
    let cases = fixture["cases"].as_array().expect("cases");

    for picker in ["priority", "repeating", "dueDate", "dueTime"] {
        let drawn = cases
            .iter()
            .filter(|case| case["answer"]["value"][picker].is_array())
            .count();
        assert_eq!(drawn, cases.len(), "{picker} is missing from a case");
    }

    for case in cases {
        let name = case["name"].as_str().unwrap_or_default();
        // The one deliberate exception: a stored value the picker cannot draw ticks nothing,
        // rather than quietly moving the person's choice to the first option.
        let expected = usize::from(!name.starts_with("unrecognised/"));
        for picker in ["priority", "repeating", "dueDate"] {
            let ticks = case["answer"]["value"][picker]
                .as_array()
                .expect(picker)
                .iter()
                .filter(|pick| pick["isSelected"] == serde_json::Value::Bool(true))
                .count();
            assert_eq!(ticks, expected, "{name} / {picker}");
        }
        // The time picker always has one: all-day is a choice, so there is no unset state.
        let ticks = case["answer"]["value"]["dueTime"]
            .as_array()
            .expect("dueTime")
            .iter()
            .filter(|pick| pick["isSelected"] == serde_json::Value::Bool(true))
            .count();
        assert_eq!(ticks, 1, "{name} / dueTime");
    }
}
