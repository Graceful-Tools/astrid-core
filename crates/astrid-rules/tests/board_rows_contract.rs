//! Replays `contracts/fixtures/board-rows.json` through the rules door (AWTD-1676).
//!
//! A **core-first** fixture: generated here (`cargo xtask gen-core-contracts`), but its
//! expectations are astrid-web's own test cases for `lib/project-board-rows.ts`
//! (`tests/lib/project-board-rows-awtd-1660.test.ts`) and the Mac's `ProjectBoardRowsTests`,
//! written out in the generator, so web can replay the same file against its TypeScript and the
//! Apple apps against the same door through UniFFI. D67 in `docs/CONTRACTS.md` records the rule
//! for a row list from another project or from none.

use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/board-rows.json");

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
fn every_board_rows_case_answers_as_recorded_awtd_1676() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 9, "the fixture lost its cases");
    for case in fixture.cases {
        let answer: serde_json::Value =
            serde_json::from_str(&astrid_rules::rules::run_json(&case.request.to_string()))
                .expect("the door answers JSON");
        assert_eq!(answer, case.answer, "{}", case.name);
    }
}

/// Both halves are locked: which lists are rows, and what one row draws. A file that dropped
/// either would pass the replay above while leaving that half to the clients' own copies.
#[test]
fn the_fixture_covers_the_rows_and_a_row_awtd_1676() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("the fixture parses");
    let kinds: Vec<&str> = fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter_map(|case| case["request"]["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"boardRows"));
    assert!(kinds.contains(&"boardRow"));
}
