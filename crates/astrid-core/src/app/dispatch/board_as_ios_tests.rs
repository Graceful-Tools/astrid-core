//! The board answers and moves as iOS's board did (AITD-461).
//!
//! The Swift the Apple apps carried (`ProjectStatus.swift`, `ProjectStateMove`, `MacBoardMove`)
//! was run against this crate before it was deleted; these are the places it answered
//! differently, each now iOS's answer (Jon, 2026-10-03: on disagreement follow iOS —
//! `docs/CONTRACTS.md` D43–D46).

use super::super::tests::app_with;
use crate::api::StubTransport;
use serde_json::{json, Value};

async fn call(app: &super::App, command: Value) -> Value {
    serde_json::from_str(&app.run_json(&command.to_string()).await).expect("valid JSON")
}

/// A board: project `p1`, its list `l1` (with `list` merged in), and `tasks` in the cache.
async fn board_with(list: Value, tasks: Value, extra_lists: Value) -> super::App {
    let app = app_with(StubTransport::new());
    let mut domain = json!({ "id": "l1", "name": "Work", "projectId": "p1" });
    for (key, value) in list.as_object().expect("an object") {
        domain[key] = value.clone();
    }
    let mut lists = vec![domain];
    lists.extend(extra_lists.as_array().cloned().unwrap_or_default());
    let seeded = call(
        &app,
        json!({
            "kind": "seedCache",
            "tasks": tasks,
            "lists": lists,
            "projects": [{ "id": "p1", "name": "Ship it" }],
        }),
    )
    .await;
    assert_eq!(seeded["value"]["seeded"], true, "{seeded}");
    app
}

fn card(id: &str, extra: Value) -> Value {
    let mut task = json!({ "id": id, "title": id, "listIds": ["l1"],
                           "createdAt": "2026-09-01T00:00:00Z" });
    for (key, value) in extra.as_object().expect("an object") {
        task[key] = value.clone();
    }
    task
}

/// Each column's ids, by column id.
async fn columns(app: &super::App, command: Value) -> Vec<(String, Vec<String>)> {
    let mut command = command;
    command["kind"] = json!("board");
    command["idsOnly"] = json!(true);
    let answer = call(app, command).await;
    assert_eq!(answer["ok"], true, "{answer}");
    answer["value"]["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|column| {
            (
                column["id"].as_str().expect("an id").to_string(),
                column["ids"]
                    .as_array()
                    .expect("ids")
                    .iter()
                    .map(|id| id.as_str().expect("an id").to_string())
                    .collect(),
            )
        })
        .collect()
}

fn column<'a>(columns: &'a [(String, Vec<String>)], id: &str) -> &'a [String] {
    &columns
        .iter()
        .find(|(column, _)| column == id)
        .expect("a column")
        .1
}

/// What each queued write does: "complete" when it carries the completion flag, else "edit".
fn journal_kinds(app: &super::App) -> Vec<&'static str> {
    crate::outbox::journal::all(&app.store)
        .expect("reads")
        .into_iter()
        .map(|entry| match entry.payload["body"].get("completed") {
            Some(_) => "complete",
            None => "edit",
        })
        .collect()
}

/// D43: a column is in the opened list's manual order, the cards it does not name after the
/// rest in iOS's store order; subtasks are not cards; and every card comes back.
#[tokio::test]
async fn aitd461_cards_are_top_level_in_the_manual_order_and_uncapped() {
    let mut tasks = vec![
        card("a", json!({ "createdAt": "2026-09-03T00:00:00Z" })),
        card("b", json!({ "createdAt": "2026-09-02T00:00:00Z" })),
        card("c", json!({ "createdAt": "2026-09-04T00:00:00Z" })),
        card("child", json!({ "parentTaskId": "a" })),
    ];
    for n in 0..60 {
        tasks.push(card(
            &format!("doing-{n:02}"),
            json!({ "statusRole": "doing" }),
        ));
    }
    let app = board_with(
        json!({ "manualSortOrder": ["b", "a"] }),
        Value::Array(tasks),
        json!([]),
    )
    .await;

    let drawn = columns(&app, json!({ "listId": "l1" })).await;
    // Named first in the order's order; then c, which the order never named.
    assert_eq!(column(&drawn, "__virtual_inbox__"), ["b", "a", "c"]);
    assert_eq!(
        column(&drawn, "doing").len(),
        60,
        "no 50-card cap: iOS draws them all"
    );

    // With no manual order, iOS's store order: newest first.
    let app = board_with(
        json!({}),
        json!([
            card("old", json!({ "createdAt": "2026-09-01T00:00:00Z" })),
            card("new", json!({ "createdAt": "2026-09-05T00:00:00Z" })),
        ]),
        json!([]),
    )
    .await;
    let drawn = columns(&app, json!({ "listId": "l1" })).await;
    assert_eq!(column(&drawn, "__virtual_inbox__"), ["new", "old"]);
}

