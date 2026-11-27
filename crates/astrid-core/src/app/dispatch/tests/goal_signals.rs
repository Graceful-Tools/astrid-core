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

fn link(goal: &str, via: &str) -> serde_json::Value {
    json!({ "projectId": "p1", "projectName": "Fund", "goalId": goal,
            "statement": format!("Goal {goal}"), "via": via, "source": "person" })
}

fn body_of(request: &crate::api::HttpRequest) -> serde_json::Value {
    serde_json::from_slice(request.body.as_deref().unwrap_or(b"null")).expect("a JSON body")
}

/// ADEV-328: linking and unlinking a task's own goals sends the write, stores the server's answer
/// as the cached links and announces `taskGoalSignals`, so the detail redraws without a re-read.
#[tokio::test]
async fn adev_328_task_goal_writes_store_the_answer_and_announce() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "/api/v1/tasks/t1/goals",
                200,
                json!({ "taskId": "t1", "links": [link("g1", "task"), link("g9", "list")] }),
            )
            .push_json(
                "/api/v1/tasks/t1/goals",
                200,
                json!({ "taskId": "t1", "links": [link("g9", "list")] }),
            )
            .push("/api/v1/tasks/t1/goals", offline()),
    );
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    app.realtime().on_change(move |change| {
        sink.lock().expect("lock").push(change.to_json());
    });

    let set = call(
        &app,
        json!({ "kind": "setTaskGoalLinks", "taskId": "t1", "goalIds": ["g1"], "projectId": "p1" }),
    )
    .await;
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["value"]["links"][0]["goalId"], "g1");

    let unlinked = call(
        &app,
        json!({ "kind": "unlinkTaskGoal", "taskId": "t1", "goalId": "g1", "projectId": "p1" }),
    )
    .await;
    assert_eq!(unlinked["ok"], true, "{unlinked}");
    assert_eq!(unlinked["value"]["links"].as_array().map(Vec::len), Some(1));

    // Offline, the read answers what the last write stored.
    let cached = call(&app, json!({ "kind": "getTaskGoalLinks", "taskId": "t1" })).await;
    assert_eq!(cached["value"], unlinked["value"]);

    let sent = transport.requests();
    assert_eq!(sent[0].method.as_str(), "PUT");
    assert_eq!(
        body_of(&sent[0]),
        json!({ "goalIds": ["g1"], "projectId": "p1" })
    );
    assert_eq!(sent[1].method.as_str(), "DELETE");
    assert_eq!(
        body_of(&sent[1]),
        json!({ "goalId": "g1", "projectId": "p1" })
    );

    let events = heard.lock().expect("lock").clone();
    let moved = events
        .iter()
        .filter(|e| e.contains("taskGoalSignals"))
        .count();
    assert_eq!(moved, 2, "once per write: {events:?}");
}

/// ADEV-328: a write is not journalled. Offline or refused, it fails and the cache keeps what the
/// server last said.
#[tokio::test]
async fn adev_328_a_failed_goal_write_fails_and_keeps_the_cache() {
    let (app, _transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "/api/v1/tasks/t1/goals",
                200,
                json!({ "taskId": "t1", "links": [link("g1", "task")] }),
            )
            .push("/api/v1/tasks/t1/goals", offline())
            .push_json(
                "/api/v1/tasks/t1/goals",
                400,
                json!({ "error": "The project does not state goal g7" }),
            )
            .push("/api/v1/tasks/t1/goals", offline()),
    );
    let read = call(&app, json!({ "kind": "getTaskGoalLinks", "taskId": "t1" })).await;
    let offline_write = call(
        &app,
        json!({ "kind": "setTaskGoalLinks", "taskId": "t1", "goalIds": ["g1", "g2"] }),
    )
    .await;
    assert_eq!(offline_write["ok"], false, "{offline_write}");
    let refused = call(
        &app,
        json!({ "kind": "setTaskGoalLinks", "taskId": "t1", "goalIds": ["g7"] }),
    )
    .await;
    assert_eq!(refused["ok"], false, "{refused}");
    let cached = call(&app, json!({ "kind": "getTaskGoalLinks", "taskId": "t1" })).await;
    assert_eq!(cached["value"], read["value"]);
}

/// ADEV-328: a list's goal links are read, replaced and unlinked, and the answer is cached for
/// an offline read. `projectId` is the server's: `null` for a list in no project.
#[tokio::test]
async fn adev_328_list_goal_links_read_write_and_cache() {
    let list = |goals: &[&str]| {
        json!({ "listId": "l1", "projectId": "p1",
                "links": goals.iter().map(|g| link(g, "list")).collect::<Vec<_>>() })
    };
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json("/api/v1/lists/l1/goal-links", 200, list(&["g1"]))
            .push_json("/api/v1/lists/l1/goal-links", 200, list(&["g1", "g2"]))
            .push_json("/api/v1/lists/l1/goal-links", 200, list(&["g2"]))
            .push("/api/v1/lists/l1/goal-links", offline())
            .push_json(
                "/api/v1/lists/l2/goal-links",
                200,
                json!({ "listId": "l2", "projectId": null, "links": [] }),
            ),
    );
    let read = call(&app, json!({ "kind": "getListGoalLinks", "listId": "l1" })).await;
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["value"]["projectId"], "p1");
    let set = call(
        &app,
        json!({ "kind": "setListGoalLinks", "listId": "l1", "goalIds": ["g1", "g2"] }),
    )
    .await;
    assert_eq!(
        set["value"]["links"].as_array().map(Vec::len),
        Some(2),
        "{set}"
    );
    let unlinked = call(
        &app,
        json!({ "kind": "unlinkListGoal", "listId": "l1", "goalId": "g1" }),
    )
    .await;
    assert_eq!(unlinked["value"]["links"][0]["goalId"], "g2", "{unlinked}");
    let cached = call(&app, json!({ "kind": "getListGoalLinks", "listId": "l1" })).await;
    assert_eq!(cached["value"], unlinked["value"]);
    let none = call(&app, json!({ "kind": "getListGoalLinks", "listId": "l2" })).await;
    assert_eq!(
        none["value"]["projectId"],
        serde_json::Value::Null,
        "{none}"
    );

    let sent = transport.requests();
    assert_eq!(sent[1].method.as_str(), "PUT");
    assert_eq!(body_of(&sent[1]), json!({ "goalIds": ["g1", "g2"] }));
    assert_eq!(sent[2].method.as_str(), "DELETE");
    assert_eq!(body_of(&sent[2]), json!({ "goalId": "g1" }));
}
