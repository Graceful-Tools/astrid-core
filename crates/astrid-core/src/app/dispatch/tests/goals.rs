//! A project's goals and guardrails (ADEV-187; web ADEV-179).

use super::*;
use std::sync::{Arc, Mutex};

fn goals_body() -> serde_json::Value {
    json!({
        "projectId": "p1", "projectName": "Fund",
        "goals": [{ "id": "returns", "statement": "Maximise returns", "measure": "IRR", "target": "≥ 8%/yr", "current": "6.1%" }],
        "guardrails": [
            { "id": "law", "statement": "Break no laws", "kind": "stated" },
            { "id": "r1", "statement": "No more than 20% in one position", "kind": "enforced" }
        ],
        "canEdit": true,
        "meta": { "apiVersion": "v1" }
    })
}

fn offline() -> Result<crate::api::HttpResponse, crate::api::TransportError> {
    Err(crate::api::TransportError::Unreachable("offline".into()))
}

fn heard(app: &App) -> Arc<Mutex<Vec<String>>> {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    app.realtime().on_change(move |change| {
        sink.lock().expect("lock").push(change.to_json());
    });
    heard
}

/// ADEV-187: the route's body is the answer as-is, it is cached for an offline board, and the
/// `projectGoals` change fires once, when the cached value moved.
#[tokio::test]
async fn adev_187_goals_are_read_cached_and_announced_once() {
    let transport = StubTransport::new()
        .push_json("/api/v1/projects/p1/goals", 200, goals_body())
        .push_json("/api/v1/projects/p1/goals", 200, goals_body())
        .push("/api/v1/projects/p1/goals", offline());
    let app = app_with(transport);
    let heard = heard(&app);

    let first = call(
        &app,
        json!({ "kind": "getProjectGoals", "projectId": "p1" }),
    )
    .await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["value"]["projectName"], "Fund");
    assert_eq!(first["value"]["goals"][0]["target"], "≥ 8%/yr");
    assert_eq!(first["value"]["guardrails"][1]["kind"], "enforced");
    assert_eq!(first["value"]["canEdit"], true);

    let same = call(
        &app,
        json!({ "kind": "getProjectGoals", "projectId": "p1" }),
    )
    .await;
    assert_eq!(same["value"], first["value"]);

    // Offline: the cache answers.
    let cached = call(
        &app,
        json!({ "kind": "getProjectGoals", "projectId": "p1" }),
    )
    .await;
    assert_eq!(cached["ok"], true, "{cached}");
    assert_eq!(cached["value"], first["value"]);

    let events = heard.lock().expect("lock").clone();
    assert_eq!(
        events,
        [r#"{"change":"projectGoals","id":"p1"}"#],
        "only the first answer moved the cache"
    );
}

/// ADEV-187: a list answers its project's goals; one in no project has none and no project id.
#[tokio::test]
async fn adev_187_a_list_answers_its_projects_goals_and_none_without_one() {
    let mut with_project = goals_body();
    with_project["listId"] = json!("l1");
    let transport = StubTransport::new()
        .push_json("/api/v1/lists/l1/goals", 200, with_project)
        .push_json(
            "/api/v1/lists/l2/goals",
            200,
            json!({ "listId": "l2", "projectId": null, "projectName": null, "goals": [], "guardrails": [], "canEdit": false }),
        );
    let app = app_with(transport);

    let one = call(&app, json!({ "kind": "getListGoals", "listId": "l1" })).await;
    assert_eq!(one["value"]["projectId"], "p1", "{one}");
    assert_eq!(one["value"]["goals"].as_array().map(Vec::len), Some(1));

    let none = call(&app, json!({ "kind": "getListGoals", "listId": "l2" })).await;
    assert_eq!(none["ok"], true, "{none}");
    assert!(none["value"]["projectId"].is_null());
    assert_eq!(none["value"]["goals"], json!([]));
    assert_eq!(none["value"]["canEdit"], false);
}

