//! The door's tests, one file per domain, matching the modules beside them in `dispatch/`.
//!
//! This was one 7,383-line `tests.rs` until task 2d3a185f split it. Which file a test belongs in
//! is decided by the command it exercises, read off [`super::run`]'s match — so a command that
//! moves between domain modules takes its tests with it.
//!
//! The preamble and every shared helper stay here rather than being copied into the children,
//! which is what keeps this a move: `super` still means `dispatch` at this depth, so no test body
//! needed touching. A child reaches all of it through `use super::*`.

use super::super::tests::app_with;
use crate::api::StubTransport;
use crate::app::{App, Config};
use serde_json::json;

mod account;
mod attachments;
mod board;
mod chat;
mod comments;
mod connections;
mod list_filters;
mod lists;
mod network;
mod reminders;
mod row_json;
mod shell;
mod tasks;

async fn call(app: &super::App, command: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&app.run_json(&command.to_string()).await).expect("valid JSON")
}

/// The id of the list with this name, from the `lists` answer.
async fn list_id_named(app: &super::App, name: &str) -> String {
    let lists = call(app, json!({ "kind": "lists" })).await;
    lists["value"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|list| list["name"] == name)
        .and_then(|list| list["id"].as_str())
        .unwrap_or_else(|| panic!("no list named {name}"))
        .to_string()
}

/// An app whose conversation the test can read back.
fn app_and_transport(transport: StubTransport) -> (App, std::sync::Arc<StubTransport>) {
    let transport = std::sync::Arc::new(transport);
    let app = App::with_parts(
        &Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
            platform: Default::default(),
            url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
            handoff_client_id: None,
        },
        std::sync::Arc::new(crate::platform::MemorySecureStore::new()),
        transport.clone(),
        std::sync::Arc::new(crate::platform::FixedClock::parsed("2026-09-07T12:00:00Z")),
    )
    .expect("starts");
    (app, transport)
}

/// A board with one list in it, for the add-a-card tests below.
async fn board_app(list: serde_json::Value) -> super::App {
    let app = app_with(StubTransport::new());
    app.store
        .upsert_projects(&[
            serde_json::from_value(json!({ "id": "p1", "name": "Ship it" })).expect("a project"),
        ])
        .expect("stores");
    app.store
        .upsert_list(&serde_json::from_value(list).expect("a list"))
        .expect("stores");
    app
}

/// A task on a GitHub board, and a classic one, in a fresh app (AITD-491).
fn on_a_github_board(app: &super::App) {
    app.store
        .upsert_list(
            &serde_json::from_value(json!({
                "id": "gh", "name": "Roadmap", "backend": "github_project", "projectId": "p1"
            }))
            .expect("a list"),
        )
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "home", "name": "Home" })).expect("a list"),
        )
        .expect("stores");
    for (id, list) in [("t1", "gh"), ("t2", "home")] {
        app.store
            .upsert_task(
                &serde_json::from_value(json!({ "id": id, "title": id, "listIds": [list] }))
                    .expect("a task"),
            )
            .expect("stores");
    }
}

fn project_schema() -> serde_json::Value {
    json!({ "fields": [
        { "id": "f-size", "remoteId": "R1", "name": "Size", "dataType": "number", "options": null,
          "iterations": null, "mappedTo": null, "position": 2 },
        { "id": "f-team", "remoteId": "R2", "name": "Team", "dataType": "single_select",
          "options": [{ "id": "o-ios", "name": "iOS", "color": "BLUE" }], "iterations": null,
          "mappedTo": null, "position": 1 },
        { "id": "f-status", "remoteId": "R3", "name": "Status", "dataType": "single_select",
          "options": [], "iterations": null, "mappedTo": "status", "position": 0 }
    ] })
}

/// An app whose reader is `hours` from UTC, for the date arithmetic that only goes wrong there.
fn app_in_zone(hours: i32) -> App {
    App::with_parts(
        &Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
            platform: Default::default(),
            url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
            handoff_client_id: None,
        },
        std::sync::Arc::new(crate::platform::MemorySecureStore::new()),
        std::sync::Arc::new(StubTransport::new()),
        std::sync::Arc::new(
            crate::platform::FixedClock::parsed("2026-12-01T18:00:00Z").in_zone(hours),
        ),
    )
    .expect("starts")
}
