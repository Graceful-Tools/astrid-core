//! Tasks: creation, the editor, detail, dates, blockers and timers.

use super::*;

/// A list's defaults reach a task made at the door, and what was said at the door wins
/// (task c4102c67).
#[tokio::test]
async fn quick_add_takes_the_list_s_defaults_for_what_it_did_not_say_task_c4102c67() {
    // The settings read at the end fetches members over the wire; nothing else here does.
    let app = app_with(StubTransport::new().push_json("/members", 200, json!({ "members": [] })));
    call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let work = list_id_named(&app, "Work").await;
    let set = call(
        &app,
        json!({
            "kind": "updateList", "listId": work,
            "changes": {
                "defaultPriority": 2, "defaultRepeating": "weekly",
                "defaultDueDate": "tomorrow", "defaultDueTime": "17:00",
                "defaultAssigneeId": "unassigned"
            }
        }),
    )
    .await;
    assert_eq!(set["ok"], true, "{set}");

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Plan", "listIds": [work] }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["value"]["priority"], 2);
    assert_eq!(made["value"]["repeating"], "weekly");
    assert_eq!(made["value"]["isAllDay"], false);
    assert!(made["value"]["assigneeId"].is_null());
    let due = made["value"]["dueDateTime"].as_str().expect("a due date");
    // The test clock is 2026-09-07T12:00Z; tomorrow at 17:00 in the clock's own zone.
    let offset = app.clock.utc_offset();
    let expected = "2026-09-08T17:00:00"
        .parse::<chrono::NaiveDateTime>()
        .expect("a time")
        .and_local_timezone(offset)
        .single()
        .expect("a local time")
        .with_timezone(&chrono::Utc);
    assert_eq!(
        crate::model::date::parse(due).expect("parses"),
        expected,
        "{due}"
    );

    // Said at the door: the list's default does not override it.
    let chosen = call(
        &app,
        json!({ "kind": "createTask", "title": "Now", "listIds": [work], "priority": 0 }),
    )
    .await;
    assert_eq!(chosen["value"]["priority"], 0);

    // And the settings answer carries the five, so the flyout can show them.
    let settings = call(&app, json!({ "kind": "listMembers", "listId": work })).await;
    assert_eq!(settings["value"]["defaults"]["priority"], 2);
    assert_eq!(settings["value"]["defaults"]["repeating"], "weekly");
    assert_eq!(settings["value"]["defaults"]["dueDate"], "tomorrow");
    assert_eq!(settings["value"]["defaults"]["dueTime"], "17:00");
    assert_eq!(settings["value"]["defaults"]["assigneeId"], "unassigned");
}

/// The detail's list editor: add, remove, and what it offers in between (task d3f3b111).
#[tokio::test]
async fn a_task_can_be_added_to_and_removed_from_a_list_from_its_editor_task_d3f3b111() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let home = list_id_named(&app, "Home").await;
    let work = list_id_named(&app, "Work").await;
    let created = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": [home] }),
    )
    .await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let picks = call(&app, json!({ "kind": "listPicks", "taskId": task_id })).await;
    assert_eq!(picks["value"]["selected"][0]["name"], "Home");
    assert_eq!(picks["value"]["options"][0]["name"], "Work");
    assert_eq!(picks["value"]["options"].as_array().unwrap().len(), 1);
    assert!(picks["value"]["createName"].is_null());

    let added = call(
        &app,
        json!({ "kind": "addTaskToList", "taskId": task_id, "listId": work }),
    )
    .await;
    assert_eq!(added["ok"], true);
    let picks = call(&app, json!({ "kind": "listPicks", "taskId": task_id })).await;
    assert_eq!(picks["value"]["selected"].as_array().unwrap().len(), 2);
    assert!(picks["value"]["options"].as_array().unwrap().is_empty());

    let removed = call(
        &app,
        json!({ "kind": "removeTaskFromList", "taskId": task_id, "listId": home }),
    )
    .await;
    assert_eq!(removed["ok"], true);
    let picks = call(&app, json!({ "kind": "listPicks", "taskId": task_id })).await;
    assert_eq!(picks["value"]["selected"][0]["name"], "Work");
    assert_eq!(picks["value"]["selected"].as_array().unwrap().len(), 1);
    // The detail agrees: its chips are the same lists.
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
    assert_eq!(detail["value"]["listChips"][0]["name"], "Work");
}

/// Typing a name no list has offers to create it, and creating files the task there with
/// one of the web's colours.
#[tokio::test]
async fn a_list_created_from_the_editor_holds_the_task_and_wears_a_web_colour() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    let created = call(&app, json!({ "kind": "createTask", "title": "Weed" })).await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let picks = call(
        &app,
        json!({ "kind": "listPicks", "taskId": task_id, "query": "Gar" }),
    )
    .await;
    assert_eq!(picks["value"]["createName"], "Gar");
    assert!(picks["value"]["options"].as_array().unwrap().is_empty());

    let made = call(
        &app,
        json!({ "kind": "createListForTask", "taskId": task_id, "name": " Garden " }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["value"]["list"]["name"], "Garden");
    let colour = made["value"]["list"]["color"].as_str().expect("a colour");
    assert!(
        crate::rows::list_picks::LIST_COLOR_PALETTE.contains(&colour),
        "{colour} is not one of the web's"
    );
    assert_eq!(made["value"]["list"]["privacy"], "PRIVATE");

    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
    assert_eq!(detail["value"]["listChips"][0]["name"], "Garden");

    let refused = call(
        &app,
        json!({ "kind": "createListForTask", "taskId": task_id, "name": "  " }),
    )
    .await;
    assert_eq!(refused["ok"], false);
}

#[tokio::test]
async fn a_task_created_through_the_door_appears_in_its_list_rows() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    let lists = call(&app, json!({ "kind": "lists" })).await;
    let list_id = lists["value"][0]["id"].as_str().expect("an id").to_string();

    call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": [list_id] }),
    )
    .await;

    let rows = call(&app, json!({ "kind": "rowsForList", "listId": list_id })).await;
    assert_eq!(rows["value"]["total"], 1);
    assert_eq!(rows["value"]["rows"][0]["title"], "Buy milk");
    assert_eq!(
        rows["value"]["rows"][0]["isPending"], true,
        "it has not reached the server yet, and the row says so"
    );
}

/// Rule 2, enforced at the boundary rather than remembered: the shell cannot complete a task
/// through an update, because that path skips the repeat rollover.
#[tokio::test]
async fn the_shell_cannot_complete_a_task_by_updating_it() {
    let app = app_with(StubTransport::new());
    let created = call(&app, json!({ "kind": "createTask", "title": "Water" })).await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let refused = call(
        &app,
        json!({ "kind": "updateTask", "taskId": task_id, "changes": { "completed": true } }),
    )
    .await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["kind"], "badRequest");
    assert!(
        refused["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("completeTask"),
        "the message has to say what to use instead"
    );
}

/// And the command that IS allowed rolls a repeating task forward rather than finishing it.
#[tokio::test]
async fn completing_a_repeating_task_through_the_door_rolls_it_forward() {
    let app = app_with(StubTransport::new());
    let created = call(
        &app,
        json!({ "kind": "createTask", "title": "Water", "dueDateTime": "2026-09-07T00:00:00Z" }),
    )
    .await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({
            "kind": "updateTask", "taskId": task_id,
            "changes": { "repeating": "daily", "repeatFrom": "DUE_DATE" }
        }),
    )
    .await;

    let completed = call(
        &app,
        json!({ "kind": "completeTask", "taskId": task_id, "completed": true }),
    )
    .await;
    assert_eq!(completed["value"]["completed"], false);
    assert_eq!(completed["value"]["dueDateTime"], "2026-09-08T00:00:00Z");
}

