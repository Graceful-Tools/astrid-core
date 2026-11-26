//! A task's goal links, estimate and experiment results, read-only (ADEV-285).

use super::*;
use std::sync::{Arc, Mutex};

fn offline() -> Result<crate::api::HttpResponse, crate::api::TransportError> {
    Err(crate::api::TransportError::Unreachable("offline".into()))
}

fn estimate_body() -> serde_json::Value {
    json!({
        "taskId": "t1",
        "latest": {
            "id": "e1", "source": "builtin_llm", "rough": true, "confidence": "low",
            "goalImpacts": [{ "goalId": "g1", "low": 1, "high": 3 }],
            "risk": { "level": "low" },
            "rationale": "Small change", "label": "do-now",
            "labelReasons": [{ "code": "guardrailAtReview", "guardrailId": "r1" }],
            "createdAt": "2026-11-26T10:00:00.000Z", "futureField": 1
        },
        "history": [],
        "meta": { "apiVersion": "v1" }
    })
}

/// ADEV-285: links, estimate and experiment are read, cached for an offline board, and announced
/// once per cache that moved.
#[tokio::test]
async fn adev_285_goal_signals_are_read_cached_and_announced_once() {
    let transport = StubTransport::new()
        .push_json(
            "/api/v1/tasks/t1/goals",
            200,
            json!({ "taskId": "t1", "links": [{
                "projectId": "p1", "projectName": "Fund", "goalId": "g1",
                "statement": "Grow", "via": "list", "listId": "l1", "source": "person" }] }),
        )
        .push("/api/v1/tasks/t1/goals", offline())
        .push_json("/api/v1/tasks/t1/estimates", 200, estimate_body())
        .push_json("/api/v1/tasks/t1/estimates", 200, estimate_body())
        .push_json(
            "/api/v1/tasks/t1/experiment",
            200,
            json!({ "experiments": [{
                "id": "x1", "kind": "experiment", "connectorId": "c1", "externalKey": "k",
                "state": "concluded", "verdict": "improving",
                "recommendation": { "action": "stop", "reason": "done" },
                "stale": false }], "inResultsColumn": true }),
        )
        .push("/api/v1/tasks/t1/experiment", offline());
    let app = app_with(transport);
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    app.realtime().on_change(move |change| {
        sink.lock().expect("lock").push(change.to_json());
    });

    let links = call(&app, json!({ "kind": "getTaskGoalLinks", "taskId": "t1" })).await;
    assert_eq!(links["ok"], true, "{links}");
    assert_eq!(links["value"]["links"][0]["via"], "list");
    let cached = call(&app, json!({ "kind": "getTaskGoalLinks", "taskId": "t1" })).await;
    assert_eq!(cached["value"], links["value"], "offline answers the cache");

    let first = call(&app, json!({ "kind": "getTaskEstimate", "taskId": "t1" })).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["value"]["latest"]["label"], "do-now");
    assert_eq!(first["value"]["latest"]["rough"], true);
    assert_eq!(
        first["value"]["latest"]["labelReasons"][0]["code"],
        "guardrailAtReview"
    );
    let same = call(&app, json!({ "kind": "getTaskEstimate", "taskId": "t1" })).await;
    assert_eq!(same["value"], first["value"]);

    let exp = call(&app, json!({ "kind": "getTaskExperiment", "taskId": "t1" })).await;
    assert_eq!(exp["ok"], true, "{exp}");
    assert_eq!(exp["value"]["inResultsColumn"], true);
    assert_eq!(exp["value"]["experiments"][0]["verdict"], "improving");
    let exp_cached = call(&app, json!({ "kind": "getTaskExperiment", "taskId": "t1" })).await;
    assert_eq!(exp_cached["value"], exp["value"]);

    let events = heard.lock().expect("lock").clone();
    let moved: Vec<_> = events
        .iter()
        .filter(|e| e.contains("taskGoalSignals"))
        .collect();
    assert_eq!(moved.len(), 3, "once per cache that moved: {events:?}");
}

/// ADEV-285: a server refusal (capability off) is a failure and leaves the cache alone.
#[tokio::test]
async fn adev_285_a_refusal_is_a_failure_and_keeps_the_cache() {
    let transport = StubTransport::new()
        .push_json("/api/v1/tasks/t1/estimates", 200, estimate_body())
        .push_json(
            "/api/v1/tasks/t1/estimates",
            403,
            json!({ "error": "not_granted" }),
        )
        .push("/api/v1/tasks/t1/estimates", offline());
    let app = app_with(transport);
    let ok = call(&app, json!({ "kind": "getTaskEstimate", "taskId": "t1" })).await;
    assert_eq!(ok["ok"], true, "{ok}");
    let refused = call(&app, json!({ "kind": "getTaskEstimate", "taskId": "t1" })).await;
    assert_eq!(refused["ok"], false, "{refused}");
    let cached = call(&app, json!({ "kind": "getTaskEstimate", "taskId": "t1" })).await;
    assert_eq!(cached["value"], ok["value"]);
}