/// D43: Done holds recent work — the opened list's completion filter and window, timed by when a
/// card was last touched. The clock is 2026-09-07T12:00Z.
#[tokio::test]
async fn aitd461_done_holds_what_the_window_lets_through() {
    let finished = json!([
        card(
            "recent",
            json!({ "completed": true, "updatedAt": "2026-09-07T11:00:00Z" })
        ),
        card(
            "old",
            json!({ "completed": true, "updatedAt": "2026-09-05T11:00:00Z",
                            "completedAt": "2026-09-07T11:00:00Z" })
        ),
    ]);
    let window = json!({ "kind": "duration", "amount": 1, "unit": "day" });

    let app = board_with(
        json!({ "recentlyCompletedWindow": window }),
        finished.clone(),
        json!([]),
    )
    .await;
    let drawn = columns(&app, json!({ "listId": "l1" })).await;
    assert_eq!(
        column(&drawn, "__virtual_done__"),
        ["recent"],
        "old was touched two days ago; its completedAt is not what iOS times it by"
    );

    let app = board_with(
        json!({ "recentlyCompletedWindow": window, "filterCompletion": "show" }),
        finished.clone(),
        json!([]),
    )
    .await;
    let drawn = columns(&app, json!({ "listId": "l1" })).await;
    assert_eq!(column(&drawn, "__virtual_done__").len(), 2);

    let app = board_with(json!({ "filterCompletion": "hide" }), finished, json!([])).await;
    let drawn = columns(&app, json!({ "listId": "l1" })).await;
    assert!(column(&drawn, "__virtual_done__").is_empty());
}

/// D44: a cached status row names its default column, below a rename stored on the board.
#[tokio::test]
async fn aitd461_a_cached_status_row_names_its_default_column() {
    let app = board_with(
        json!({}),
        json!([]),
        json!([{ "id": "row-doing", "name": "In flight", "listType": "status",
                 "statusRole": "doing", "statusDescription": "Moving" }]),
    )
    .await;
    let answer = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    let doing = &answer["value"]["columns"][2];
    assert_eq!(
        doing["id"], "doing",
        "the id is the role whatever is cached"
    );
    assert_eq!(doing["name"], "In flight");
    assert_eq!(doing["description"], "Moving");
}

/// A board asked for by its project alone draws its columns: a shell that has the project but
/// not its list yet.
#[tokio::test]
async fn aitd461_a_board_can_be_asked_for_by_project() {
    let app = board_with(json!({}), json!([card("a", json!({}))]), json!([])).await;
    let drawn = columns(&app, json!({ "projectId": "p1" })).await;
    assert_eq!(drawn.len(), 5);
    assert_eq!(column(&drawn, "__virtual_inbox__"), ["a"]);
}

/// D45: a move to the column the card is already in writes nothing.
#[tokio::test]
async fn aitd461_a_move_to_its_own_column_writes_nothing() {
    let app = board_with(
        json!({}),
        json!([card("t1", json!({ "statusRole": "doing" }))]),
        json!([]),
    )
    .await;
    let moved = call(
        &app,
        json!({ "kind": "moveTaskToColumn", "taskId": "t1", "columnId": "doing", "listId": "l1" }),
    )
    .await;
    assert_eq!(moved["ok"], true, "{moved}");
    assert_eq!(moved["value"]["statusRole"], "doing");
    let menu = call(
        &app,
        json!({ "kind": "setTaskStatus", "taskId": "t1", "columnId": "doing" }),
    )
    .await;
    assert_eq!(menu["ok"], true, "{menu}");
    assert!(journal_kinds(&app).is_empty(), "{:?}", journal_kinds(&app));
}

