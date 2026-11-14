//! What every Phase-0 contract test shares: reading a fixture's `disputed` cases.
//!
//! A web-generated fixture routes the cases the clients deliberately disagree on — the core
//! following iOS where web has not moved yet (`docs/CONTRACTS.md`, top) — to `disputed`, each
//! naming its CONTRACTS.md entry (astrid-web `scripts/contract-fixtures/drivers/disputed.mjs`). A test runs those through the
//! core too, and requires that **each entry still disagrees with web somewhere**. When web adopts
//! iOS's behaviour every case of an entry starts agreeing, this fails, and the exclusion comes
//! out of the driver: an exclusion cannot outlive its reason.

#![allow(dead_code)]

use std::collections::BTreeMap;

/// One disputed case after the core has answered it.
pub struct Answered {
    pub entry: String,
    pub id: String,
    pub agrees_with_web: bool,
}

/// Fail if any declared entry has no case left that disagrees with web.
pub fn each_dispute_still_disagrees(fixture: &str, declared: &[String], answered: &[Answered]) {
    let mut disagreeing: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in declared {
        disagreeing.insert(entry.as_str(), 0);
    }
    for case in answered {
        let count = disagreeing.get_mut(case.entry.as_str()).unwrap_or_else(|| {
            panic!(
                "{fixture}: case {} names {}, which `disputes` does not declare",
                case.id, case.entry
            )
        });
        if !case.agrees_with_web {
            *count += 1;
        }
    }
    let settled: Vec<&str> = disagreeing
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(entry, _)| *entry)
        .collect();
    assert!(
        settled.is_empty(),
        "{fixture}: every disputed case of {settled:?} now agrees with web — the divergence has \
         closed, so take the dispute out of its driver and regenerate"
    );
}

/// The `entry` names a fixture's `disputes` array declares.
pub fn declared(disputes: &serde_json::Value) -> Vec<String> {
    disputes
        .as_array()
        .expect("disputes is an array")
        .iter()
        .map(|d| d["entry"].as_str().expect("an entry").to_string())
        .collect()
}

/// Assert no failures, listing every one rather than stopping at the first.
pub fn no_failures(fixture: &str, failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{fixture}: {} case(s) answered differently from web:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Ask the stateless door, and insist it answered.
///
/// Every contract fixture whose rule the door exposes is driven through here rather than through
/// the Rust function, because a fixture that only ever reaches the function proves nothing about
/// the door web and the VS Code extension actually call — which was the whole finding of task
/// `c2109ff8`: ten rules' worth of list pipeline existed in `astrid-rules` and was reachable only
/// by a client that links Rust.
#[allow(dead_code)]
pub fn ask(request: serde_json::Value) -> serde_json::Value {
    let answer: serde_json::Value =
        serde_json::from_str(&astrid_rules::rules::run_json(&request.to_string()))
            .expect("the door answers JSON");
    assert_eq!(
        answer["ok"], true,
        "the door refused {request}: {}",
        answer["error"]
    );
    answer["value"].clone()
}
