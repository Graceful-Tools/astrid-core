//! The row payloads — rows, counts, badges and the decks built from them.

use super::*;

/// The window a virtualised list asks for, with the total it needs to size its scrollbar.
#[tokio::test]
async fn rows_come_back_a_window_at_a_time() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    let list_id = call(&app, json!({ "kind": "lists" })).await["value"][0]["id"]
        .as_str()
        .expect("an id")
        .to_string();
    for index in 0..10 {
        call(
            &app,
            json!({ "kind": "createTask", "title": format!("Task {index}"), "listIds": [list_id] }),
        )
        .await;
    }

    let page = call(
        &app,
        json!({ "kind": "rowsForList", "listId": list_id, "offset": 2, "limit": 3 }),
    )
    .await;
    assert_eq!(
        page["value"]["total"], 10,
        "the total is what there is to scroll"
    );
    assert_eq!(page["value"]["offset"], 2);
    assert_eq!(
        page["value"]["rows"].as_array().expect("rows").len(),
        3,
        "and only the window crosses the boundary"
    );
}

/// A virtual list — "Today", "Not in a List", "I've Assigned" — has no membership. It is a
/// saved set of filters over everything, and sourcing it from membership the way a real list
/// is sourced gives an empty screen with nothing to explain it.
#[tokio::test]
async fn a_virtual_list_filters_everything_rather_than_its_own_membership() {
    let app = app_with(StubTransport::new());

    // A real list with one task in it, and a task in no list at all.
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    let list_id = call(&app, json!({ "kind": "lists" })).await["value"][0]["id"]
        .as_str()
        .expect("an id")
        .to_string();
    call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": [list_id] }),
    )
    .await;
    call(&app, json!({ "kind": "createTask", "title": "Loose end" })).await;

    // The "Not in a List" virtual list, exactly as the server stores it.
    let virtual_list: crate::model::TaskList = serde_json::from_value(json!({
        "id": "v-not-in-list",
        "name": "Not in a List",
        "isVirtual": true,
        "virtualListType": "not-in-list",
        "filterInLists": "not_in_list",
        "filterCompletion": "default",
        "sortBy": "auto"
    }))
    .expect("decodes");
    app.store.upsert_list(&virtual_list).expect("stores");

    let rows = call(
        &app,
        json!({ "kind": "rowsForList", "listId": "v-not-in-list" }),
    )
    .await;
    assert_eq!(rows["value"]["total"], 1);
    assert_eq!(rows["value"]["rows"][0]["title"], "Loose end");
}

/// And a virtual list with no filters shows everything, rather than everything in a list that
/// does not exist.
#[tokio::test]
async fn an_unfiltered_virtual_list_shows_the_whole_account() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createTask", "title": "one" })).await;
    call(&app, json!({ "kind": "createTask", "title": "two" })).await;

    let virtual_list: crate::model::TaskList = serde_json::from_value(json!({
        "id": "v-all", "name": "Everything", "isVirtual": true
    }))
    .expect("decodes");
    app.store.upsert_list(&virtual_list).expect("stores");

    let rows = call(&app, json!({ "kind": "rowsForList", "listId": "v-all" })).await;
    assert_eq!(rows["value"]["total"], 2);
}

