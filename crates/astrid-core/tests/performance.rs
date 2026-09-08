//! What the app costs on an account nobody would call small.
//!
//! Ten thousand tasks in one list, which is more than any real account has and exactly the size
//! the M0 spike was worried about. These are not micro-benchmarks: the numbers a machine produces
//! vary by a factor of three between a laptop on battery and a desktop, so the bounds here are
//! deliberately generous. What they catch is a *shape* change — an accidental O(n²), a clone of
//! the whole account per row, a filter that re-reads the store inside a loop — which shows up as
//! seconds rather than as a percentage.
//!
//! The one number that is not generous is the window: `rowsForList` must not get slower as the
//! list grows past the screen, because that is the whole reason it takes an offset and a limit.

use std::time::{Duration, Instant};

use astrid_core::api::transport::StubTransport;
use astrid_core::app::{App, Config};
use astrid_core::model::{date, Task};
use astrid_core::platform::{FixedClock, MemorySecureStore};
use serde_json::json;
use std::sync::Arc;

/// How many tasks. More than any real account, and the size the transport spike asked about.
const TASKS: usize = 10_000;

/// The bound every measurement below is checked against.
///
/// Half a second for ten thousand tasks. A machine three times slower than this one still passes;
/// an implementation that went quadratic does not.
const BUDGET: Duration = Duration::from_millis(500);

fn app_with_tasks(count: usize) -> App {
    let app = App::with_parts(
        &Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
        },
        Arc::new(MemorySecureStore::new()),
        Arc::new(StubTransport::new()),
        Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
    )
    .expect("starts");

    let list: astrid_core::model::TaskList =
        serde_json::from_value(json!({ "id": "l1", "name": "Everything" })).expect("a list");
    app.store().upsert_list(&list).expect("stores");

    let tasks: Vec<Task> = (0..count)
        .map(|index| Task {
            list_ids: Some(vec!["l1".into()]),
            due_date_time: date::parse("2026-09-08T09:00:00Z"),
            completed: index % 7 == 0,
            description: format!("something about number {index}"),
            ..Task::new(format!("t{index}"), format!("Task number {index}"))
        })
        .collect();
    app.store().upsert_tasks(&tasks).expect("stores");
    app
}

async fn time(app: &App, command: serde_json::Value) -> (Duration, serde_json::Value) {
    let kind = command["kind"].as_str().unwrap_or("?").to_string();
    let started = Instant::now();
    let answer = app.run_json(&command.to_string()).await;
    let elapsed = started.elapsed();
    // Printed, not just asserted: `cargo test -- --nocapture` then reports what this machine
    // actually does, which is the number worth writing down when somebody asks how fast it is.
    println!("{kind}: {elapsed:?}");
    (
        elapsed,
        serde_json::from_str(&answer).expect("an answer that parses"),
    )
}

/// The first paint: one window of rows out of ten thousand tasks.
#[tokio::test]
async fn a_window_of_rows_is_fast_on_a_huge_list() {
    let app = app_with_tasks(TASKS);

    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "rowsForList", "listId": "l1", "offset": 0, "limit": 50 }),
    )
    .await;

    assert_eq!(answer["value"]["rows"].as_array().expect("rows").len(), 50);
    assert!(
        elapsed < BUDGET,
        "a window of 50 rows out of {TASKS} took {elapsed:?}"
    );
}

/// Scrolling: the thousandth window must cost what the first one did.
///
/// This is the one that would catch a window implemented by building every row and slicing.
#[tokio::test]
async fn scrolling_does_not_get_slower_further_down() {
    let app = app_with_tasks(TASKS);

    let (first, _) = time(
        &app,
        json!({ "kind": "rowsForList", "listId": "l1", "offset": 0, "limit": 50 }),
    )
    .await;
    let (deep, answer) = time(
        &app,
        json!({ "kind": "rowsForList", "listId": "l1", "offset": 5000, "limit": 50 }),
    )
    .await;

    assert_eq!(answer["value"]["rows"].as_array().expect("rows").len(), 50);
    assert!(deep < BUDGET, "a window at offset 5000 took {deep:?}");
    // Generous, because both are small and the ratio of two small numbers is noise. What it rules
    // out is the deep window costing a hundred times the first.
    assert!(
        deep < first * 10 + Duration::from_millis(50),
        "scrolling got much slower further down: first {first:?}, deep {deep:?}"
    );
}

/// Search reads every task by definition. It still has to feel instant.
#[tokio::test]
async fn search_over_the_whole_account_is_fast() {
    let app = app_with_tasks(TASKS);

    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "searchTasks", "query": "number 9999", "limit": 50 }),
    )
    .await;

    assert!(answer["value"]["total"].as_u64().expect("a total") >= 1);
    assert!(elapsed < BUDGET, "searching {TASKS} tasks took {elapsed:?}");
}

/// The board groups every card on the project into columns.
#[tokio::test]
async fn a_board_of_ten_thousand_cards_is_fast() {
    let app = app_with_tasks(TASKS);
    let list: astrid_core::model::TaskList =
        serde_json::from_value(json!({ "id": "l1", "name": "Everything", "projectId": "p1" }))
            .expect("a list");
    app.store().upsert_list(&list).expect("stores");

    let (elapsed, answer) = time(&app, json!({ "kind": "board", "listId": "l1" })).await;

    let columns = answer["value"]["columns"].as_array().expect("columns");
    assert_eq!(columns.len(), 5);
    assert!(
        elapsed < BUDGET,
        "a board of {TASKS} cards took {elapsed:?}"
    );
}

/// Completing one task out of ten thousand must not cost the account.
#[tokio::test]
async fn one_write_does_not_cost_the_whole_account() {
    let app = app_with_tasks(TASKS);

    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "completeTask", "taskId": "t42", "completed": true }),
    )
    .await;

    assert_eq!(answer["ok"], true);
    assert!(elapsed < BUDGET, "completing one task took {elapsed:?}");
}
