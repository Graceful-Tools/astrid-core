//! Replays `contracts/fixtures/tradeoff.json` through the rules door (ADEV-273).
//!
//! A **core-first** fixture, generated here (`cargo xtask gen-core-contracts`) from
//! `astrid_rules::tradeoff` and the `policy` arm of `filters::sort_by_setting`. astrid-web replays
//! the same file against its Node package, which evaluates an estimate and stores the key the sort
//! compares. The browser build carries the sort and not the evaluation.
#![cfg(feature = "tradeoff")]

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/tradeoff.json");

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

fn ask(request: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&astrid_rules::rules::run_json(&request.to_string()))
        .expect("the door answers JSON")
}

#[test]
fn every_tradeoff_case_answers_as_recorded_adev_273() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 20, "the fixture lost its cases");
    for case in fixture.cases {
        assert_eq!(ask(&case.request), case.answer, "{}", case.name);
    }
}