/// Absent leaves a field alone; present-and-null clears it. Losing that distinction in the
/// JSON layer would make "remove the due date" impossible to ask for.
#[tokio::test]
async fn an_explicit_null_clears_and_an_absent_field_does_not() {
    let app = app_with(StubTransport::new());
    let created = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "dueDateTime": "2026-09-08T00:00:00Z" }),
    )
    .await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let renamed = call(
        &app,
        json!({ "kind": "updateTask", "taskId": task_id, "changes": { "title": "Buy oat milk" } }),
    )
    .await;
    assert_eq!(renamed["value"]["dueDateTime"], "2026-09-08T00:00:00Z");

    let cleared = call(
        &app,
        json!({ "kind": "updateTask", "taskId": task_id, "changes": { "dueDateTime": null } }),
    )
    .await;
    assert!(cleared["value"]["dueDateTime"].is_null());
}

/// The whole detail screen in one answer, with the field order that makes four clients agree.
#[tokio::test]
async fn the_detail_screen_arrives_assembled() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    let list_id = call(&app, json!({ "kind": "lists" })).await["value"][0]["id"]
        .as_str()
        .expect("an id")
        .to_string();
    let created = call(
        &app,
        json!({ "kind": "createTask", "title": "Plan the trip", "listIds": [list_id] }),
    )
    .await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    call(
        &app,
        json!({ "kind": "createTask", "title": "Book flights", "parentTaskId": task_id }),
    )
    .await;
    call(
        &app,
        json!({ "kind": "postComment", "taskId": task_id, "content": "asked Sam" }),
    )
    .await;
    call(
        &app,
        json!({ "kind": "updateTask", "taskId": task_id, "changes": { "priority": 3 } }),
    )
    .await;

    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
    let value = &detail["value"];
    assert_eq!(value["task"]["title"], "Plan the trip");
    assert_eq!(
        value["fieldOrder"],
        json!(["assignee", "when", "priority", "lists"]),
        "the order is the contract; changing it is a cross-repo change"
    );
    assert_eq!(value["priorityGlyph"], "!!!");
    assert_eq!(value["listChips"][0]["name"], "Home");
    assert_eq!(value["subtasks"][0]["title"], "Book flights");
    assert_eq!(value["comments"][0]["content"], "asked Sam");
}

/// Project mode has no assignee or priority row — both live behind the leading control.
#[tokio::test]
async fn project_mode_asks_for_a_shorter_detail_screen() {
    let app = app_with(StubTransport::new());
    let created = call(&app, json!({ "kind": "createTask", "title": "x" })).await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let detail = call(
        &app,
        json!({ "kind": "taskDetail", "taskId": task_id, "displayMode": "project" }),
    )
    .await;
    assert_eq!(detail["value"]["fieldOrder"], json!(["when", "lists"]));
}

#[tokio::test]
async fn asking_for_a_task_that_is_not_there_says_which_one() {
    let app = app_with(StubTransport::new());
    let answer = call(&app, json!({ "kind": "taskDetail", "taskId": "nope" })).await;
    assert_eq!(answer["error"]["kind"], "notFound");
    assert_eq!(answer["error"]["id"], "nope");
}

/// The quick picks arrive with the instant each one means, so the shell shows a label and
/// sends back a value it did not have to compute.
#[tokio::test]
async fn the_quick_date_picks_carry_the_instants_they_mean() {
    let app = app_with(StubTransport::new());
    let created = call(
        &app,
        json!({ "kind": "createTask", "title": "Water", "dueDateTime": "2026-09-07T00:00:00Z" }),
    )
    .await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let options = call(&app, json!({ "kind": "dueDateOptions", "taskId": task_id })).await;
    let dates = options["value"]["dates"].as_array().expect("an array");

    // iOS's four, with no clearing row (D26).
    assert_eq!(dates.len(), 4);
    assert_eq!(dates[0]["titleKey"], "picker.today");
    assert_eq!(dates[0]["dueDateTime"], "2026-09-07T00:00:00Z");
    assert_eq!(dates[0]["isSelected"], true, "it is due today");
    assert_eq!(dates[1]["dueDateTime"], "2026-09-08T00:00:00Z");
    assert_eq!(dates[3]["dueDateTime"], "2026-09-14T00:00:00Z");
}

/// The tick goes on the row the task is actually set to, computed the same way the row label
/// is — a picker deciding for itself is how it lands on the wrong row west of UTC.
#[tokio::test]
async fn nothing_is_ticked_when_a_task_has_no_date() {
    let app = app_with(StubTransport::new());
    let created = call(&app, json!({ "kind": "createTask", "title": "Water" })).await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let options = call(&app, json!({ "kind": "dueDateOptions", "taskId": task_id })).await;
    let dates = options["value"]["dates"].as_array().expect("an array");
    // No date means no row is ticked: there is no "No due date" row to tick (D26).
    assert!(dates.iter().all(|date| date["isSelected"] == false));
}

/// An all-day task has no time of day, so no time is ticked until one is chosen.
#[tokio::test]
async fn an_all_day_task_has_no_time_selected() {
    let app = app_with(StubTransport::new());
    let created = call(
        &app,
        json!({ "kind": "createTask", "title": "Water", "dueDateTime": "2026-09-07T00:00:00Z" }),
    )
    .await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let options = call(&app, json!({ "kind": "dueDateOptions", "taskId": task_id })).await;
    assert_eq!(options["value"]["isAllDay"], true);
    let times = options["value"]["times"].as_array().expect("an array");
    assert_eq!(times[0]["titleKey"], "picker.morning");
    assert_eq!(times[0]["dueDateTime"], "2026-09-07T09:00:00Z");
    assert!(times.iter().all(|time| time["isSelected"] == false));
}

/// Search answers with rows, so a result list draws exactly like any other list.
#[tokio::test]
async fn search_answers_with_rows_over_the_cache() {
    let app = app_with(StubTransport::new());
    call(
        &app,
        json!({ "kind": "createTask", "title": "Book flights" }),
    )
    .await;
    call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;

    let found = call(&app, json!({ "kind": "searchTasks", "query": "book" })).await;
    assert_eq!(found["value"]["total"], 1);
    assert_eq!(found["value"]["rows"][0]["title"], "Book flights");
    // A row, with everything a row has.
    assert!(found["value"]["rows"][0]["leading"].is_object());
}

/// AITD-459: iOS searches from the first character; only an empty query finds nothing.
#[tokio::test]
async fn aitd459_search_starts_at_one_character_as_ios_does() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;

    let found = call(&app, json!({ "kind": "searchTasks", "query": "b" })).await;
    assert_eq!(found["value"]["total"], 1);
    let none = call(&app, json!({ "kind": "searchTasks", "query": "" })).await;
    assert_eq!(none["value"]["total"], 0);
}

/// AITD-459: the command answers as iOS's search — a subtask is not a result of its own.
#[tokio::test]
async fn aitd459_search_leaves_subtasks_out_as_ios_does() {
    let app = app_with(StubTransport::new());
    let parent = call(
        &app,
        json!({ "kind": "createTask", "title": "Plan the trip" }),
    )
    .await;
    let parent_id = parent["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({ "kind": "createTask", "title": "Book flights", "parentTaskId": parent_id }),
    )
    .await;
    call(
        &app,
        json!({ "kind": "createTask", "title": "Flights home" }),
    )
    .await;

    let found = call(&app, json!({ "kind": "searchTasks", "query": "flights" })).await;
    assert_eq!(found["value"]["total"], 1);
    assert_eq!(found["value"]["rows"][0]["title"], "Flights home");
}

