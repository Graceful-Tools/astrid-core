//! Boards, their columns, and the cards on them.

use super::*;

/// A board's columns can be added, renamed, reordered and removed from here, the board
/// redraws, and a name the web would refuse is refused with the web's words (task e5214fba).
#[tokio::test]
async fn board_columns_are_managed_through_the_door_task_e5214fba() {
    let review = json!({ "role": "custom-review", "name": "Review", "order": 0 });
    let renamed = json!({ "role": "custom-review", "name": "In review", "order": 0 });
    // The three writes, in order. The projects re-fetch after each is deliberately NOT
    // stubbed: the cache carries the rule's result before the refresh, so an unanswered
    // refresh changes nothing — and a stub matched by URL fragment would match the writes'
    // own URL too, which is a race, not a script.
    let transport = StubTransport::new()
        .push_json("/p1/statuses", 200, json!({ "state": review }))
        .push_json("/p1/statuses", 200, json!({ "state": renamed }))
        .push_json("/p1/statuses", 200, json!({ "state": renamed }))
        .push_json("/members", 200, json!({ "members": [] }));
    let (app, transport) = app_and_transport(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "projectId": "p1" }))
                .expect("a list"),
        )
        .expect("stores");
    app.store
        .upsert_projects(&[
            serde_json::from_value(json!({ "id": "p1", "name": "Work" })).expect("a project"),
        ])
        .expect("stores");

    let added = call(
        &app,
        json!({ "kind": "addBoardStatus", "listId": "l1", "name": " Review " }),
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(added["value"]["state"]["role"], "custom-review");
    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    let names: Vec<&str> = board["value"]["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|column| column["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        vec!["Inbox", "Ready", "Doing", "Waiting", "Review", "Done"]
    );

    // Refused before any round trip, in the web's words.
    let refused = call(
        &app,
        json!({ "kind": "addBoardStatus", "listId": "l1", "name": "Ready" }),
    )
    .await;
    assert_eq!(refused["ok"], false);
    assert_eq!(
        refused["error"]["message"],
        "\"Ready\" is a built-in status"
    );
    let kept = call(
        &app,
        json!({ "kind": "removeBoardStatus", "listId": "l1", "role": "doing" }),
    )
    .await;
    assert_eq!(
        kept["error"]["message"],
        "Built-in statuses cannot be removed"
    );

    let renamed_answer = call(
            &app,
            json!({ "kind": "renameBoardStatus", "listId": "l1", "role": "custom-review", "name": "In review" }),
        )
        .await;
    assert_eq!(renamed_answer["ok"], true, "{renamed_answer}");
    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    assert_eq!(board["value"]["columns"][4]["name"], "In review");
    assert_eq!(
        board["value"]["columns"][4]["id"], "custom-review",
        "a rename keeps the role"
    );

    let removed = call(
        &app,
        json!({ "kind": "removeBoardStatus", "listId": "l1", "role": "custom-review" }),
    )
    .await;
    assert_eq!(removed["ok"], true, "{removed}");
    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    assert_eq!(
        board["value"]["columns"].as_array().expect("columns").len(),
        5
    );

    // The settings answer lists the editable columns, saying which are built in.
    let settings = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
    assert_eq!(settings["value"]["projectId"], "p1");
    let statuses = settings["value"]["statuses"].as_array().expect("statuses");
    assert_eq!(statuses.len(), 3);
    assert_eq!(statuses[0]["id"], "ready");
    assert_eq!(statuses[0]["isDefault"], true);

    // Every write went to the board's versioned statuses route.
    let sent: Vec<String> = transport
        .requests()
        .into_iter()
        .filter(|request| request.url.contains("/api/v1/projects/p1/statuses"))
        .map(|request| request.method.as_str().to_string())
        .collect();
    assert_eq!(sent, vec!["POST", "PATCH", "DELETE"]);

    // Until the web ships that route, a 404 reads as what it is.
    let (app, _) = app_and_transport(StubTransport::new().push_json(
        "/p1/statuses",
        404,
        json!({ "error": "Not found" }),
    ));
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "projectId": "p1" }))
                .expect("a list"),
        )
        .expect("stores");
    app.store
        .upsert_projects(&[
            serde_json::from_value(json!({ "id": "p1", "name": "Work" })).expect("a project"),
        ])
        .expect("stores");
    let missing = call(
        &app,
        json!({ "kind": "addBoardStatus", "listId": "l1", "name": "Review" }),
    )
    .await;
    assert_eq!(missing["ok"], false);
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("does not manage board columns yet"),
        "{missing}"
    );
}

