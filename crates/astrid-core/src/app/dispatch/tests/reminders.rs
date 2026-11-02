//! Reminders: what is due, what has been shown, and the snooze.

use super::*;

/// The picker offers offsets from the due time, because "an hour before" is what somebody
/// means — and a task with nothing to be before can only have its reminder cleared.
#[tokio::test]
async fn reminder_options_are_offsets_from_the_due_time() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({
            "kind": "createTask",
            "title": "Call the vet",
            "dueDateTime": "2026-09-20T09:00:00Z",
        }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let offered = call(&app, json!({ "kind": "reminderOptions", "taskId": id })).await;
    let picks = offered["value"]["picks"].as_array().expect("picks");
    assert_eq!(picks[0]["titleKey"], "reminder.none");
    assert!(picks.len() > 1);

    let bare = call(&app, json!({ "kind": "createTask", "title": "Someday" })).await;
    let bare_id = bare["value"]["id"].as_str().expect("an id").to_string();
    let none = call(
        &app,
        json!({ "kind": "reminderOptions", "taskId": bare_id }),
    )
    .await;
    assert_eq!(none["value"]["picks"].as_array().expect("picks").len(), 1);
}

/// A reminder is shown once. A banner that comes back every thirty seconds is one that gets
/// dismissed without being read.
#[tokio::test]
async fn a_reminder_is_offered_once_and_again_after_a_snooze() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Call the vet" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    // A minute before the fixed clock, so it is inside the grace window.
    let when = "2026-09-07T11:59:00Z";
    call(
        &app,
        json!({
            "kind": "updateTask",
            "taskId": id,
            "changes": { "reminderTime": when },
        }),
    )
    .await;

    let due = call(&app, json!({ "kind": "remindersDue" })).await;
    assert_eq!(
        due["value"]["reminders"].as_array().expect("a list").len(),
        1
    );

    call(&app, json!({ "kind": "reminderShown", "taskId": id })).await;
    let again = call(&app, json!({ "kind": "remindersDue" })).await;
    assert!(again["value"]["reminders"]
        .as_array()
        .expect("a list")
        .is_empty());

    // Snoozing gives it a new time, so it may ask again — and not before it is due.
    call(
        &app,
        json!({ "kind": "snoozeReminder", "taskId": id, "minutes": 10 }),
    )
    .await;
    let snoozed = call(&app, json!({ "kind": "remindersDue" })).await;
    assert!(snoozed["value"]["reminders"]
        .as_array()
        .expect("a list")
        .is_empty());
}

/// D24, resolved toward iOS (`ReminderPresenter.snoozeTask`): a snooze moves the task's due date
/// to now plus the snooze, as a timed task, and leaves `reminderTime` exactly where it was.
#[tokio::test]
async fn d24_snooze_moves_the_due_date_as_ios_does() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Call the vet", "dueDateTime": "2026-09-08T00:00:00Z" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({
            "kind": "updateTask",
            "taskId": id,
            "changes": { "reminderTime": "2026-09-07T11:59:00Z" },
        }),
    )
    .await;

    let snoozed = call(
        &app,
        json!({ "kind": "snoozeReminder", "taskId": id, "minutes": 15 }),
    )
    .await;

    assert_eq!(snoozed["value"]["dueDateTime"], "2026-09-07T12:15:00Z");
    assert_eq!(snoozed["value"]["isAllDay"], false);
    assert_eq!(snoozed["value"]["reminderTime"], "2026-09-07T11:59:00Z");
}

/// D24: iOS reschedules its own notification for the snoozed time on the device; the core's
/// equivalent is that the in-app reminder comes back then — once — without moving `reminderTime`.
#[tokio::test]
async fn d24_a_snoozed_reminder_comes_back_at_the_snoozed_time() {
    let clock = std::sync::Arc::new(crate::platform::FixedClock::parsed("2026-09-07T12:00:00Z"));
    let app = App::with_parts(
        &Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
            platform: Default::default(),
            url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
            handoff_client_id: None,
        },
        std::sync::Arc::new(crate::platform::MemorySecureStore::new()),
        std::sync::Arc::new(StubTransport::new()),
        clock.clone(),
    )
    .expect("starts");
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Call the vet" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({
            "kind": "updateTask",
            "taskId": id,
            "changes": { "reminderTime": "2026-09-07T11:59:00Z" },
        }),
    )
    .await;
    call(&app, json!({ "kind": "reminderShown", "taskId": id })).await;
    call(
        &app,
        json!({ "kind": "snoozeReminder", "taskId": id, "minutes": 15 }),
    )
    .await;

    let early = call(&app, json!({ "kind": "remindersDue" })).await;
    assert!(early["value"]["reminders"]
        .as_array()
        .expect("a list")
        .is_empty());

    clock.advance(chrono::Duration::minutes(15));
    let back = call(&app, json!({ "kind": "remindersDue" })).await;
    let reminders = back["value"]["reminders"].as_array().expect("a list");
    assert_eq!(reminders.len(), 1, "{back}");
    assert_eq!(reminders[0]["taskId"], id.as_str());

    call(&app, json!({ "kind": "reminderShown", "taskId": id })).await;
    let shown = call(&app, json!({ "kind": "remindersDue" })).await;
    assert!(shown["value"]["reminders"]
        .as_array()
        .expect("a list")
        .is_empty());
}

/// AATD-21: `nextReminder` is the alarm a shell sets — the earliest reminder still to come, and
/// the snoozed time once snoozed.
#[tokio::test]
async fn aatd_21_next_reminder_names_the_alarm_to_set() {
    let clock = std::sync::Arc::new(crate::platform::FixedClock::parsed("2026-09-07T12:00:00Z"));
    let app = App::with_parts(
        &Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
            platform: Default::default(),
            url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
            handoff_client_id: None,
        },
        std::sync::Arc::new(crate::platform::MemorySecureStore::new()),
        std::sync::Arc::new(StubTransport::new()),
        clock.clone(),
    )
    .expect("starts");
    let none = call(&app, json!({ "kind": "nextReminder" })).await;
    assert_eq!(none["value"]["reminder"], serde_json::Value::Null, "{none}");

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Call the vet" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({ "kind": "updateTask", "taskId": id, "changes": { "reminderTime": "2026-09-07T13:00:00Z" } }),
    )
    .await;
    let next = call(&app, json!({ "kind": "nextReminder" })).await;
    assert_eq!(next["value"]["reminder"]["taskId"], id.as_str(), "{next}");
    assert_eq!(
        next["value"]["reminder"]["reminderTime"],
        "2026-09-07T13:00:00Z"
    );

    call(
        &app,
        json!({ "kind": "snoozeReminder", "taskId": id, "minutes": 120 }),
    )
    .await;
    let snoozed = call(&app, json!({ "kind": "nextReminder" })).await;
    assert_eq!(
        snoozed["value"]["reminder"]["reminderTime"], "2026-09-07T14:00:00Z",
        "{snoozed}"
    );
}
