//! Replays `contracts/fixtures/list-project-moves.json` through the rules door (astrid-ios
//! AITD-680).
//!
//! A **core-first** fixture: generated here (`cargo xtask gen-core-contracts`), its expectations
//! D68's permission table written out — owner, admin, member and viewer of the list by owner,
//! admin, member, viewer and stranger of the project. astrid-web replays the same file against
//! its server's checks (AWTD-1702) and the Apple apps reach the same door through UniFFI.

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/list-project-moves.json");

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
fn every_list_project_moves_case_answers_as_recorded_aitd_680() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 12, "the fixture lost its cases");
    for case in fixture.cases {
        let answer: serde_json::Value =
            serde_json::from_str(&astrid_rules::rules::run_json(&case.request.to_string()))
                .expect("the door answers JSON");
        assert_eq!(answer, case.answer, "{}", case.name);
    }
}