/// Making a board from a list: one request, and the board has its columns at once — the seeded
/// status lists cached, the list attached (Apple `createBoardForList`, 2026-05-12).
#[tokio::test]
async fn a_board_made_from_a_list_has_its_columns_at_once() {
    let transport = StubTransport::new().push_json(
        "/api/v1/projects/from-list",
        201,
        json!({ "project": {
            "id": "p1", "name": "Work",
            "lists": [
                { "id": "l1", "name": "Work", "listType": "regular" },
                { "id": "s1", "name": "Doing", "listType": "status" }
            ]
        } }),
    );
    let app = app_with(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "color": "#ff0000" }))
                .expect("a list"),
        )
        .expect("stores");

    let made = call(
        &app,
        json!({ "kind": "createBoardForList", "listId": "l1" }),
    )
    .await;
    assert_eq!(made["value"]["id"], "p1", "{made}");
    let attached = app.store.list("l1").expect("reads").expect("cached");
    assert_eq!(attached.project_id.as_deref(), Some("p1"));
    assert_eq!(
        attached.color.as_deref(),
        Some("#ff0000"),
        "the richer cached list is kept"
    );
    assert!(
        app.store.list("s1").expect("reads").is_some(),
        "the column is there"
    );
    let projects = call(&app, json!({ "kind": "projects" })).await;
    assert_eq!(projects["value"][0]["id"], "p1");
}

/// Deleting a board detaches its lists and keeps them.
#[tokio::test]
async fn deleting_a_board_detaches_its_lists() {
    let transport = StubTransport::new().push_json(
        "/api/v1/projects/p1",
        200,
        json!({ "success": true, "detachedListIds": ["l1"] }),
    );
    let app = app_with(transport);
    app.store
        .upsert_projects(&[
            serde_json::from_value(json!({ "id": "p1", "name": "Work" })).expect("a project"),
        ])
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "projectId": "p1" }))
                .expect("a list"),
        )
        .expect("stores");

    let deleted = call(&app, json!({ "kind": "deleteProject", "projectId": "p1" })).await;
    assert_eq!(deleted["value"]["success"], true, "{deleted}");
    assert!(app.store.projects().expect("reads").is_empty());
    let list = app.store.list("l1").expect("reads").expect("kept");
    assert!(list.project_id.is_none());
}

/// A board deleted on the web goes on the next refresh; the set is whole.
#[tokio::test]
async fn a_board_deleted_elsewhere_goes_on_refresh() {
    let transport = StubTransport::new().push_json(
        "/api/v1/projects",
        200,
        json!({ "projects": [{ "id": "p2", "name": "Kept" }] }),
    );
    let app = app_with(transport);
    app.store
        .upsert_projects(&[
            serde_json::from_value(json!({ "id": "p1", "name": "Gone" })).expect("a project"),
            serde_json::from_value(json!({ "id": "p2", "name": "Kept" })).expect("a project"),
        ])
        .expect("stores");
    app.context
        .boards()
        .refresh_projects()
        .await
        .expect("refreshes");
    let ids: Vec<String> = app
        .store
        .projects()
        .expect("reads")
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec!["p2"]);
}