/// Task da326190. iOS's profile draws a read-only section of the person's shared tasks beneath
/// the three numbers. The same `/users/:id/profile` answer that `profileStats` already fetches
/// carries them, so this is a projection and not a second round trip: the server's tasks go
/// through `TaskRow`, the way every other row surface here is projected.
///
/// The answer names who the profile belongs to and whether it is your own, and NOT a title.
/// iOS's two titles ("Your Tasks" / "Shared tasks with Dana") are user-facing strings, and those
/// live in each client's resources.
#[tokio::test]
async fn da326190_a_profile_answers_its_shared_tasks_as_rows() {
    let app = app_with(StubTransport::new().push_json(
        "users/u-them/profile",
        200,
        json!({
            "user": { "id": "u-them", "name": "Dana", "image": "https://astrid.cc/dana.png" },
            "stats": { "completed": 3, "inspired": 1, "supported": 2 },
            "sharedTasks": [
                { "id": "t1", "title": "Plant the bulbs", "creatorId": "u-them",
                  "completed": false, "priority": 1 },
                { "id": "t2", "title": "Water them", "creatorId": "u-them", "completed": true },
            ],
            "isOwnProfile": false,
        }),
    ));
    call(
        &app,
        json!({ "kind": "seedCache", "currentUser": { "id": "u-me", "name": "Ada" } }),
    )
    .await;

    let profile = call(&app, json!({ "kind": "profileTasks", "userId": "u-them" })).await;

    assert_eq!(profile["ok"], true, "{profile}");
    let rows = profile["value"]["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "{profile}");
    assert_eq!(rows[0]["title"], "Plant the bulbs");
    assert_eq!(rows[0]["completed"], false);
    assert_eq!(rows[1]["completed"], true);
    // The count iOS shows beside the heading is the section's own, not the account's statistics.
    assert_eq!(profile["value"]["count"], 2);
    assert_eq!(profile["value"]["isOwnProfile"], false);
    assert_eq!(profile["value"]["name"], "Dana");
    assert!(
        profile["value"]["title"].is_null(),
        "the core names no user-facing string: {profile}"
    );
}

/// AATD-69 (astrid-android). Another person's profile is drawn from this one answer: who they are
/// (photo, joined, AI agent), their three numbers and their shared tasks. One request, so the
/// numbers and the rows beneath them come from the same moment. No email: the server discloses it
/// only on your own profile, and the core does not go looking for it.
#[tokio::test]
async fn aatd69_a_profile_answers_who_they_are_and_their_numbers_too() {
    let app = app_with(StubTransport::new().push_json(
        "users/u-them/profile",
        200,
        json!({
            "user": { "id": "u-them", "name": "Dana", "image": "https://astrid.cc/dana.png",
                      "createdAt": "2025-03-04T10:00:00.000Z", "isAIAgent": false },
            "stats": { "completed": 3, "inspired": 1, "supported": 2 },
            "sharedTasks": [],
            "isOwnProfile": false,
        }),
    ));
    call(
        &app,
        json!({ "kind": "seedCache", "currentUser": { "id": "u-me", "name": "Ada" } }),
    )
    .await;

    let profile = call(&app, json!({ "kind": "profileTasks", "userId": "u-them" })).await;

    assert_eq!(profile["ok"], true, "{profile}");
    assert_eq!(profile["value"]["id"], "u-them");
    assert_eq!(profile["value"]["createdAt"], "2025-03-04T10:00:00.000Z");
    assert_eq!(profile["value"]["isAIAgent"], false);
    assert_eq!(
        profile["value"]["stats"],
        json!({ "completed": 3, "inspired": 1, "supported": 2 })
    );
    assert!(profile["value"]["email"].is_null(), "{profile}");
}

/// Task da326190. With no `userId` it is your own profile, asked for by the id sign-in recorded;
/// and signed out it is refused the way `profileStats` is, because an empty list would read as an
/// account that shares nothing rather than as nobody being signed in.
#[tokio::test]
async fn da326190_your_own_profile_needs_no_id_and_signed_out_is_refused() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "users/u-me/profile",
        200,
        json!({
            "user": { "id": "u-me", "name": "Ada" },
            "sharedTasks": [{ "id": "t1", "title": "Mine", "creatorId": "u-me" }],
            "isOwnProfile": true,
        }),
    ));

    let signed_out = call(&app, json!({ "kind": "profileTasks" })).await;
    assert_eq!(signed_out["ok"], false, "{signed_out}");
    assert!(
        transport.requests().is_empty(),
        "nothing is asked of the server with nobody signed in"
    );

    call(
        &app,
        json!({ "kind": "seedCache", "currentUser": { "id": "u-me", "name": "Ada" } }),
    )
    .await;
    let mine = call(&app, json!({ "kind": "profileTasks" })).await;

    assert_eq!(mine["ok"], true, "{mine}");
    assert_eq!(mine["value"]["isOwnProfile"], true);
    assert_eq!(mine["value"]["count"], 1);
    assert!(
        transport
            .requests()
            .iter()
            .any(|request| request.url.ends_with("/api/v1/users/u-me/profile")),
        "your own profile is asked for by the signed-in id"
    );
}

