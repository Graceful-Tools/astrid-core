//! `rowsForList`, `listCounts` and the search splice answer as iOS's list drew (AITD-460).
//!
//! The Swift filters, sort and splice the Apple apps carried were run against this crate before
//! they were deleted; these are the places it answered differently, each now iOS's answer
//! (Jon, 2026-10-03: on disagreement follow iOS — `docs/CONTRACTS.md` D40–D42), and the inputs a
//! shell holds ahead of the cache.

use super::super::tests::app_with;
use crate::api::StubTransport;
use serde_json::{json, Value};

async fn call(app: &super::App, command: Value) -> Value {
    serde_json::from_str(&app.run_json(&command.to_string()).await).expect("valid JSON")
}

/// An app whose cache holds exactly `tasks`, with "me" signed in.
async fn seeded(tasks: Value) -> super::App {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    let seeded = call(&app, json!({ "kind": "seedCache", "tasks": tasks })).await;
    assert_eq!(seeded["value"]["seeded"], true, "{seeded}");
    app
}

fn task(id: &str, extra: Value) -> Value {
    let mut task = json!({ "id": id, "title": id, "listIds": ["l1"] });
    for (key, value) in extra.as_object().expect("an object") {
        task[key] = value.clone();
    }
    task
}

async fn ids(app: &super::App, mut command: Value) -> Vec<String> {
    command["kind"] = json!("rowsForList");
    command["idsOnly"] = json!(true);
    let answer = call(app, command).await;
    assert_eq!(answer["ok"], true, "{answer}");
    answer["value"]["ids"]
        .as_array()
        .expect("ids")
        .iter()
        .map(|id| id.as_str().expect("an id").to_string())
        .collect()
}

fn list(extra: Value) -> Value {
    let mut list = json!({ "id": "l1", "name": "Work" });
    for (key, value) in extra.as_object().expect("an object") {
        list[key] = value.clone();
    }
    list
}

/// D40: iOS sorts a list that has never been given a sort as `manual` — newest first with
/// nothing arranged; this crate read the absence as auto.
#[tokio::test]
async fn aitd460_a_list_with_no_sort_is_newest_first_as_on_ios() {
    let app = seeded(json!([
        task(
            "old",
            json!({ "priority": 3, "createdAt": "2026-09-01T00:00:00Z" })
        ),
        task(
            "new",
            json!({ "priority": 0, "createdAt": "2026-09-02T00:00:00Z" })
        ),
    ]))
    .await;
    let rows = ids(&app, json!({ "listId": "l1", "list": list(json!({})) })).await;
    assert_eq!(rows, ["new", "old"]);
    let auto = ids(
        &app,
        json!({ "listId": "l1", "list": list(json!({ "sortBy": "auto" })) }),
    )
    .await;
    assert_eq!(auto, ["old", "new"], "auto is still auto");
}

/// D41: a spliced subtask obeys the view's completion filter alone — the list's other filters
/// chose its parent, not its parts.
#[tokio::test]
async fn aitd460_a_subtask_shows_by_the_completion_filter_alone() {
    let app = seeded(json!([
        task("p", json!({ "priority": 3 })),
        task("c", json!({ "priority": 0, "parentTaskId": "p" })),
        task(
            "done",
            json!({ "priority": 3, "parentTaskId": "p", "completed": true,
                             "completedAt": "2026-08-01T00:00:00Z" })
        ),
    ]))
    .await;
    let command = json!({
        "listId": "l1",
        "list": list(json!({ "sortBy": "auto", "filterPriority": "3" })),
        "subtaskDisplay": "indented",
    });
    assert_eq!(ids(&app, command).await, ["p", "c"]);
}

/// D41: iOS splices from every task it holds, so a subtask never filed in its parent's list
/// still shows under it.
#[tokio::test]
async fn aitd460_a_subtask_outside_the_list_is_spliced_under_its_parent() {
    let app = seeded(json!([
        task("p", json!({})),
        task("c", json!({ "parentTaskId": "p", "listIds": [] })),
    ]))
    .await;
    let rows = ids(
        &app,
        json!({ "listId": "l1", "list": list(json!({ "sortBy": "auto" })), "subtaskDisplay": "indented" }),
    )
    .await;
    assert_eq!(rows, ["p", "c"]);
}

/// D42: ties fall in iOS's store order — due date, then newest created, then id — not storage order.
#[tokio::test]
async fn aitd460_ties_fall_in_ios_store_order() {
    let app = seeded(json!([
        task("a-older", json!({ "createdAt": "2026-09-01T00:00:00Z" })),
        task("z-newer", json!({ "createdAt": "2026-09-03T00:00:00Z" })),
        task("m-middle", json!({ "createdAt": "2026-09-02T00:00:00Z" })),
    ]))
    .await;
    let rows = ids(
        &app,
        json!({ "listId": "l1", "list": list(json!({ "sortBy": "priority" })) }),
    )
    .await;
    assert_eq!(rows, ["z-newer", "m-middle", "a-older"]);
}