/// The remaining five `supports` keys gate TASK-DETAIL controls, and the answer comes from the
/// core for the same reason `supports.membership` does (task 897e25c5): a shell that worked out
/// from `backend` whether a priority picker would 403 would be the fourth implementation of the
/// rule. `taskDetail` had no capability fields at all before this, so there was nowhere for a
/// shell to look even if it wanted to ask.
///
/// Two cases the membership work flagged and this pins. A `supports` block that OMITS a key is a
/// classic list saying nothing, not a board saying no — the whole point of `unwrap_or_default` on
/// the way in. And a task on no list this cache knows is classic too: a membership the reader
/// cannot see must never take away a control that works.
#[tokio::test]
async fn a_board_that_maps_no_priority_field_offers_no_priority_on_task_detail() {
    let app = app_with(StubTransport::new());
    let store_list = |value: serde_json::Value| {
        app.store
            .upsert_list(&serde_json::from_value(value).expect("a list"))
            .expect("stores");
    };
    let store_task = |value: serde_json::Value| {
        app.store
            .upsert_task(&serde_json::from_value(value).expect("a task"))
            .expect("stores");
    };
    // A board with neither field mapped, no Astrid comments and an item that comes off the
    // project rather than being deleted.
    store_list(json!({
        "id": "gh", "name": "Roadmap", "ownerId": "me", "backend": "github_project",
        "supports": {
            "repeatMode": "spawn", "membership": false, "privateTasks": false,
            "priority": false, "dueDate": false, "comments": false,
            "multipleAssignees": true, "deleteTask": "remove_from_list",
        },
    }));
    // A `supports` block that says nothing about any of the five.
    store_list(json!({ "id": "quiet", "name": "Quiet", "ownerId": "me",
                       "supports": { "membership": false } }));
    // A tag is not what decides: a task in a milestone and on a classic list is the list's.
    store_list(json!({ "id": "tag", "name": "v2", "listType": "milestone" }));
    store_task(json!({ "id": "t1", "title": "Mapped nothing", "listIds": ["gh"] }));
    store_task(json!({ "id": "t2", "title": "Quiet list", "listIds": ["quiet"] }));
    store_task(json!({ "id": "t3", "title": "Tagged", "listIds": ["tag", "quiet"] }));
    store_task(json!({ "id": "t4", "title": "Nowhere this cache knows", "listIds": ["gone"] }));

    let board = call(&app, json!({ "kind": "taskDetail", "taskId": "t1" })).await;
    assert_eq!(board["ok"], true, "{board}");
    let supports = &board["value"]["supports"];
    assert_eq!(supports["priority"], false, "no Priority field is mapped");
    assert_eq!(supports["dueDate"], false, "no date field is mapped");
    assert_eq!(supports["comments"], false, "comments live on the issue");
    assert_eq!(supports["privateTasks"], false);
    assert_eq!(supports["multipleAssignees"], true);
    assert_eq!(
        supports["deleteTask"], "remove_from_list",
        "the item comes off the project and the issue stays"
    );

    for (task_id, why) in [
        (
            "t2",
            "a block that omits a key is a classic list saying nothing",
        ),
        (
            "t3",
            "a milestone has no controls to offer; the list decides",
        ),
        ("t4", "a list this cache cannot see takes nothing away"),
    ] {
        let answered = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
        let supports = &answered["value"]["supports"];
        assert_eq!(supports["priority"], true, "{why}");
        assert_eq!(supports["dueDate"], true, "{why}");
        assert_eq!(supports["comments"], true, "{why}");
        assert_eq!(supports["privateTasks"], true, "{why}");
        assert_eq!(supports["multipleAssignees"], false, "{why}");
        assert_eq!(supports["deleteTask"], "delete", "{why}");
    }

    // And the flyout's DEFAULTS section is gated on the same two keys, because it offers the very
    // controls they gate: a default priority for a field the board does not have is a default
    // nothing can apply.
    let settings = call(&app, json!({ "kind": "listMembers", "listId": "gh" })).await;
    assert_eq!(settings["value"]["supports"]["priority"], false);
    assert_eq!(settings["value"]["supports"]["dueDate"], false);
    let quiet = call(&app, json!({ "kind": "listMembers", "listId": "quiet" })).await;
    assert_eq!(
        quiet["value"]["supports"]["priority"], true,
        "a classic list keeps every default it has always had"
    );
}