/// Task `56bf4598`: the command resolves a bare identifier, which only `search_tasks` was tested
/// for. The server is the only minter of an id, so the cache is seeded rather than `createTask`
/// being asked for one.
#[tokio::test]
async fn task_56bf4598_search_resolves_a_bare_identifier() {
    let app = app_with(StubTransport::new());
    let mut numbered = crate::model::Task::new("t1", "Reply routing");
    numbered.identifier = Some("AWTD-1405".into());
    app.store.upsert_task(&numbered).expect("stores");
    app.store
        .upsert_task(&crate::model::Task::new("t2", "Buy milk"))
        .expect("stores");

    // Lowercase in: one identifier must not resolve two different ways.
    let found = call(&app, json!({ "kind": "searchTasks", "query": "awtd-1405" })).await;
    assert_eq!(found["ok"], true, "{found}");
    assert_eq!(found["value"]["total"], 1);
    assert_eq!(found["value"]["rows"][0]["title"], "Reply routing");

    // An id nobody minted is not a search for the words either.
    let missing = call(&app, json!({ "kind": "searchTasks", "query": "awtd-9999" })).await;
    assert_eq!(missing["value"]["total"], 0);
}

/// A file attached here comes back on a comment, so the detail has to carry it. Drawing only
/// the text is what makes attaching look broken from the outside.
#[tokio::test]
async fn a_comment_carries_its_files_into_the_detail() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    let task_id = made["value"]["id"].as_str().expect("an id").to_string();
    let comment: crate::model::Comment = serde_json::from_value(json!({
        "id": "c1",
        "taskId": task_id,
        "content": "",
        "secureFiles": [{
            "id": "f1",
            "originalName": "shot.png",
            "fileSize": 2048,
            "mimeType": "image/png",
        }],
    }))
    .expect("a comment");
    app.store
        .upsert_comments(std::slice::from_ref(&comment))
        .expect("stores");

    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;

    let comments = detail["value"]["comments"].as_array().expect("comments");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["showsText"], false, "no empty bubble");
    assert_eq!(comments[0]["files"][0]["name"], "shot.png");
    assert_eq!(comments[0]["files"][0]["rendersInline"], true);
}

/// A timer survives a restart, because the start time is in the cache rather than in memory —
/// which is the one thing this does that Apple's does not.
#[tokio::test]
async fn a_timer_records_what_the_session_was_worth() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Write it up" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let started = call(&app, json!({ "kind": "startTimer", "taskId": id })).await;
    assert_eq!(started["value"]["isRunning"], true);

    // The clock is fixed in these tests, so the session is zero minutes and records nothing —
    // which is itself the rule: a timer that never ran did not do any work.
    let stopped = call(&app, json!({ "kind": "stopTimer", "taskId": id })).await;
    assert_eq!(stopped["value"]["isRunning"], false);
    assert_eq!(stopped["value"]["loggedMinutes"], 0);

    // And the detail screen carries the state, so the section knows whether to draw.
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": id })).await;
    assert_eq!(detail["value"]["timer"]["isRunning"], false);
}

/// Two clicks on Start should not discard the first ten minutes.
#[tokio::test]
async fn starting_a_timer_that_is_already_running_keeps_the_original_start() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Write it up" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let first = call(&app, json!({ "kind": "startTimer", "taskId": id })).await;
    let again = call(&app, json!({ "kind": "startTimer", "taskId": id })).await;
    assert_eq!(first["value"]["startedAt"], again["value"]["startedAt"]);
}

/// Stopping one that is not running is an ordinary thing to do, not an error.
#[tokio::test]
async fn stopping_a_timer_that_never_started_says_so_quietly() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Write it up" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let stopped = call(&app, json!({ "kind": "stopTimer", "taskId": id })).await;
    assert_eq!(stopped["ok"], true);
    assert_eq!(stopped["value"]["isRunning"], false);
}

/// A cycle is refused by the server, and the refusal reaches the person who picked it — nothing
/// is journalled to be refused again later, unseen (CONTRACTS D32).
#[tokio::test]
async fn a_blocker_that_would_make_a_cycle_is_refused_to_the_person_who_picked_it() {
    let transport = StubTransport::new().push_json(
        "/api/v1/tasks/t1/blockers",
        409,
        json!({ "error": "dependency_cycle" }),
    );
    let app = app_with(transport);
    let added = call(
        &app,
        json!({ "kind": "addTaskBlocker", "taskId": "t1", "blockingTaskId": "t2" }),
    )
    .await;
    assert_eq!(added["ok"], false, "{added}");
    assert!(crate::outbox::journal::all(&app.store)
        .expect("reads")
        .is_empty());
}

/// Online, the server's answer is what the row shows.
#[tokio::test]
async fn a_blocker_added_online_shows_the_servers_answer() {
    let transport = StubTransport::new().push_json(
        "/api/v1/tasks/t1/blockers",
        201,
        json!({ "taskId": "t1", "blockingTaskId": "t2",
                "blockedBy": [{ "id": "t2", "title": "Ship it", "identifier": "AWTD-2" }] }),
    );
    let app = app_with(transport);
    let added = call(
        &app,
        json!({ "kind": "addTaskBlocker", "taskId": "t1", "blockingTaskId": "t2" }),
    )
    .await;
    assert_eq!(
        added["value"]["blockedBy"][0]["title"], "Ship it",
        "{added}"
    );
    assert!(crate::outbox::journal::all(&app.store)
        .expect("reads")
        .is_empty());
}

/// The picker asks the server's search, whose permission filter is in the query; offline it
/// falls back to this machine's cache.
#[tokio::test]
async fn the_blocker_picker_asks_the_server_then_the_cache() {
    let transport = StubTransport::new().push_json(
        "/api/v1/search",
        200,
        json!({ "tasks": [{ "id": "s1", "title": "From the server", "lists": [{ "id": "l1" }] }] }),
    );
    let app = app_with(transport);
    let online = call(
        &app,
        json!({ "kind": "taskBlockerCandidates", "taskId": "t1", "query": "server" }),
    )
    .await;
    let online_ids: Vec<&str> = online["value"]["candidates"]
        .as_array()
        .or_else(|| online["value"].as_array())
        .expect("candidates")
        .iter()
        .filter_map(|candidate| candidate["id"].as_str())
        .collect();
    assert_eq!(online_ids, vec!["s1"], "{online}");

    call(
        &app,
        json!({ "kind": "createTask", "title": "Cached offline task" }),
    )
    .await;
    let offline = call(
        &app,
        json!({ "kind": "taskBlockerCandidates", "taskId": "t1", "query": "offline" }),
    )
    .await;
    let titles: Vec<&str> = offline["value"]["candidates"]
        .as_array()
        .or_else(|| offline["value"].as_array())
        .expect("candidates")
        .iter()
        .filter_map(|candidate| candidate["title"].as_str())
        .collect();
    assert_eq!(titles, vec!["Cached offline task"], "{offline}");
}

/// Neighbours first means the task's own lists, as web ranks (`rankBlockerCandidates` is given
/// `task.lists`) — not every list on its board.
#[tokio::test]
async fn the_blocker_picker_ranks_the_tasks_own_lists_first() {
    let transport = StubTransport::new().push_json(
        "/api/v1/search",
        200,
        json!({ "tasks": [
            { "id": "c1", "title": "Same board, other list", "lists": [{ "id": "doing" }] },
            { "id": "c2", "title": "Same list", "lists": [{ "id": "work" }] }
        ] }),
    );
    let app = app_with(transport);
    app.store
        .upsert_task(
            &serde_json::from_value(json!({ "id": "t1", "title": "Mine", "listIds": ["work"] }))
                .expect("a task"),
        )
        .expect("stores");
    let found = call(
        &app,
        json!({ "kind": "taskBlockerCandidates", "taskId": "t1", "query": "list" }),
    )
    .await;
    let ids: Vec<&str> = found["value"]["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter_map(|candidate| candidate["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["c2", "c1"], "{found}");
}

