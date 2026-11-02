//! How many rows a read decodes, for the reads whose answer is small whatever the account holds.
//!
//! Regression cover for task `71ddacb8`: `store.tasks()` is `SELECT json FROM tasks` plus a full
//! decode per row, and sixteen places called it — including the reminder loop, every thirty
//! seconds, and the Google mirror, every five minutes. Every one of them then threw almost all of
//! it away.
//!
//! These are not timings. `performance.rs` holds the budgets, and a budget in milliseconds cannot
//! tell "bounded" from "fast on this laptop" — ten thousand decodes inside half a second still
//! passes it, and still costs a phone its battery. What is asserted here is the *count*:
//! [`astrid_core::store::decodes`] counts a row decode on the calling thread, so a read bounded in
//! SQL shows a small number and one bounded in Rust afterwards shows the size of the account.
//!
//! The counter is per thread, so these tests do not interfere with each other or with anything
//! else `cargo test` is running at the time.

use std::sync::Arc;

use astrid_core::api::transport::StubTransport;
use astrid_core::app::{App, Config};
use astrid_core::model::{date, Task};
use astrid_core::platform::{FixedClock, MemorySecureStore};
use astrid_core::store::decodes;
use serde_json::json;

/// Big enough that "it decoded the account" and "it decoded what it needed" cannot be confused.
const TASKS: usize = 500;

/// The clock every fixture here is fixed at.
const NOW: &str = "2026-09-07T12:00:00Z";

fn app() -> App {
    App::with_parts(
        &Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
            platform: Default::default(),
            url_scheme: astrid_core::app::DEFAULT_URL_SCHEME.into(),
            handoff_client_id: None,
        },
        Arc::new(MemorySecureStore::new()),
        Arc::new(StubTransport::new()),
        Arc::new(FixedClock::parsed(NOW)),
    )
    .expect("starts")
}

/// An account of `TASKS` tasks in one list, none of which has a reminder.
fn app_with_tasks() -> App {
    let app = app();
    let list: astrid_core::model::TaskList =
        serde_json::from_value(json!({ "id": "l1", "name": "Everything" })).expect("a list");
    app.store().upsert_list(&list).expect("stores");
    let tasks: Vec<Task> = (0..TASKS)
        .map(|index| Task {
            list_ids: Some(vec!["l1".into()]),
            due_date_time: date::parse("2026-09-08T09:00:00Z"),
            ..Task::new(format!("t{index}"), format!("Task number {index}"))
        })
        .collect();
    app.store().upsert_tasks(&tasks).expect("stores");
    app
}

fn with_reminder(app: &App, id: &str, reminder: &str) {
    let task = Task {
        list_ids: Some(vec!["l1".into()]),
        reminder_time: date::parse(reminder),
        ..Task::new(id, format!("Remember {id}"))
    };
    app.store().upsert_task(&task).expect("stores");
}

async fn run(app: &App, command: serde_json::Value) -> serde_json::Value {
    let answer = app.run_json(&command.to_string()).await;
    serde_json::from_str(&answer).expect("an answer")
}

/// The one that was costing the most: a tick on an account with no reminder in it at all.
///
/// Every thirty seconds, for as long as the app is open. It must read nothing — not "quickly read
/// everything", nothing — because there is nothing for it to find, and SQL can say so.
#[tokio::test]
async fn a_reminder_tick_decodes_nothing_when_no_reminder_is_due_71ddacb8() {
    let app = app_with_tasks();
    decodes::reset();
    let answer = run(&app, json!({ "kind": "remindersDue" })).await;
    assert_eq!(answer["value"]["reminders"], json!([]));
    assert_eq!(
        decodes::count(),
        0,
        "a reminder tick decoded rows on an account with no reminders"
    );
}

/// And when one *is* due, only the ones in the window — not the account around them.
#[tokio::test]
async fn a_reminder_tick_decodes_only_the_reminders_in_its_window_71ddacb8() {
    let app = app_with_tasks();
    // In the window: GRACE is an hour, so half an hour ago is still worth showing.
    with_reminder(&app, "due-now", "2026-09-07T11:30:00Z");
    // Outside it on both sides: last week is history, tomorrow has not happened.
    with_reminder(&app, "long-past", "2026-08-30T09:00:00Z");
    with_reminder(&app, "future", "2026-09-08T09:00:00Z");

    decodes::reset();
    let answer = run(&app, json!({ "kind": "remindersDue" })).await;
    let reminders = answer["value"]["reminders"]
        .as_array()
        .expect("reminders")
        .clone();
    assert_eq!(reminders.len(), 1, "{reminders:?}");
    assert_eq!(reminders[0]["taskId"], "due-now");
    assert_eq!(
        decodes::count(),
        1,
        "a reminder tick decoded more than the reminders in its own window"
    );
}