/// A board answers with rows, so a card draws like a row, and the columns come back in the
/// order the board shows them.
#[tokio::test]
async fn a_board_answers_with_its_columns_and_their_cards() {
    let app = app_with(StubTransport::new());
    let project = serde_json::json!({ "id": "p1", "name": "Ship it" });
    app.store
        .upsert_projects(&[serde_json::from_value(project).expect("a project")])
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(serde_json::json!({
                "id": "l1", "name": "Work", "projectId": "p1"
            }))
            .expect("a list"),
        )
        .expect("stores");
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Write it down", "listIds": ["l1"] }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    let columns = board["value"]["columns"].as_array().expect("columns");
    assert_eq!(columns[0]["id"], "__virtual_inbox__");
    assert_eq!(columns.last().expect("done")["id"], "__virtual_done__");
    // A new card with no role is in the Inbox, and it arrives as a row.
    assert_eq!(columns[0]["total"], 1);
    assert_eq!(columns[0]["cards"][0]["title"], "Write it down");
    assert!(columns[0]["cards"][0]["leading"].is_object());

    // Moving it carries the role, and the card lands in that column.
    call(
        &app,
        json!({
            "kind": "moveTaskToColumn",
            "taskId": id,
            "columnId": "doing",
            "listId": "l1",
        }),
    )
    .await;
    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    let columns = board["value"]["columns"].as_array().expect("columns");
    let doing = columns
        .iter()
        .find(|column| column["id"] == "doing")
        .expect("a doing column");
    assert_eq!(doing["total"], 1);
    assert_eq!(columns[0]["total"], 0);
}

/// A card typed into a column is BORN in that column — role and all, in one request (task
/// 95c7a68f).
///
/// The Mac's AITD-328: a first version dropped the status role on the way through, so a card
/// typed into Doing was created with no role, resolved to Inbox, and appeared there. Hence the
/// assertion on the role itself and not only on which column the board draws it in.
#[tokio::test]
async fn a_card_typed_into_a_column_is_born_in_it_task_95c7a68f() {
    let app = board_app(json!({ "id": "l1", "name": "Work", "projectId": "p1" })).await;

    let made = call(
        &app,
        json!({
            "kind": "addBoardCard",
            "listId": "l1",
            "columnId": "doing",
            "title": "  Write it down  ",
        }),
    )
    .await;

    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["value"]["title"], "Write it down", "trimmed");
    assert_eq!(made["value"]["statusRole"], "doing");
    assert_eq!(made["value"]["listIds"], json!(["l1"]));
    assert_eq!(made["value"]["completed"], false);

    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    let columns = board["value"]["columns"].as_array().expect("columns");
    let doing = columns
        .iter()
        .find(|column| column["id"] == "doing")
        .expect("a doing column");
    assert_eq!(doing["total"], 1, "in the column it was typed into");
    assert_eq!(doing["cards"][0]["title"], "Write it down");
    assert_eq!(columns[0]["total"], 0, "and not in the Inbox");
}