/// Nobody is signed in, so there is no creator to resolve and the pseudo-list arm must not invent
/// one. Same shape as `signed_out_quick_add_shows_unassigned_task_8aa5732c`, kept beside the fix
/// above so the two halves of the arm are read together.
#[tokio::test]
async fn signed_out_a_task_made_from_my_tasks_stays_unassigned_task_a2320078() {
    let app = app_with(StubTransport::new());

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": ["virtual:my-tasks"] }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert!(made["value"]["assigneeId"].is_null(), "{made}");
}

/// A deliberate "nobody" from My Tasks is still nobody. The pick is said at the door and beats the
/// fallback, exactly as it beats a list's own default.
#[tokio::test]
async fn an_unassigned_pick_from_my_tasks_is_honoured_task_a2320078() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    let made = call(
        &app,
        json!({
            "kind": "createTask",
            "title": "Buy milk",
            "listIds": ["virtual:my-tasks"],
            "assigneeId": "unassigned",
        }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert!(
        made["value"]["assigneeId"].is_null(),
        "the person chose nobody: {made}"
    );
}

/// The other half of the same rule: a list that says `unassigned` means it. That is a choice
/// somebody made on the list's admin tab, and overruling it with "well, you created it" would make
/// the setting impossible to express.
#[tokio::test]
async fn a_list_that_says_unassigned_still_makes_an_unassigned_task_task_e2505d10() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(
                json!({ "id": "l1", "name": "Triage", "defaultAssigneeId": "unassigned" }),
            )
            .expect("a list"),
        )
        .expect("stores");

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": ["l1"] }),
    )
    .await;

    assert_eq!(made["ok"], true, "{made}");
    assert!(
        made["value"]["assigneeId"].is_null(),
        "the list said unassigned: {made}"
    );
}

/// Quick-add's leading control is a PREVIEW of the task about to exist, so it is answered by the
/// same `list_defaults` pass `createTask` runs (task 8aa5732c).
///
/// The failure this guards against is the picker and the create disagreeing. A list with no default
/// assignee makes a task that is MINE (task e2505d10) — so the control above it has to draw a
/// checkbox, not the unassigned mark. A second copy of the rule in the shell, or in this command,
/// is how those two drift apart.
#[tokio::test]
async fn quick_add_previews_the_task_it_would_make_task_8aa5732c() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "defaultPriority": 2 }))
                .expect("a list"),
        )
        .expect("stores");

    let shown = call(&app, json!({ "kind": "quickAddDefaults", "listId": "l1" })).await;
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(
        shown["value"]["leading"]["kind"], "checkbox",
        "a list with no default assignee makes a task that is mine, so the control is a checkbox: \
         {shown}"
    );
    assert_eq!(shown["value"]["assigneeId"], "me", "{shown}");
    assert_eq!(
        shown["value"]["priority"], 2,
        "the list's own default priority, which is what tints the control: {shown}"
    );
    assert!(
        shown["value"]["options"][0]["userId"].is_null(),
        "unassigned is offered first, as in every other picker: {shown}"
    );

    // And the preview told the truth: creating with nothing said produces exactly that.
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Plan", "listIds": ["l1"] }),
    )
    .await;
    assert_eq!(made["value"]["assigneeId"], shown["value"]["assigneeId"]);
    assert_eq!(made["value"]["priority"], shown["value"]["priority"]);
}

/// A list that says `unassigned` is previewed as unassigned — the control shows the list's choice
/// rather than overruling it with "well, you are adding it" (task 8aa5732c).
#[tokio::test]
async fn a_list_that_says_unassigned_previews_unassigned_task_8aa5732c() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(
                json!({ "id": "l1", "name": "Triage", "defaultAssigneeId": "unassigned" }),
            )
            .expect("a list"),
        )
        .expect("stores");

    let shown = call(&app, json!({ "kind": "quickAddDefaults", "listId": "l1" })).await;
    assert_eq!(shown["value"]["leading"]["kind"], "unassigned", "{shown}");
    assert!(shown["value"]["assigneeId"].is_null(), "{shown}");
    assert_eq!(shown["value"]["priority"], 0, "{shown}");
}

/// A list whose default assignee is somebody else draws THEIR mark, which is the third state the
/// control has — and the one the web's `selectedAssignee?.id === currentUser?.id` arm misses if the
/// shell tries to decide it (task 8aa5732c).
#[tokio::test]
async fn a_list_assigned_to_somebody_else_previews_their_mark_task_8aa5732c() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(
                json!({ "id": "l1", "name": "Ops", "defaultAssigneeId": "dana" }),
            )
            .expect("a list"),
        )
        .expect("stores");

    let shown = call(&app, json!({ "kind": "quickAddDefaults", "listId": "l1" })).await;
    assert_eq!(shown["value"]["leading"]["kind"], "avatar", "{shown}");
    assert_eq!(shown["value"]["leading"]["userId"], "dana", "{shown}");
    assert_eq!(shown["value"]["assigneeId"], "dana", "{shown}");
}

/// No list — My Tasks — still offers the reader and still starts on them. A picker that came up
/// empty there is the bug `rows::assignee` already guards for a task with no resolvable lists.
#[tokio::test]
async fn quick_add_without_a_list_is_still_mine_task_8aa5732c() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    let shown = call(&app, json!({ "kind": "quickAddDefaults" })).await;
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(shown["value"]["leading"]["kind"], "checkbox", "{shown}");
    assert_eq!(shown["value"]["assigneeId"], "me", "{shown}");
    assert!(
        shown["value"]["options"].as_array().expect("options").len() >= 2,
        "unassigned and me at the least: {shown}"
    );
}

/// Signed out there is no creator, so the control is the unassigned mark rather than somebody
/// else's face. Same arm as `with_no_known_creator_the_task_stays_unassigned`.
#[tokio::test]
async fn signed_out_quick_add_shows_unassigned_task_8aa5732c() {
    let app = app_with(StubTransport::new());
    let shown = call(&app, json!({ "kind": "quickAddDefaults" })).await;
    assert_eq!(shown["value"]["leading"]["kind"], "unassigned", "{shown}");
    assert!(shown["value"]["assigneeId"].is_null(), "{shown}");
}

/// Which of the three states a PICK produces is the same rule, so the control is re-asked rather
/// than worked out in the shell (task 8aa5732c).
///
/// Choosing somebody else gives their mark; choosing yourself gives the checkbox; choosing
/// unassigned gives the unassigned mark even in a list that would have defaulted to you — a pick is
/// said at the door and beats the default, as it does in `createTask`.
#[tokio::test]
async fn a_pick_in_quick_add_is_previewed_by_the_same_rule_task_8aa5732c() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work" })).expect("a list"),
        )
        .expect("stores");

    let theirs = call(
        &app,
        json!({ "kind": "quickAddDefaults", "listId": "l1", "assigneeId": "dana" }),
    )
    .await;
    assert_eq!(theirs["value"]["leading"]["kind"], "avatar", "{theirs}");
    assert_eq!(theirs["value"]["leading"]["userId"], "dana", "{theirs}");

    let mine = call(
        &app,
        json!({ "kind": "quickAddDefaults", "listId": "l1", "assigneeId": "me" }),
    )
    .await;
    assert_eq!(mine["value"]["leading"]["kind"], "checkbox", "{mine}");

    // Chosen nobody, on purpose — which beats the list's "unset means you". Spelled
    // `"unassigned"` rather than null, because a null cannot be told from "not chosen".
    let nobody = call(
        &app,
        json!({ "kind": "quickAddDefaults", "listId": "l1", "assigneeId": "unassigned" }),
    )
    .await;
    assert_eq!(nobody["value"]["leading"]["kind"], "unassigned", "{nobody}");
    assert!(nobody["value"]["assigneeId"].is_null(), "{nobody}");

    // A chosen priority is the tint, and it survives a list that has its own default.
    let urgent = call(
        &app,
        json!({ "kind": "quickAddDefaults", "listId": "l1", "priority": 3 }),
    )
    .await;
    assert_eq!(urgent["value"]["priority"], 3, "{urgent}");
}