/// `nextReminder` is bounded by "has a future reminder", which is a different window and the same
/// point: an account of five hundred tasks and two reminders costs two decodes.
#[tokio::test]
async fn the_next_reminder_decodes_only_future_reminders_71ddacb8() {
    let app = app_with_tasks();
    with_reminder(&app, "soon", "2026-09-07T18:00:00Z");
    with_reminder(&app, "later", "2026-09-09T09:00:00Z");
    with_reminder(&app, "long-past", "2026-08-30T09:00:00Z");

    decodes::reset();
    let answer = run(&app, json!({ "kind": "nextReminder" })).await;
    assert_eq!(answer["value"]["reminder"]["taskId"], "soon");
    assert_eq!(
        decodes::count(),
        2,
        "the next reminder decoded more than the tasks with a future reminder"
    );
}

/// A reminder that exists only as a snooze made on this device still has to be found — it has no
/// `reminderTime` of its own, so the window query cannot see it. Bounding the read must not lose
/// it (`astrid_rules::reminders::reminder_at`).
#[tokio::test]
async fn a_snoozed_reminder_is_still_found_when_the_read_is_bounded_71ddacb8() {
    let app = app_with_tasks();
    let task = Task {
        list_ids: Some(vec!["l1".into()]),
        ..Task::new("snoozed", "Snoozed, with no reminder of its own")
    };
    app.store().upsert_task(&task).expect("stores");
    app.store()
        .set_metadata("reminder.snoozed.snoozed", "2026-09-07T11:45:00Z")
        .expect("stores");

    let answer = run(&app, json!({ "kind": "remindersDue" })).await;
    let reminders = answer["value"]["reminders"]
        .as_array()
        .expect("reminders")
        .clone();
    assert_eq!(reminders.len(), 1, "{reminders:?}");
    assert_eq!(reminders[0]["taskId"], "snoozed");
}

/// Seeding over a cache that already holds something is refused — and the refusal is a question
/// about one row, not a reason to decode the account to call `.is_empty()` on it.
#[tokio::test]
async fn refusing_to_seed_a_filled_cache_decodes_nothing_71ddacb8() {
    let app = app_with_tasks();
    decodes::reset();
    let answer = run(&app, json!({ "kind": "seedCache", "tasks": [] })).await;
    assert_eq!(answer["value"]["seeded"], json!(false));
    assert_eq!(
        decodes::count(),
        0,
        "testing whether the cache was empty decoded rows"
    );
}

/// A board is a project's lists. It drew every card it needed by reading every task the account
/// holds, so a board in a one-list project cost the whole account.
#[tokio::test]
async fn a_board_decodes_only_the_tasks_in_its_project_71ddacb8() {
    let app = app();
    for (id, project) in [("in-project", Some("p1")), ("elsewhere", None)] {
        let list: astrid_core::model::TaskList = serde_json::from_value(json!({
            "id": id,
            "name": id,
            "projectId": project,
        }))
        .expect("a list");
        app.store().upsert_list(&list).expect("stores");
    }
    // Five in the board's project, and the rest of the account outside it.
    for index in 0..5 {
        let task = Task {
            list_ids: Some(vec!["in-project".into()]),
            ..Task::new(format!("b{index}"), format!("Card {index}"))
        };
        app.store().upsert_task(&task).expect("stores");
    }
    let outside: Vec<Task> = (0..TASKS)
        .map(|index| Task {
            list_ids: Some(vec!["elsewhere".into()]),
            ..Task::new(format!("o{index}"), format!("Not on the board {index}"))
        })
        .collect();
    app.store().upsert_tasks(&outside).expect("stores");

    decodes::reset();
    run(&app, json!({ "kind": "board", "listId": "in-project" })).await;
    let decoded = decodes::count();
    assert!(
        decoded < TASKS as u64,
        "a board decoded {decoded} rows for a project holding five tasks"
    );
}
