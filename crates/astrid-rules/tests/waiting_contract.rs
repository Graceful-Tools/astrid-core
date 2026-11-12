//! Replays `contracts/fixtures/waiting.json` through the rules door.
//!
//! The first **core-first** fixture: generated from this crate (`cargo xtask gen-core-contracts`),
//! not from astrid-web, because the Waiting rules are canonical here (Jon, 2026-11-12). Each case
//! is a request to `rules::run_json` — the function the WebAssembly build exports as `runJson` —
//! and the answer it must give. Web's TypeScript mirror and the native apps are held to the same
//! file. The xtask's own test fails when the file is stale; this one fails when the door answers a
//! recorded case differently, whatever the generator says.

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/waiting.json");

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
fn every_waiting_case_answers_as_recorded() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 40, "the fixture lost its cases");
    for case in fixture.cases {
        let answer: serde_json::Value =
            serde_json::from_str(&astrid_rules::rules::run_json(&case.request.to_string()))
                .expect("the door answers JSON");
        assert_eq!(answer, case.answer, "{}", case.name);
    }
}

/// The rule names every case cites — so a fixture that silently dropped a rule's cases fails.
#[test]
fn every_rule_is_covered() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("the fixture parses");
    let rules: std::collections::BTreeSet<&str> = fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter_map(|case| case["rule"].as_str())
        .collect();
    for rule in ["R2", "R3", "R4", "R5", "R7"] {
        assert!(rules.iter().any(|cited| cited.contains(rule)), "{rule}");
    }
}
