//! Replays `contracts/fixtures/questions.json` through the rules door (AWTD-1687).
//!
//! A **core-first** fixture: generated here (`cargo xtask gen-core-contracts`) from
//! `astrid_rules::questions`. astrid-web replays the same file against its Node package.
//!
//! The question rules are server-side writes, left out of the browser build (AWTD2-162).
#![cfg(feature = "rules-waiting")]

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/questions.json");

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
fn every_question_case_answers_as_recorded_awtd_1687() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 13, "the fixture lost its cases");
    for case in fixture.cases {
        let answer: serde_json::Value =
            serde_json::from_str(&astrid_rules::rules::run_json(&case.request.to_string()))
                .expect("the door answers JSON");
        assert_eq!(answer, case.answer, "{}", case.name);
    }
}