/// What the shell holds wins over the cache: who is signed in, My Tasks' filters, a device sort.
#[tokio::test]
async fn aitd460_the_shells_inputs_win_over_the_cache() {
    let app = seeded(json!([
        task(
            "mine",
            json!({ "assigneeId": "me", "priority": 1, "listIds": [] })
        ),
        task(
            "yours",
            json!({ "assigneeId": "you", "priority": 3, "listIds": [] })
        ),
    ]))
    .await;
    let my_tasks = json!({ "listId": "virtual:my-tasks" });
    assert_eq!(ids(&app, my_tasks.clone()).await, ["mine"]);

    let as_you = json!({ "listId": "virtual:my-tasks", "currentUserId": "you" });
    assert_eq!(ids(&app, as_you).await, ["yours"]);

    let held = json!({ "listId": "virtual:my-tasks", "myTasks": { "filterPriority": [3] } });
    assert_eq!(ids(&app, held).await, Vec::<String>::new());

    let everything = json!({
        "listId": "virtual:all",
        "list": { "id": "virtual:all", "name": "", "isVirtual": true, "sortBy": "priority" },
    });
    assert_eq!(ids(&app, everything.clone()).await, ["yours", "mine"]);
    let mut by_hand = everything;
    by_hand["sortBy"] = json!("createdAt");
    by_hand["list"]["filterPriority"] = json!("1");
    assert_eq!(ids(&app, by_hand).await, ["mine"]);
}

/// A public list the reader is not in: neither it nor its tasks are cached, so both travel.
#[tokio::test]
async fn aitd460_a_list_and_tasks_the_cache_does_not_hold() {
    let app = seeded(json!([])).await;
    let rows = ids(
        &app,
        json!({
            "listId": "pub",
            "list": { "id": "pub", "name": "Public", "sortBy": "priority" },
            "tasks": [task("t1", json!({ "listIds": ["pub"] })),
                      task("t2", json!({ "listIds": ["pub"], "priority": 2 }))],
        }),
    )
    .await;
    assert_eq!(rows, ["t2", "t1"]);
}

/// `matched` counts what the filters kept, subtasks included — a sidebar badge — and an
/// ids-only answer projects nothing.
#[tokio::test]
async fn aitd460_matched_counts_what_the_filters_kept() {
    let app = seeded(json!([
        task("p", json!({})),
        task("c", json!({ "parentTaskId": "p" })),
        task(
            "done",
            json!({ "completed": true, "completedAt": "2026-08-01T00:00:00Z" })
        ),
    ]))
    .await;
    let answer = call(
        &app,
        json!({ "kind": "rowsForList", "listId": "l1", "list": list(json!({})), "idsOnly": true,
                "limit": 0 }),
    )
    .await;
    assert_eq!(answer["value"]["matched"], 2, "{answer}");
    assert_eq!(answer["value"]["ids"], json!([]));
    assert!(answer["value"]["rows"].is_null());
}

/// `listCounts`: what each list's saved filters keep, in one call.
#[tokio::test]
async fn aitd460_list_counts_answer_for_every_list_at_once() {
    let app = seeded(json!([
        task("a", json!({ "repeating": "daily", "listIds": [] })),
        task("b", json!({})),
        task(
            "c",
            json!({ "completed": true, "completedAt": "2026-08-01T00:00:00Z" })
        ),
    ]))
    .await;
    let counts = call(
        &app,
        json!({
            "kind": "listCounts",
            "lists": [
                { "id": "daily", "name": "", "isVirtual": true, "filterRepeating": "daily" },
                { "id": "l1", "name": "Work", "filterCompletion": "all" },
            ],
        }),
    )
    .await;
    assert_eq!(counts["value"], json!({ "daily": 1, "l1": 2 }), "{counts}");
}

/// iOS draws a search result's subtasks under it; asked to, the search splices them in by the
/// default completion window. Unasked, results stay flat (the Mac, web).
#[tokio::test]
async fn aitd460_search_splices_subtasks_when_asked() {
    let app = seeded(json!([
        task("milk run", json!({})),
        task("bread", json!({ "parentTaskId": "milk run" })),
        task(
            "old",
            json!({ "parentTaskId": "milk run", "completed": true,
                            "completedAt": "2026-08-01T00:00:00Z" })
        ),
    ]))
    .await;
    let row_ids = |answer: &Value| -> Vec<String> {
        answer["value"]["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["id"].as_str().expect("an id").to_string())
            .collect()
    };
    let flat = call(&app, json!({ "kind": "searchTasks", "query": "milk" })).await;
    assert_eq!(row_ids(&flat), ["milk run"]);
    let spliced = call(
        &app,
        json!({ "kind": "searchTasks", "query": "milk", "subtaskDisplay": "indented" }),
    )
    .await;
    assert_eq!(row_ids(&spliced), ["milk run", "bread"]);
    let off = call(
        &app,
        json!({ "kind": "searchTasks", "query": "milk", "subtaskDisplay": "indented",
                "showSubtasks": false }),
    )
    .await;
    assert_eq!(row_ids(&off), ["milk run"]);
}
