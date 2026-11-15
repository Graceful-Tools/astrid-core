//! Replays `contracts/fixtures/sidebar-projects.json` through the rules door (AWTD-1681).
//!
//! A **core-first** fixture: generated here (`cargo xtask gen-core-contracts`). The behaviour is
//! astrid-web's `ProjectsSection.tsx` (AWTD-1660); the decisions it left implicit — which projects,
//! in what order, which lists under each, a favourite inside one, an archived one — are
//! docs/CONTRACTS.md D70, written out as cases. Web, Windows and the Apple apps adopt against this
//! table rather than grouping lists themselves.

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/sidebar-projects.json");

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
fn every_sidebar_projects_case_answers_as_recorded_awtd_1681() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 8, "the fixture lost its cases");
    for case in fixture.cases {
        let answer: serde_json::Value =
            serde_json::from_str(&astrid_rules::rules::run_json(&case.request.to_string()))
                .expect("the door answers JSON");
        assert_eq!(answer, case.answer, "{}", case.name);
    }
}
