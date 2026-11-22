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
/// Half a second for ten thousand tasks on a developer machine, which does the board's id pass in
/// about 250ms. On CI the bound is four times that: GitHub's hosted macOS runner, with a debug
/// build and shared cores, takes about 500ms for the same pass, so a 500ms bound there failed on
/// noise: these tests went red 21 times between 2026-10-23 and 2026-11-21, the latest at 506ms. Two seconds
/// still catches what these tests are for: a quadratic pass over ten thousand tasks costs tens of
/// seconds, not a few hundred milliseconds.
fn budget() -> Duration {
    if std::env::var_os("CI").is_some() {
        Duration::from_millis(2_000)
    } else {
        Duration::from_millis(500)
    }
}

/// Held for the whole of each test, so they run one at a time.
///
/// The test harness runs a binary's tests in parallel, and each of these builds ten thousand tasks
/// and times a wall clock. Six of them at once on a two-core CI runner timed each other rather than
/// the code: one of the six went over the budget, a different one in each run (core (linux) on
/// main, 2026-11-14).
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn app_with_tasks(count: usize) -> App {
    let app = App::with_parts(
        &Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
            platform: Default::default(),
            url_scheme: astrid_core::app::DEFAULT_URL_SCHEME.into(),
            handoff_client_id: None,
        },
        Arc::new(MemorySecureStore::new()),
        Arc::new(StubTransport::new()),
        Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
    )
    .expect("starts");

    let list: astrid_core::model::TaskList =
        serde_json::from_value(json!({ "id": "l1", "name": "Everything" })).expect("a list");
    app.store().upsert_list(&list).expect("stores");
    // Signed in, and holding every task: My Tasks is what is assigned to the reader (D25), so an
    // account of unassigned tasks would measure an empty view.
    app.store()
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    let tasks: Vec<Task> = (0..count)
        .map(|index| Task {
            list_ids: Some(vec!["l1".into()]),
            due_date_time: date::parse("2026-09-08T09:00:00Z"),
            completed: index % 7 == 0,
            assignee_id: Some("me".into()),
            description: Some(format!("something about number {index}")),
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
    let _alone = ONE_AT_A_TIME.lock().await;
    let app = app_with_tasks(TASKS);

    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "rowsForList", "listId": "l1", "offset": 0, "limit": 50 }),
    )
    .await;

    assert_eq!(answer["value"]["rows"].as_array().expect("rows").len(), 50);
    assert!(
        elapsed < budget(),
        "a window of 50 rows out of {TASKS} took {elapsed:?}"
    );
}

/// My Tasks is the view the app opens on, so it is the first thing anybody waits for.
///
/// It reads every task in the account rather than one list's membership, which is what makes it
/// worth its own guard: the scope, the filters and the sort all run over the whole set.
#[tokio::test]
async fn my_tasks_is_fast_on_a_huge_account() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let app = app_with_tasks(TASKS);

    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "rowsForList", "listId": "virtual:my-tasks", "offset": 0, "limit": 50 }),
    )
    .await;

    assert_eq!(answer["value"]["rows"].as_array().expect("rows").len(), 50);
    assert!(
        elapsed < budget(),
        "the first window of My Tasks out of {TASKS} took {elapsed:?}"
    );
}

/// Scrolling: the thousandth window must cost what the first one did.
///
/// This is the one that would catch a window implemented by building every row and slicing.
#[tokio::test]
async fn scrolling_does_not_get_slower_further_down() {
    let _alone = ONE_AT_A_TIME.lock().await;
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
    assert!(deep < budget(), "a window at offset 5000 took {deep:?}");
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
    let _alone = ONE_AT_A_TIME.lock().await;
    let app = app_with_tasks(TASKS);

    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "searchTasks", "query": "number 9999", "limit": 50 }),
    )
    .await;

    assert!(answer["value"]["total"].as_u64().expect("a total") >= 1);
    assert!(
        elapsed < budget(),
        "searching {TASKS} tasks took {elapsed:?}"
    );
}

/// The board groups every card on the project into columns.
#[tokio::test]
async fn a_board_of_ten_thousand_cards_is_fast() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let app = app_with_tasks(TASKS);
    let list: astrid_core::model::TaskList =
        serde_json::from_value(json!({ "id": "l1", "name": "Everything", "projectId": "p1" }))
            .expect("a list");
    app.store().upsert_list(&list).expect("stores");

    let (elapsed, answer) = time(&app, json!({ "kind": "board", "listId": "l1" })).await;

    let columns = answer["value"]["columns"].as_array().expect("columns");
    assert_eq!(columns.len(), 5);
    assert!(
        elapsed < budget(),
        "a board of {TASKS} cards took {elapsed:?}"
    );

    // Every card's id, as the Apple boards ask (AITD-461), arranged by a manual order naming
    // them all.
    let order: Vec<String> = (0..TASKS).rev().map(|index| format!("t{index}")).collect();
    let list: astrid_core::model::TaskList = serde_json::from_value(
        json!({ "id": "l1", "name": "Everything", "projectId": "p1", "manualSortOrder": order }),
    )
    .expect("a list");
    app.store().upsert_list(&list).expect("stores");
    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "board", "listId": "l1", "idsOnly": true }),
    )
    .await;
    let inbox = answer["value"]["columns"][1]["ids"]
        .as_array()
        .expect("ids");
    assert_eq!(inbox.first().and_then(|id| id.as_str()), Some("t9999"));
    assert!(
        elapsed < budget(),
        "every id of a board of {TASKS} cards took {elapsed:?}"
    );
}

/// Completing one task out of ten thousand must not cost the account.
#[tokio::test]
async fn one_write_does_not_cost_the_whole_account() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let app = app_with_tasks(TASKS);

    let (elapsed, answer) = time(
        &app,
        json!({ "kind": "completeTask", "taskId": "t42", "completed": true }),
    )
    .await;

    assert_eq!(answer["ok"], true);
    assert!(elapsed < budget(), "completing one task took {elapsed:?}");
}