/// D45: leaving Done un-completes first, then gives the card its column.
#[tokio::test]
async fn aitd461_leaving_done_uncompletes_first() {
    let app = board_with(
        json!({}),
        json!([card(
            "t1",
            json!({ "completed": true, "updatedAt": "2026-09-07T11:00:00Z" })
        )]),
        json!([]),
    )
    .await;
    let moved = call(
        &app,
        json!({ "kind": "moveTaskToColumn", "taskId": "t1", "columnId": "ready", "listId": "l1" }),
    )
    .await;
    assert_eq!(moved["ok"], true, "{moved}");
    assert_eq!(moved["value"]["completed"], false);
    assert_eq!(moved["value"]["statusRole"], "ready");
    assert_eq!(journal_kinds(&app), ["complete", "edit"]);
}

/// D45: going to Done sets the column first (no role), then completes.
#[tokio::test]
async fn aitd461_going_to_done_moves_then_completes() {
    let app = board_with(
        json!({}),
        json!([card("t1", json!({ "statusRole": "doing" }))]),
        json!([]),
    )
    .await;
    let moved = call(
        &app,
        json!({ "kind": "setTaskStatus", "taskId": "t1", "columnId": "__virtual_done__" }),
    )
    .await;
    assert_eq!(moved["value"]["completed"], true, "{moved}");
    assert!(moved["value"]["statusRole"].is_null());
    assert_eq!(journal_kinds(&app), ["edit", "complete"]);
}

/// A drop writes what iOS's drop wrote: the move, then the card's new place in the opened list's
/// manual order, with the list's sort set to manual so the order shows.
#[tokio::test]
async fn aitd461_a_drop_writes_the_move_and_the_manual_order() {
    let app = board_with(
        json!({ "manualSortOrder": ["a", "b", "x"], "sortBy": "priority" }),
        json!([
            card("a", json!({ "statusRole": "doing" })),
            card("b", json!({ "statusRole": "doing" })),
            card("x", json!({})),
        ]),
        json!([]),
    )
    .await;
    let dropped = call(
        &app,
        json!({ "kind": "dropBoardCard", "taskId": "x", "columnId": "doing",
                "listId": "l1", "index": 1 }),
    )
    .await;
    assert_eq!(dropped["ok"], true, "{dropped}");
    assert_eq!(dropped["value"]["task"]["statusRole"], "doing");
    assert_eq!(
        dropped["value"]["list"]["manualSortOrder"],
        json!(["a", "x", "b"])
    );
    assert_eq!(dropped["value"]["list"]["sortBy"], "manual");

    let drawn = columns(&app, json!({ "listId": "l1" })).await;
    assert_eq!(column(&drawn, "doing"), ["a", "x", "b"]);

    // Rearranging inside its own column is still a drop: the order changes, the role does not.
    let again = call(
        &app,
        json!({ "kind": "dropBoardCard", "taskId": "b", "columnId": "doing",
                "listId": "l1", "index": 0 }),
    )
    .await;
    assert_eq!(again["ok"], true, "{again}");
    let drawn = columns(&app, json!({ "listId": "l1" })).await;
    assert_eq!(column(&drawn, "doing"), ["b", "a", "x"]);

    // A repeating card dropped on Done rolls forward through the completion service.
    let finished = call(
        &app,
        json!({ "kind": "dropBoardCard", "taskId": "a", "columnId": "__virtual_done__",
                "listId": "l1", "index": 0 }),
    )
    .await;
    assert_eq!(finished["value"]["task"]["completed"], true, "{finished}");
    assert!(finished["value"]["task"]["statusRole"].is_null());
}

/// D46: the status menu never offers Done; a finished task's `current` still says Done.
#[tokio::test]
async fn aitd461_the_status_menu_has_no_done() {
    let app = board_with(
        json!({}),
        json!([card("t1", json!({ "completed": true }))]),
        json!([]),
    )
    .await;
    let options = call(&app, json!({ "kind": "taskStatusOptions", "taskId": "t1" })).await;
    let ids: Vec<&str> = options["value"]["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|column| column["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, ["__virtual_inbox__", "ready", "doing", "waiting"]);
    assert_eq!(options["value"]["current"], "__virtual_done__");
}