/// The preview and the create must read the sentinel the same way (task 8aa5732c).
///
/// `quickAddDefaults` translated `"unassigned"` to nobody; `createTask` did not, and stored it as
/// though it were a user id — so a quick-add that previewed the unassigned mark produced a task
/// assigned to a person called `unassigned`, which is the one thing the preview exists to prevent.
/// Sending `null` instead is not the fix: an absent assignee is unsaid, and the list's default
/// would then overrule a deliberate choice of nobody. The sentinel is how "nobody, on purpose" is
/// spelled, and it has to mean that at both doors.
#[tokio::test]
async fn create_reads_the_unassigned_sentinel_as_the_preview_does_task_8aa5732c() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    // A list with no default assignee, so "unset means the creator" is what a plain create does —
    // which is exactly what a choice of nobody has to beat.
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work" })).expect("a list"),
        )
        .expect("stores");

    let previewed = call(
        &app,
        json!({ "kind": "quickAddDefaults", "listId": "l1", "assigneeId": "unassigned" }),
    )
    .await;
    assert!(previewed["value"]["assigneeId"].is_null(), "{previewed}");

    let made = call(
        &app,
        json!({
            "kind": "createTask",
            "title": "Nobody's job",
            "listIds": ["l1"],
            "assigneeId": "unassigned",
        }),
    )
    .await;

    assert_eq!(made["ok"], true, "{made}");
    assert!(
        made["value"]["assigneeId"].is_null(),
        "the sentinel is a choice, not a user id: {made}"
    );
    // And it really was a choice — the list's "unset means you" did not fill it back in.
    assert_ne!(made["value"]["assigneeId"], "me", "{made}");
}

/// `createTask` can say which status a task is born with (task 95c7a68f). The draft and the
/// service already carried it; the command was the only thing that could not express it.
#[tokio::test]
async fn create_task_can_carry_a_status_role_task_95c7a68f() {
    let app = app_with(StubTransport::new());

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Write it down", "statusRole": "doing" }),
    )
    .await;

    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["value"]["statusRole"], "doing");
}

/// The detail carries a BOARD STATE row for a task on a board (task 5221e43f): the board's
/// columns as chips with Done left out and the current one lit, in list mode only, and never
/// for a task with no board column. The rule is the one the Mac, iOS and the web share.
#[tokio::test]
async fn the_detail_shows_the_board_state_of_a_board_task_in_list_mode_task_5221e43f() {
    let app = app_with(StubTransport::new());
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "projectId": "p1" }))
                .expect("a list"),
        )
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l2", "name": "Home" })).expect("a list"),
        )
        .expect("stores");
    let mut on_board = crate::model::Task::new("t1", "Water plants");
    on_board.list_ids = Some(vec!["l1".into()]);
    on_board.status_role = Some("doing".into());
    app.store.upsert_task(&on_board).expect("stores");
    let mut off_board = crate::model::Task::new("t2", "Buy milk");
    off_board.list_ids = Some(vec!["l2".into()]);
    app.store.upsert_task(&off_board).expect("stores");

    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": "t1" })).await;
    assert_eq!(detail["ok"], true, "{detail}");
    let state = &detail["value"]["boardState"];
    assert_eq!(state["current"], "doing");
    let names: Vec<&str> = state["chips"]
        .as_array()
        .expect("chips")
        .iter()
        .map(|chip| chip["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(
        names,
        ["Inbox", "Ready", "Doing", "Waiting"],
        "Done is the checkbox's job"
    );
    assert_eq!(state["chips"][2]["isCurrent"], true);

    let compact = call(
        &app,
        json!({ "kind": "taskDetail", "taskId": "t1", "displayMode": "project" }),
    )
    .await;
    assert!(
        compact["value"]["boardState"].is_null(),
        "project mode already offers the state behind the leading control"
    );

    let plain = call(&app, json!({ "kind": "taskDetail", "taskId": "t2" })).await;
    assert!(
        plain["value"]["boardState"].is_null(),
        "a task with no board column gets no row"
    );
}

/// The detail's menu: Won't do closes with a reason and no rollover, Reopen clears it, the
/// status choices are the board's own columns, and choosing one is the same move a dragged card
/// makes — including that Done means completed (task 016ce981).
#[tokio::test]
async fn the_action_menu_closes_as_wont_do_and_sets_status_like_the_board_task_016ce981() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "/api/v1/shortcodes",
        200,
        json!({ "url": "https://astrid.cc/s/abc123" }),
    ));
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "projectId": "p1" }))
                .expect("a list"),
        )
        .expect("stores");
    let mut task = crate::model::Task::new("t1", "Water plants");
    task.list_ids = Some(vec!["l1".into()]);
    task.repeating = Some(crate::model::Repeating::Daily);
    task.due_date_time = crate::model::date::parse("2026-09-07T09:00:00Z");
    app.store.upsert_task(&task).expect("stores");

    // Won't do: closed, with the reason, where it stands — no rollover.
    let closed = call(
        &app,
        json!({ "kind": "setClosedReason", "taskId": "t1", "closedReason": "canceled" }),
    )
    .await;
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(closed["value"]["completed"], true);
    assert_eq!(closed["value"]["closedReason"], "canceled");
    assert!(closed["value"]["dueDateTime"]
        .as_str()
        .expect("a due date")
        .starts_with("2026-09-07"));
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": "t1" })).await;
    assert_eq!(detail["value"]["isCanceled"], true);
    assert_eq!(detail["value"]["link"], "https://astrid.cc/tasks/t1");

    let refused = call(
        &app,
        json!({ "kind": "setClosedReason", "taskId": "t1", "closedReason": "meh" }),
    )
    .await;
    assert_eq!(
        refused["ok"], false,
        "a typo must not become 'completed normally'"
    );

    // Reopen clears both.
    let reopened = call(
        &app,
        json!({ "kind": "setClosedReason", "taskId": "t1", "closedReason": null }),
    )
    .await;
    assert_eq!(reopened["value"]["completed"], false);
    assert!(reopened["value"]["closedReason"].is_null());

    // The menu's columns are the board's, resolved from the task's own list.
    let options = call(&app, json!({ "kind": "taskStatusOptions", "taskId": "t1" })).await;
    let names: Vec<&str> = options["value"]["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|column| column["name"].as_str().expect("a name"))
        .collect();
    // Never Done: the menu sits beside Complete, which is what finishing is (AITD-461, D46).
    assert_eq!(names, ["Inbox", "Ready", "Doing", "Waiting"]);
    assert_eq!(options["value"]["current"], "__virtual_inbox__");
    assert_eq!(options["value"]["columns"][0]["isCurrent"], true);

    // Choosing Doing is the move a dragged card makes.
    let doing = call(
        &app,
        json!({ "kind": "setTaskStatus", "taskId": "t1", "columnId": "doing" }),
    )
    .await;
    assert_eq!(doing["ok"], true, "{doing}");
    assert_eq!(doing["value"]["statusRole"], "doing");
    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    let columns = board["value"]["columns"].as_array().expect("columns");
    let in_doing = columns
        .iter()
        .find(|column| column["id"] == "doing")
        .expect("a doing column");
    assert_eq!(in_doing["total"], 1);

    // Done means completed, through the completion service — so the daily task rolls forward
    // exactly as it would when its card is dragged there.
    let done = call(
        &app,
        json!({ "kind": "setTaskStatus", "taskId": "t1", "columnId": "__virtual_done__" }),
    )
    .await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(
        done["value"]["completed"], false,
        "a repeating task rolls to its next occurrence"
    );
    assert!(done["value"]["dueDateTime"]
        .as_str()
        .expect("a due date")
        .starts_with("2026-09-08"));
    assert!(done["value"]["statusRole"].is_null());

    // Share mints a link on the server and hands back its address.
    let shared = call(&app, json!({ "kind": "shareTask", "taskId": "t1" })).await;
    assert_eq!(shared["ok"], true, "{shared}");
    assert_eq!(shared["value"]["url"], "https://astrid.cc/s/abc123");
    let minted = transport
        .requests()
        .into_iter()
        .find(|request| request.url.ends_with("/api/v1/shortcodes"))
        .expect("a shortcode request");
    let body: serde_json::Value =
        serde_json::from_slice(minted.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["targetType"], "task");
    assert_eq!(body["targetId"], "t1");
}