/// Inbox has no status, so a card typed there carries no role — and a column that is not on this
/// board is refused rather than guessed at (task 95c7a68f).
#[tokio::test]
async fn a_card_typed_into_the_inbox_carries_no_role_task_95c7a68f() {
    let app = board_app(json!({ "id": "l1", "name": "Work", "projectId": "p1" })).await;

    let made = call(
        &app,
        json!({
            "kind": "addBoardCard",
            "listId": "l1",
            "columnId": "__virtual_inbox__",
            "title": "Write it down",
        }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert!(
        made["value"]["statusRole"].is_null(),
        "Inbox is the absence of a status, not a status called Inbox"
    );

    let refused = call(
        &app,
        json!({
            "kind": "addBoardCard",
            "listId": "l1",
            "columnId": "not-a-column",
            "title": "Write it down",
        }),
    )
    .await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["kind"], "badRequest");
}

/// A card typed into Done is completed through the COMPLETION SERVICE, not by writing the flag
/// (task 95c7a68f, ASTRID.md §0 rule 2).
///
/// The list's own default repeat is what makes this provable: `update(completed: true)` would
/// leave a finished daily task, while `TaskService::complete` rolls it forward to tomorrow and
/// leaves it open. The same test therefore shows a card typed into a column takes its list's
/// defaults,
/// because it goes through the one create path the quick-add box uses.
#[tokio::test]
async fn a_card_typed_into_done_is_completed_through_the_service_task_95c7a68f() {
    let app = board_app(json!({
        "id": "l1",
        "name": "Work",
        "projectId": "p1",
        "defaultRepeating": "daily",
        "defaultDueDate": "today",
        "defaultPriority": 3,
    }))
    .await;

    let made = call(
        &app,
        json!({
            "kind": "addBoardCard",
            "listId": "l1",
            "columnId": "__virtual_done__",
            "title": "Water the plants",
        }),
    )
    .await;

    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(
        made["value"]["priority"], 3,
        "the list's default reached it"
    );
    assert_eq!(made["value"]["repeating"], "daily");
    assert_eq!(
        made["value"]["completed"], false,
        "a repeating card completed in Done rolls forward rather than finishing"
    );
    let due = made["value"]["dueDateTime"]
        .as_str()
        .expect("a due date from the list's default");
    let today = crate::filters::local_day(app.clock.now(), app.clock.utc_offset());
    assert_eq!(
        due,
        crate::model::date::format(crate::model::date::all_day_instant(
            today + chrono::Duration::days(1)
        )),
        "rolled to tomorrow, which only the completion service does"
    );
}

/// An ordinary card typed into Done is simply done, and the board draws it there (task 95c7a68f).
#[tokio::test]
async fn an_ordinary_card_typed_into_done_lands_in_done_task_95c7a68f() {
    let app = board_app(json!({ "id": "l1", "name": "Work", "projectId": "p1" })).await;

    let made = call(
        &app,
        json!({
            "kind": "addBoardCard",
            "listId": "l1",
            "columnId": "__virtual_done__",
            "title": "Write it down",
        }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["value"]["completed"], true);

    let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
    let columns = board["value"]["columns"].as_array().expect("columns");
    let done = columns
        .iter()
        .find(|column| column["id"] == "__virtual_done__")
        .expect("a done column");
    assert_eq!(done["total"], 1);
    assert_eq!(done["cards"][0]["title"], "Write it down");
}

/// Dragging a repeating card to Done rolls it forward like every other completion. Rule 2 does
/// not stop applying because the gesture is a drag.
#[tokio::test]
async fn a_repeating_card_dragged_to_done_rolls_over() {
    let app = app_with(StubTransport::new());
    app.store
        .upsert_list(
            &serde_json::from_value(serde_json::json!({
                "id": "l1", "name": "Work", "projectId": "p1"
            }))
            .expect("a list"),
        )
        .expect("stores");
    let made = call(
        &app,
        json!({
            "kind": "createTask",
            "title": "Water plants",
            "listIds": ["l1"],
            "dueDateTime": "2026-09-20T09:00:00Z",
        }),
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

    call(
        &app,
        json!({
            "kind": "moveTaskToColumn",
            "taskId": id,
            "columnId": "__virtual_done__",
            "listId": "l1",
        }),
    )
    .await;

    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": id })).await;
    assert_eq!(
        detail["value"]["task"]["completed"], false,
        "a repeating card rolls forward instead of finishing"
    );
    // A daily repeat counted from the completion date, which is what a task carries unless it
    // says otherwise: finished on the 7th, so it comes back on the 8th — at the time of day it
    // was already due, rather than the moment it happened to be ticked off.
    assert_eq!(
        detail["value"]["task"]["dueDateTime"],
        "2026-09-08T09:00:00Z"
    );
}

/// A list with no project has no board. Not an error: the shell asks before it knows.
#[tokio::test]
async fn a_list_with_no_project_has_no_board() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let board = call(&app, json!({ "kind": "board", "listId": id })).await;
    assert!(board["value"]["projectId"].is_null());
    assert!(board["value"]["columns"]
        .as_array()
        .expect("columns")
        .is_empty());
}

#[tokio::test]
async fn a_column_that_is_not_on_this_board_is_refused() {
    let app = app_with(StubTransport::new());
    app.store
        .upsert_list(
            &serde_json::from_value(serde_json::json!({
                "id": "l1", "name": "Work", "projectId": "p1"
            }))
            .expect("a list"),
        )
        .expect("stores");
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Write it down", "listIds": ["l1"] }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let answered = call(
        &app,
        json!({
            "kind": "moveTaskToColumn",
            "taskId": id,
            "columnId": "nowhere",
            "listId": "l1",
        }),
    )
    .await;
    assert_eq!(answered["error"]["kind"], "badRequest");
}