/// My Tasks is not in the list collection, so asking for its rows by a list id would be a
/// not-found. It is the view the app opens on.
#[tokio::test]
async fn my_tasks_rows_answer_without_a_list_row_behind_them() {
    let app = app_with(StubTransport::new());
    // My Tasks is what is assigned to the reader (D25), so somebody has to be signed in.
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "assigneeId": "me" }),
    )
    .await;
    assert_eq!(made["ok"], true);

    let rows = call(
        &app,
        json!({ "kind": "rowsForList", "listId": "virtual:my-tasks" }),
    )
    .await;

    assert_eq!(rows["ok"], true, "{rows}");
    assert_eq!(rows["value"]["total"], 1);
}

/// A task you just made, in a list that names no default assignee, is yours — so its row draws a
/// CHECKBOX and not the unassigned mark (task e2505d10, reported as "new tasks in windows don't
/// have a checkbox on list view").
///
/// The rule was already the server's: `resolveAssignee` returns `actorId` when the list's
/// `defaultAssigneeId` is null. The core just used to leave it to the server, so the row drawn the
/// instant somebody hits + was unassigned until a sync answered — and offline, forever. Asserted
/// through `rowsForList` rather than on the draft, because the leading control is the thing that
/// was actually wrong on screen.
#[tokio::test]
async fn a_task_i_just_made_has_a_checkbox_task_e2505d10() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work" })).expect("a list"),
        )
        .expect("stores");

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": ["l1"] }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(
        made["value"]["assigneeId"], "me",
        "the creator is resolved locally, not left to the server: {made}"
    );

    let rows = call(&app, json!({ "kind": "rowsForList", "listId": "l1" })).await;
    assert_eq!(
        rows["value"]["rows"][0]["leading"]["kind"], "checkbox",
        "{rows}"
    );
}

/// The same thing again from **My Tasks**, which is where it was actually reported (task a2320078:
/// *"when adding a task to windows task list the checkbox doesn't show up in the row immediately"*).
///
/// Task `e2505d10` resolved the creator locally, but only down the arm that finds a cached list to
/// read defaults from. My Tasks has no such row — `ListService::my_tasks` synthesises it and says so
/// ("Not in the list collection and never will be") — so a task added from the view the app OPENS
/// ON fell past the whole `list_defaults` pass and stayed unassigned until a sync answered.
///
/// The tell that this is a bug rather than a policy is the control above the box: `quickAddDefaults`
/// already has the pseudo-list arm and previews a checkbox here, so the preview and the create
/// disagreed — the one thing the shared-draft preview exists to make impossible.
#[tokio::test]
async fn a_task_made_from_my_tasks_has_a_checkbox_task_a2320078() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    // What the sidebar hands the shell for My Tasks, and therefore what `createTask` is told.
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": ["virtual:my-tasks"] }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(
        made["value"]["assigneeId"], "me",
        "a task added from My Tasks is mine, with no list row to say so and no network: {made}"
    );

    let shown = call(
        &app,
        json!({ "kind": "quickAddDefaults", "listId": "virtual:my-tasks" }),
    )
    .await;
    assert_eq!(
        shown["value"]["assigneeId"], made["value"]["assigneeId"],
        "the control above the box previewed the task that was made: {shown}"
    );

    let rows = call(
        &app,
        json!({ "kind": "rowsForList", "listId": "virtual:my-tasks" }),
    )
    .await;
    assert_eq!(
        rows["value"]["rows"][0]["leading"]["kind"], "checkbox",
        "the row drawn straight after the tap carries the checkbox, not the unassigned mark: {rows}"
    );
}