/// A repeat describes itself in parts with resource keys, so the shell says it in its own
/// words and in its own order.
#[tokio::test]
async fn repeat_options_describe_the_current_repeat() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Water plants" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({
            "kind": "updateTask",
            "taskId": id,
            "changes": { "repeating": "weekly" },
        }),
    )
    .await;

    let offered = call(&app, json!({ "kind": "repeatOptions", "taskId": id })).await;
    assert_eq!(offered["value"]["presets"][0]["value"], "never");
    assert_eq!(offered["value"]["summary"][0]["key"], "repeating.weekly");
    let selected = offered["value"]["presets"]
        .as_array()
        .expect("presets")
        .iter()
        .filter(|preset| preset["isSelected"] == true)
        .count();
    assert_eq!(selected, 1);
}

/// The detail screen carries the same sentence the picker shows, from the same function.
#[tokio::test]
async fn the_detail_screen_describes_the_repeat_the_same_way() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Water plants" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({
            "kind": "updateTask",
            "taskId": id,
            "changes": { "repeating": "daily" },
        }),
    )
    .await;

    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": id })).await;
    assert_eq!(
        detail["value"]["repeatSummary"][0]["key"],
        "repeating.daily"
    );
}

/// The picker offers unassigned first and knows which row is the current one, so the shell
/// draws a tick beside it without deciding anything.
#[tokio::test]
async fn assignee_options_offer_unassigned_and_say_who_holds_the_task() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Book flights" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let offered = call(&app, json!({ "kind": "assigneeOptions", "taskId": id })).await;
    assert_eq!(
        offered["value"]["options"][0]["userId"],
        serde_json::Value::Null
    );
    assert_eq!(offered["value"]["assigneeId"], serde_json::Value::Null);
}

#[tokio::test]
async fn assignee_options_for_a_task_that_is_not_there_are_a_not_found() {
    let app = app_with(StubTransport::new());
    let answered = call(&app, json!({ "kind": "assigneeOptions", "taskId": "nope" })).await;
    assert_eq!(answered["error"]["kind"], "notFound");
}

/// A task in a public list the reader may not edit — a passer-by's view of it — answers a copy
/// control on its row and in its detail (task f6bc59e8); the reader's own list answers the
/// checkbox.
#[tokio::test]
async fn a_public_task_the_reader_cannot_edit_is_a_copy_control_task_f6bc59e8() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({
                "id": "pub", "name": "Recipes", "ownerId": "them", "privacy": "PUBLIC",
            }))
            .expect("a list"),
        )
        .expect("stores");
    app.store
        .upsert_task(
            &serde_json::from_value(
                json!({ "id": "t1", "title": "Bake", "listIds": ["pub"], "creatorId": "them" }),
            )
            .expect("a task"),
        )
        .expect("stores");
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    let home = list_id_named(&app, "Home").await;
    call(
        &app,
        json!({ "kind": "createTask", "title": "Sweep", "listIds": [home] }),
    )
    .await;

    let theirs = call(&app, json!({ "kind": "rowsForList", "listId": "pub" })).await;
    assert_eq!(
        theirs["value"]["rows"][0]["leading"]["kind"], "copy",
        "{theirs}"
    );
    assert_eq!(theirs["value"]["rows"][0]["action"], "copy");
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": "t1" })).await;
    assert_eq!(detail["value"]["isCopyOnly"], true);

    let mine = call(&app, json!({ "kind": "rowsForList", "listId": home })).await;
    // A checkbox, which is what this test's own description always claimed and what the assertion
    // did not say: a task I just made in my own list is mine. It read "unassigned" because the
    // creator was left to the server (task e2505d10).
    assert_eq!(
        mine["value"]["rows"][0]["leading"]["kind"], "checkbox",
        "{mine}"
    );
    assert_eq!(mine["value"]["rows"][0]["action"], "complete");
}

/// A shell that applies a list's defaults itself — the Apple apps, until their services are this
/// crate's — must be able to say so, or a task it made with no date would be given the list's.
/// The same switch lets a sync provider import a task exactly as it was.
#[tokio::test]
async fn a_shell_can_create_a_task_without_the_list_s_defaults() {
    let app = app_with(StubTransport::new().push_json("/members", 200, json!({ "members": [] })));
    call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let work = list_id_named(&app, "Work").await;
    call(
        &app,
        json!({ "kind": "updateList", "listId": work,
                "changes": { "defaultPriority": 2, "defaultDueDate": "tomorrow" } }),
    )
    .await;

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "As it was", "listIds": [work],
                "applyListDefaults": false, "repeating": "daily", "isPrivate": false,
                "repeatFrom": "DUE_DATE" }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert!(made["value"]["dueDateTime"].is_null(), "{made}");
    assert_eq!(made["value"]["priority"], 0);
    assert_eq!(made["value"]["repeating"], "daily");
    assert_eq!(made["value"]["repeatFrom"], "DUE_DATE");
    assert_eq!(made["value"]["isPrivate"], false);
}

/// Rule 3 through the door: a view that let the person move the due date first completes the task
/// as they see it, so the series rolls from the date on screen, not the cache's.
#[tokio::test]
async fn completing_rolls_from_the_task_as_the_person_sees_it() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Water plants", "repeating": "daily",
                "repeatFrom": "DUE_DATE", "dueDateTime": "2026-09-07T09:00:00Z",
                "isAllDay": false }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    let mut on_screen = made["value"].clone();
    on_screen["dueDateTime"] = json!("2026-09-10T09:00:00Z");

    let done = call(
        &app,
        json!({ "kind": "completeTask", "taskId": id, "completed": true, "task": on_screen }),
    )
    .await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["value"]["dueDateTime"], "2026-09-11T09:00:00Z");
    assert_eq!(done["value"]["completed"], false);
}

/// A tick brought in from elsewhere keeps its own time and says where it came from, so a sync
/// provider can tell its echo from a person's.
#[tokio::test]
async fn a_completion_from_elsewhere_keeps_its_time_and_names_its_source() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Imported" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let done = call(
        &app,
        json!({ "kind": "completeTask", "taskId": id, "completed": true,
                "completedAt": "2026-09-06T08:30:00Z", "source": "google",
                "timerDuration": 300 }),
    )
    .await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["value"]["completed"], true);
    assert_eq!(done["value"]["completedAt"], "2026-09-06T08:30:00Z");
    assert_eq!(done["value"]["completedSource"], "google");
    assert_eq!(done["value"]["timerDuration"], 300);
}

