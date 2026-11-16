//! Replays `contracts/fixtures/orchestration.json` through the rules door (AWTD-1685).
//!
//! A **core-first** fixture: generated here (`cargo xtask gen-core-contracts`) from
//! `astrid_rules::orchestration`. Astrid's own fixall Autonomy rules are one stored project policy
//! in it (`astridPolicy`). astrid-web replays the same file against its Node package, which its
//! server asks wherever an agent acts. The browser build does not carry these rules.
#![cfg(feature = "orchestration")]

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/orchestration.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    cases: Vec<Case>,
    astrid_policy: serde_json::Value,
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
fn every_orchestration_case_answers_as_recorded_awtd_1685() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 28, "the fixture lost its cases");
    for case in fixture.cases {
        assert_eq!(ask(&case.request), case.answer, "{}", case.name);
    }
}

/// The fixture's `astridPolicy` is a stored policy the rules read, not prose: additive migrations
/// ship and destructive ones ask (astrid-web docs/FIXALL_WORKFLOW.md → Autonomy).
#[test]
fn astrids_stored_policy_is_one_the_rules_read_awtd_1685() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    let decide = |migration: &str| {
        ask(&serde_json::json!({
            "kind": "orchestrationMay", "project": fixture.astrid_policy,
            "agent": "claude", "action": "migrate", "migration": migration
        }))["value"]["decision"]
            .clone()
    };
    assert_eq!(decide("additive"), "allowed");
    assert_eq!(decide("destructive"), "ask");
}