/// What the sidebar hands the shell for My Tasks is a view, not a destination, so the task it makes
/// belongs to no list — which is exactly what the web sends from the same box
/// (`components/quick-add.tsx` passes `listIds` only for a real list).
///
/// Found while fixing `a2320078` above and fixed separately. The task was filed into
/// `virtual:my-tasks`, and the server validates `listIds` by existence: the outbox create came back
/// `400 Invalid list IDs: virtual:my-tasks` and the task never left the device that made it.
#[tokio::test]
async fn a_task_made_from_my_tasks_is_not_filed_in_the_view_task_fda15483() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "listIds": ["virtual:my-tasks"] }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(
        made["value"]["listIds"],
        json!([]),
        "the view is not a list the task can be in: {made}"
    );

    // The row still draws under My Tasks, because that view is "assigned to me" and never a filing.
    let rows = call(
        &app,
        json!({ "kind": "rowsForList", "listId": "virtual:my-tasks" }),
    )
    .await;
    assert_eq!(
        rows["value"]["rows"][0]["title"], "Buy milk",
        "dropping the id must not lose the task from the view it was added in: {rows}"
    );
}

/// AATD-46: a featured (collaborative) public list shows its tasks from the server — someone else's read-only,
/// its control copying; mine completing — and none of them enter the cache.
#[tokio::test]
async fn aatd46_a_featured_list_shows_its_tasks_without_caching_them() {
    let transport = StubTransport::new().push_json(
        "/api/v1/tasks",
        200,
        json!({ "tasks": [
            { "id": "theirs", "title": "Plant tulips", "listIds": ["pub"], "creatorId": "them" },
            { "id": "mine", "title": "Water roses", "listIds": ["pub"], "creatorId": "me" }
        ] }),
    );
    let app = app_with(transport);
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    let rows = call(
        &app,
        json!({ "kind": "featuredRows", "listId": "pub",
                "list": { "id": "pub", "name": "Garden tips", "privacy": "PUBLIC", "ownerId": "them", "publicListType": "collaborative" } }),
    )
    .await;
    assert_eq!(rows["ok"], true, "{rows}");
    let action = |id: &str| {
        rows["value"]["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["id"] == id)
            .map(|row| row["action"].clone())
    };
    assert_eq!(action("theirs"), Some(json!("copy")), "{rows}");
    assert_eq!(action("mine"), Some(json!("complete")), "{rows}");
    let cached = call(&app, json!({ "kind": "task", "taskId": "theirs" })).await;
    assert_eq!(cached["ok"], false, "not cached: {cached}");
}

/// AATD-54: the triage deck is the reminded task, then my open tasks due today or overdue —
/// never someone else's, never a done one.
#[tokio::test]
async fn aatd54_the_triage_deck_is_what_is_due_for_me() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    call(
        &app,
        json!({ "kind": "seedCache", "tasks": [
            { "id": "late", "title": "Late", "assigneeId": "me", "dueDateTime": "2000-01-01T00:00:00Z", "isAllDay": true },
            { "id": "theirs", "title": "Theirs", "assigneeId": "bo", "dueDateTime": "2000-01-01T00:00:00Z", "isAllDay": true },
            { "id": "done", "title": "Done", "assigneeId": "me", "completed": true, "dueDateTime": "2000-01-01T00:00:00Z", "isAllDay": true },
            { "id": "later", "title": "Later", "assigneeId": "me", "dueDateTime": "2099-01-01T00:00:00Z", "isAllDay": true },
            { "id": "pinged", "title": "Pinged", "assigneeId": "me", "dueDateTime": "2099-06-01T00:00:00Z", "isAllDay": true }
        ] }),
    )
    .await;
    let deck = call(
        &app,
        json!({ "kind": "reminderTriageDeck", "remindedTaskId": "pinged" }),
    )
    .await;
    let ids: Vec<&str> = deck["value"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["pinged", "late"], "{deck}");
}

/// AATD-37: rowsForList says whether the reader may add here — yes in a list they belong to, no in
/// a copy-only public list they do not.
#[tokio::test]
async fn aatd37_rows_say_whether_the_reader_may_add() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "mine", "name": "Home", "ownerId": "me" }))
                .expect("a list"),
        )
        .expect("stores");
    let mine = call(&app, json!({ "kind": "rowsForList", "listId": "mine" })).await;
    assert_eq!(mine["value"]["canAddTasks"], true, "{mine}");
    let theirs = call(
        &app,
        json!({ "kind": "rowsForList", "listId": "pub",
                "list": { "id": "pub", "name": "Tips", "privacy": "PUBLIC", "ownerId": "them" } }),
    )
    .await;
    assert_eq!(theirs["value"]["canAddTasks"], false, "{theirs}");
}