/// The wire-shape read a shell with its own models binds to: every task, or the named ones, with a
/// temporary id reading as the task it is.
#[tokio::test]
async fn tasks_answers_every_cached_task_or_the_named_ones() {
    let app = app_with(StubTransport::new());
    let a = call(&app, json!({ "kind": "createTask", "title": "A" })).await;
    call(&app, json!({ "kind": "createTask", "title": "B" })).await;
    let a_id = a["value"]["id"].as_str().expect("an id").to_string();

    let all = call(&app, json!({ "kind": "tasks" })).await;
    assert_eq!(all["value"].as_array().map(Vec::len), Some(2), "{all}");

    let named = call(&app, json!({ "kind": "tasks", "ids": [a_id, "nobody"] })).await;
    assert_eq!(named["value"].as_array().map(Vec::len), Some(1), "{named}");
    assert_eq!(named["value"][0]["title"], "A");
}

/// A shell that holds a temporary id learns the real one once the create lands, and only the ids
/// that moved come back.
#[tokio::test]
async fn resolve_ids_names_only_the_ids_that_moved() {
    let app = app_with(StubTransport::new());
    app.store
        .record_id_mapping("temp_a", "real-a", app.clock.now())
        .expect("records");
    let answer = call(
        &app,
        json!({ "kind": "resolveIds", "ids": ["temp_a", "temp_b", "real-c"] }),
    )
    .await;
    assert_eq!(answer["value"], json!({ "temp_a": "real-a" }));
}

/// Task detail reads the schema once per project and the values from the task's own route, and
/// draws the unmapped fields in order with this task's values. A classic task has no rows.
#[tokio::test]
async fn aitd_491_a_github_task_shows_its_project_fields_and_a_classic_one_none() {
    let transport = StubTransport::new()
        .push_json("/api/v1/projects/p1/fields", 200, project_schema())
        .push_json(
            "/api/v1/tasks/t1",
            200,
            json!({ "task": { "id": "t1", "fields": { "f-team": "o-ios", "f-size": 5 } } }),
        );
    let app = app_with(transport);
    on_a_github_board(&app);

    let fields = call(&app, json!({ "kind": "taskProjectFields", "taskId": "t1" })).await;
    assert_eq!(fields["ok"], true, "{fields}");
    let rows = fields["value"]["rows"].as_array().expect("rows");
    assert_eq!(
        rows.len(),
        2,
        "the mapped Status field is not drawn: {fields}"
    );
    assert_eq!(rows[0]["name"], "Team");
    assert_eq!(rows[0]["value"], "o-ios");
    assert_eq!(rows[0]["options"][0]["name"], "iOS");
    assert_eq!(rows[1]["name"], "Size");
    assert_eq!(rows[1]["value"], 5);

    // Offline now (nothing more is stubbed): the cache answers, unchanged.
    let again = call(&app, json!({ "kind": "taskProjectFields", "taskId": "t1" })).await;
    assert_eq!(again["value"], fields["value"], "{again}");

    let classic = call(&app, json!({ "kind": "taskProjectFields", "taskId": "t2" })).await;
    assert_eq!(classic["ok"], true, "{classic}");
    assert!(classic["value"]["rows"]
        .as_array()
        .expect("rows")
        .is_empty());
}

/// Online, an edit is sent at once and the server's value is kept; a refusal reaches the person
/// who made it, by its code, and nothing is journalled to be refused again unseen.
#[tokio::test]
async fn aitd_491_a_field_edit_online_is_sent_and_a_refusal_comes_back_by_its_code() {
    let transport = StubTransport::new()
        .push_json("/api/v1/projects/p1/fields", 200, project_schema())
        .push_json(
            "/api/v1/tasks/t1/fields/f-size",
            200,
            json!({ "field": { "id": "f-size", "value": 8 } }),
        )
        .push_json(
            "/api/v1/tasks/t1/fields/f-team",
            // 403, never 401: a 401 from Astrid means the Astrid session (web lib/github/projects/http.ts).
            403,
            json!({ "error": "sso_required", "ssoUrl": "https://github.com/orgs/x/sso" }),
        )
        .push_json("/api/v1/tasks/t1", 200, json!({ "task": { "id": "t1" } }));
    let app = app_with(transport);
    on_a_github_board(&app);
    let _ = call(&app, json!({ "kind": "taskProjectFields", "taskId": "t1" })).await;

    let set = call(
        &app,
        json!({ "kind": "setTaskProjectField", "taskId": "t1", "fieldId": "f-size", "value": 8 }),
    )
    .await;
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["value"]["rows"][1]["value"], 8, "{set}");

    let refused = call(
        &app,
        json!({ "kind": "setTaskProjectField", "taskId": "t1", "fieldId": "f-team", "value": "o-ios" }),
    )
    .await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused.to_string().contains("sso_required"),
        "the code the shell picks its words from: {refused}"
    );
    assert!(crate::outbox::journal::all(&app.store)
        .expect("reads")
        .is_empty());

    // The wrong JSON type is refused here, before anything is sent.
    let mismatched = call(
        &app,
        json!({ "kind": "setTaskProjectField", "taskId": "t1", "fieldId": "f-size", "value": "eight" }),
    )
    .await;
    assert_eq!(mismatched["ok"], false, "{mismatched}");
}

/// Offline, an edit — a clear included — shows at once and waits in the journal.
#[tokio::test]
async fn aitd_491_a_field_edit_offline_shows_at_once_and_is_journalled() {
    let transport = StubTransport::new()
        .push_json("/api/v1/projects/p1/fields", 200, project_schema())
        .push_json(
            "/api/v1/tasks/t1",
            200,
            json!({ "task": { "id": "t1", "fields": { "f-team": "o-ios" } } }),
        );
    let app = app_with(transport);
    on_a_github_board(&app);
    let _ = call(&app, json!({ "kind": "taskProjectFields", "taskId": "t1" })).await;

    let cleared = call(
        &app,
        json!({ "kind": "setTaskProjectField", "taskId": "t1", "fieldId": "f-team", "value": null }),
    )
    .await;
    assert_eq!(cleared["ok"], true, "{cleared}");
    assert!(cleared["value"]["rows"][0]["value"].is_null(), "{cleared}");
    let journal = crate::outbox::journal::all(&app.store).expect("reads");
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].kind, "setTaskField");
    assert_eq!(journal[0].payload["fieldId"], "f-team");
    assert!(journal[0].payload["body"]["value"].is_null());
}

/// AATD-25: a picked time lands on the task's own day — for an all-day task, its calendar date.
#[tokio::test]
async fn aatd_25_a_picked_time_lands_on_the_tasks_own_day() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Dentist" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({ "kind": "updateTask", "taskId": id, "changes": { "dueDateTime": "2026-12-25T00:00:00Z", "isAllDay": true } }),
    )
    .await;
    let at = call(
        &app,
        json!({ "kind": "dueDateAtTime", "taskId": id, "hour": 14, "minute": 30 }),
    )
    .await;
    assert_eq!(at["value"]["isAllDay"], false, "{at}");
    let due = at["value"]["dueDateTime"].as_str().expect("a date");
    assert!(
        due.starts_with("2026-12-25T14:30"),
        "UTC test clock: the 25th at 14:30, got {due}"
    );
}