/// ADEV-187: online, the answer to a write is the server's echo (it assigned the ids), not the
/// request, and nothing is journalled.
#[tokio::test]
async fn adev_187_a_write_keeps_the_servers_echo() {
    let transport = StubTransport::new().push_json(
        "/api/v1/projects/p1/goals",
        200,
        json!({
            "projectId": "p1", "projectName": "Fund",
            "goals": [{ "id": "g1", "statement": "Grow" }],
            "guardrails": [], "canEdit": true
        }),
    );
    let app = app_with(transport);

    let saved = call(
        &app,
        json!({ "kind": "setProjectGoals", "projectId": "p1",
                "goals": [{ "statement": "  Grow " }], "guardrails": [] }),
    )
    .await;
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["value"]["goals"][0]["id"], "g1");
    assert!(crate::outbox::journal::all(&app.store)
        .expect("reads")
        .is_empty());
}

/// ADEV-187: a refusal (an agent token, not an owner or admin, a limit) is a plain failure: it is
/// shown to the person who asked, not retried, and not queued.
#[tokio::test]
async fn adev_187_a_refused_write_is_a_plain_failure_and_is_not_queued() {
    for status in [400, 403] {
        let transport = StubTransport::new().push_json(
            "/api/v1/projects/p1/goals",
            status,
            json!({ "error": "refused" }),
        );
        let app = app_with(transport);
        let answer = call(
            &app,
            json!({ "kind": "setProjectGoals", "projectId": "p1",
                    "goals": [{ "statement": "Grow" }], "guardrails": [] }),
        )
        .await;
        assert_eq!(answer["ok"], false, "{answer}");
        assert!(
            crate::outbox::journal::all(&app.store)
                .expect("reads")
                .is_empty(),
            "a {status} must not be journalled"
        );
    }
}

/// ADEV-187: offline, the edit shows at once with the ids the server would give, is journalled,
/// and once delivered the server's echo replaces it and the shell is told.
#[tokio::test]
async fn adev_187_an_offline_write_is_journalled_then_replaced_by_the_echo() {
    let transport = StubTransport::new()
        .push("/api/v1/projects/p1/goals", offline())
        .push_json(
            "/api/v1/projects/p1/goals",
            200,
            json!({
                "projectId": "p1", "projectName": "Fund",
                "goals": [{ "id": "g1", "statement": "Grow", "measure": "ARR" }],
                "guardrails": [{ "id": "r1", "statement": "No debt", "kind": "enforced" }],
                "canEdit": true
            }),
        );
    let app = app_with(transport);
    let heard = heard(&app);

    let shown = call(
        &app,
        json!({ "kind": "setProjectGoals", "projectId": "p1",
                "goals": [{ "statement": "Grow" }, { "statement": "   " }],
                "guardrails": [{ "statement": "No debt", "kind": "enforced" }] }),
    )
    .await;
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(shown["value"]["goals"].as_array().map(Vec::len), Some(1));
    assert_eq!(shown["value"]["goals"][0]["id"], "g1");
    assert_eq!(shown["value"]["guardrails"][0]["id"], "r1");

    let journal = crate::outbox::journal::all(&app.store).expect("reads");
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].kind, "setProjectGoals");
    assert_eq!(journal[0].payload["projectId"], "p1");

    // What the delivery loop does: drain, then announce what settled.
    let report = app.runner.drain().await.expect("drains");
    crate::app::background::announce_delivery(&app, report.delivered);
    let kept = app.context.goals().cached_project("p1");
    assert_eq!(
        kept.goals[0].measure.as_deref(),
        Some("ARR"),
        "the echo won"
    );

    let events = heard.lock().expect("lock").clone();
    assert!(
        events
            .iter()
            .filter(|event| event.as_str() == r#"{"change":"projectGoals","id":"p1"}"#)
            .count()
            >= 2,
        "told when it showed and when it settled: {events:?}"
    );
}

/// ADEV-187: the offline write that the server refuses on delivery is dead-lettered, not retried.
#[tokio::test]
async fn adev_187_a_write_refused_on_delivery_is_dead_lettered() {
    let transport = StubTransport::new()
        .push("/api/v1/projects/p1/goals", offline())
        .push_json(
            "/api/v1/projects/p1/goals",
            403,
            json!({ "error": "forbidden" }),
        );
    let app = app_with(transport);
    let _ = call(
        &app,
        json!({ "kind": "setProjectGoals", "projectId": "p1",
                "goals": [{ "statement": "Grow" }], "guardrails": [] }),
    )
    .await;
    let _ = call(&app, json!({ "kind": "drain" })).await;
    let journal = crate::outbox::journal::all(&app.store).expect("reads");
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].status, crate::outbox::Status::FailedPermanent);
}