/// Task 0616f2d1: taking the time off a task keeps the day the reader sees it on. West of UTC the
/// stored instant is already the next UTC day, so writing it back with `isAllDay: true` — the
/// shortcut nothing rejects — would move the task to tomorrow.
#[tokio::test]
async fn task_0616f2d1_clearing_a_time_keeps_the_readers_day_west_of_utc() {
    let app = app_in_zone(-7);
    let made = call(&app, json!({ "kind": "createTask", "title": "Dentist" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    // 20:00 on the 14th in California is 03:00 on the 15th in UTC.
    call(
        &app,
        json!({ "kind": "updateTask", "taskId": id, "changes": { "dueDateTime": "2026-12-15T03:00:00Z", "isAllDay": false } }),
    )
    .await;

    let cleared = call(&app, json!({ "kind": "dueDateAllDay", "taskId": id })).await;
    assert_eq!(cleared["ok"], true, "{cleared}");
    assert_eq!(cleared["value"]["isAllDay"], true, "{cleared}");
    assert_eq!(
        cleared["value"]["dueDateTime"], "2026-12-14T00:00:00Z",
        "the 14th, the day it was shown on — not the 15th: {cleared}"
    );
}

/// The mirror image east of UTC, where the stored instant is the PREVIOUS UTC day.
#[tokio::test]
async fn task_0616f2d1_clearing_a_time_keeps_the_readers_day_east_of_utc() {
    let app = app_in_zone(9);
    let made = call(&app, json!({ "kind": "createTask", "title": "Breakfast" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    // 07:00 on the 15th in Tokyo is 22:00 on the 14th in UTC.
    call(
        &app,
        json!({ "kind": "updateTask", "taskId": id, "changes": { "dueDateTime": "2026-12-14T22:00:00Z", "isAllDay": false } }),
    )
    .await;

    let cleared = call(&app, json!({ "kind": "dueDateAllDay", "taskId": id })).await;
    assert_eq!(
        cleared["value"]["dueDateTime"], "2026-12-15T00:00:00Z",
        "the 15th, the day it was shown on — not the 14th: {cleared}"
    );
    assert_eq!(cleared["value"]["isAllDay"], true, "{cleared}");
}

/// A task that is already all-day has nothing to clear, and must not drift: west of UTC its stored
/// midnight reads as the previous afternoon locally, so a second tap would walk the date back.
#[tokio::test]
async fn task_0616f2d1_an_all_day_task_does_not_move() {
    let app = app_in_zone(-7);
    let made = call(&app, json!({ "kind": "createTask", "title": "Christmas" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({ "kind": "updateTask", "taskId": id, "changes": { "dueDateTime": "2026-12-25T00:00:00Z", "isAllDay": true } }),
    )
    .await;

    let cleared = call(&app, json!({ "kind": "dueDateAllDay", "taskId": id })).await;
    assert_eq!(
        cleared["value"]["dueDateTime"], "2026-12-25T00:00:00Z",
        "{cleared}"
    );
    assert_eq!(cleared["value"]["isAllDay"], true, "{cleared}");
}

/// A task with no date has no day to keep, so the answer carries no date rather than inventing
/// today — the shell is turning a time off, not setting a due date.
#[tokio::test]
async fn task_0616f2d1_a_task_with_no_date_keeps_none() {
    let app = app_in_zone(-7);
    let made = call(&app, json!({ "kind": "createTask", "title": "Someday" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let cleared = call(&app, json!({ "kind": "dueDateAllDay", "taskId": id })).await;
    assert_eq!(cleared["ok"], true, "{cleared}");
    assert_eq!(
        cleared["value"]["dueDateTime"],
        serde_json::Value::Null,
        "{cleared}"
    );
    assert_eq!(cleared["value"]["isAllDay"], true, "{cleared}");
}

/// A task the cache has not got answers notFound, like every other task command.
#[tokio::test]
async fn task_0616f2d1_clearing_a_time_on_an_unknown_task_says_not_found() {
    let app = app_in_zone(0);
    let missing = call(&app, json!({ "kind": "dueDateAllDay", "taskId": "nope" })).await;
    assert_eq!(missing["ok"], false, "{missing}");
}

/// AITD-561: the quick time picks and the time picker answer with the target day's offset. Read
/// in Los Angeles on 2026-10-27 (PDT), "Morning" on a task due Sunday 2026-11-01 (PST) is
/// 09:00 PST — 17:00Z — not 09:00 PDT, which the phone shows as 08:00.
#[tokio::test]
async fn aitd_561_morning_across_a_dst_change_is_nine_local() {
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
        std::sync::Arc::new(
            crate::platform::FixedClock::parsed("2026-10-27T19:00:00Z")
                .in_named_zone(chrono_tz::America::Los_Angeles),
        ),
    )
    .expect("starts");
    let made = call(&app, json!({ "kind": "createTask", "title": "Dentist" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();
    // 03:37 PST on Sunday the 1st.
    call(
        &app,
        json!({ "kind": "updateTask", "taskId": id, "changes": { "dueDateTime": "2026-11-01T11:37:00Z", "isAllDay": false } }),
    )
    .await;

    let options = call(&app, json!({ "kind": "dueDateOptions", "taskId": id })).await;
    assert_eq!(
        options["value"]["times"][0]["dueDateTime"], "2026-11-01T17:00:00Z",
        "{options}"
    );
    let picked = call(
        &app,
        json!({ "kind": "dueDateAtTime", "taskId": id, "hour": 9, "minute": 0 }),
    )
    .await;
    assert_eq!(
        picked["value"]["dueDateTime"], "2026-11-01T17:00:00Z",
        "{picked}"
    );
}

/// AATD-42: a row dropped on another becomes its subtask; pulled to the gutter it climbs back out.
#[tokio::test]
async fn aatd42_nest_task_writes_the_parent_the_drop_means() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    call(
        &app,
        json!({ "kind": "seedCache", "tasks": [
            { "id": "a", "title": "Paint", "listIds": ["l1"] },
            { "id": "b", "title": "Buy brushes", "listIds": ["l1"] }
        ] }),
    )
    .await;
    let nested = call(
        &app,
        json!({ "kind": "nestTask", "taskId": "b", "zone": "row", "targetId": "a" }),
    )
    .await;
    assert_eq!(nested["value"]["outcome"], "makeSubtask", "{nested}");
    let task = call(&app, json!({ "kind": "task", "taskId": "b" })).await;
    assert_eq!(task["value"]["parentTaskId"], "a", "{task}");
    let out = call(
        &app,
        json!({ "kind": "nestTask", "taskId": "b", "zone": "outdent" }),
    )
    .await;
    assert_eq!(out["value"]["outcome"], "moveToTopLevel", "{out}");
    let task = call(&app, json!({ "kind": "task", "taskId": "b" })).await;
    assert!(task["value"]["parentTaskId"].is_null(), "{task}");
    let reorder = call(
        &app,
        json!({ "kind": "nestTask", "taskId": "a", "zone": "line" }),
    )
    .await;
    assert_eq!(reorder["value"]["outcome"], "reorderOnly");
}

/// AATD-51: tapping people in a multi-assignee picker adds and removes them, primary first.
#[tokio::test]
async fn aatd51_toggle_assignee_adds_and_removes_people() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    call(&app, json!({ "kind": "seedCache", "tasks": [ { "id": "t1", "title": "Ship", "assigneeId": "me" } ] })).await;
    let added = call(
        &app,
        json!({ "kind": "toggleAssignee", "taskId": "t1", "userId": "bo" }),
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let options = call(&app, json!({ "kind": "assigneeOptions", "taskId": "t1" })).await;
    assert_eq!(
        options["value"]["assigneeIds"],
        json!(["me", "bo"]),
        "{options}"
    );
    call(
        &app,
        json!({ "kind": "toggleAssignee", "taskId": "t1", "userId": "me" }),
    )
    .await;
    let options = call(&app, json!({ "kind": "assigneeOptions", "taskId": "t1" })).await;
    assert_eq!(options["value"]["assigneeIds"], json!(["bo"]), "{options}");
}
