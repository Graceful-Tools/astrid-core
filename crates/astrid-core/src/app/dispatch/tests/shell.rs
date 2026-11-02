//! The shell itself: keys, the editing session, the theme and the palette.

use super::*;

/// The shell asks what a key means rather than knowing. The guard — never while typing,
/// never inside a modal, selection-scoped actions need a selection — is the half of the
/// contract that is easiest to get wrong and impossible to see in a shortcut table.
#[tokio::test]
async fn a_key_resolves_to_an_action_only_when_it_is_allowed_to_fire() {
    let app = app_with(StubTransport::new());

    let ready = call(
        &app,
        json!({ "kind": "resolveShortcut", "key": "x", "hasSelection": true }),
    )
    .await;
    assert_eq!(ready["value"]["action"], "completeTask");

    // Selection-scoped, with nothing selected.
    let unselected = call(&app, json!({ "kind": "resolveShortcut", "key": "x" })).await;
    assert!(unselected["value"]["action"].is_null());

    // Typing.
    let typing = call(
        &app,
        json!({
            "kind": "resolveShortcut", "key": "x",
            "hasSelection": true, "isTextFieldFocused": true
        }),
    )
    .await;
    assert!(typing["value"]["action"].is_null());

    // A modal is open.
    let modal = call(
        &app,
        json!({
            "kind": "resolveShortcut", "key": "x",
            "hasSelection": true, "isModalPresented": true
        }),
    )
    .await;
    assert!(modal["value"]["action"].is_null());
}

/// A Windows `VirtualKey` reads as `ArrowDown`; web and Mac write the glyph. Both resolve, so
/// the shell does not have to translate before asking.
#[tokio::test]
async fn an_arrow_key_resolves_under_either_name() {
    let app = app_with(StubTransport::new());
    for key in ["ArrowDown", "\u{2193}", "j"] {
        let answer = call(&app, json!({ "kind": "resolveShortcut", "key": key })).await;
        assert_eq!(answer["value"]["action"], "selectNext", "for {key}");
    }
}

/// Resolving a key touches nothing. It has to be safe to ask while a key is being handled.
#[tokio::test]
async fn resolving_a_shortcut_reaches_no_network() {
    let transport = std::sync::Arc::new(StubTransport::new());
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

    call(&app, json!({ "kind": "resolveShortcut", "key": "n" })).await;
    assert!(transport.requests().is_empty());
}

#[tokio::test]
async fn the_whole_scheme_is_available_for_a_help_sheet() {
    let app = app_with(StubTransport::new());
    let answer = call(&app, json!({ "kind": "shortcuts" })).await;
    let shortcuts = answer["value"].as_array().expect("an array");
    assert!(shortcuts.len() > 10);
    assert!(shortcuts
        .iter()
        .any(|entry| entry["action"] == "newTask" && entry["requiresSelection"] == false));
}

/// Somebody who has never opened settings is looking at Ocean, and the picker offers it first.
#[tokio::test]
async fn the_theme_starts_as_the_brand_look() {
    let app = app_with(StubTransport::new());

    let answer = call(&app, json!({ "kind": "theme" })).await;

    assert_eq!(answer["value"]["theme"], "ocean");
    assert_eq!(
        answer["value"]["isDark"], false,
        "ocean is a light appearance"
    );
    assert_eq!(
        answer["value"]["choices"],
        json!(["ocean", "light", "dark", "auto"])
    );
}

/// It belongs to the machine, so it has to survive being asked for again.
#[tokio::test]
async fn a_chosen_theme_is_remembered() {
    let app = app_with(StubTransport::new());

    let set = call(&app, json!({ "kind": "setTheme", "theme": "dark" })).await;
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["value"]["isDark"], true);

    let held = call(&app, json!({ "kind": "theme" })).await;
    assert_eq!(held["value"]["theme"], "dark");
}

/// Auto has no appearance of its own — the system decides, and the shell has to be told that
/// rather than being handed a guess.
#[tokio::test]
async fn auto_leaves_the_appearance_to_the_system() {
    let app = app_with(StubTransport::new());

    call(&app, json!({ "kind": "setTheme", "theme": "auto" })).await;
    let held = call(&app, json!({ "kind": "theme" })).await;

    assert_eq!(held["value"]["theme"], "auto");
    assert!(held["value"]["isDark"].is_null());
}

/// Once. A tour that came back every launch would be the first thing anybody turned off.
#[tokio::test]
async fn the_tour_is_shown_once() {
    let app = app_with(StubTransport::new());
    assert_eq!(
        call(&app, json!({ "kind": "hasSeenTour" })).await["value"]["seen"],
        false
    );

    call(&app, json!({ "kind": "tourSeen" })).await;
    assert_eq!(
        call(&app, json!({ "kind": "hasSeenTour" })).await["value"]["seen"],
        true
    );
}

/// The palette answers from the cache, ranked, with commands first — so the keyboard action
/// for "new task" is reachable from the box that exists to reach things.
#[tokio::test]
async fn the_palette_finds_commands_lists_and_tasks() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createList", "name": "Home" })).await;
    call(
        &app,
        json!({ "kind": "createTask", "title": "Buy oat milk" }),
    )
    .await;

    let found = call(&app, json!({ "kind": "palette", "query": "milk" })).await;
    let rows = found["value"]["rows"].as_array().expect("rows");
    assert!(rows.iter().any(|row| row["title"] == "Buy oat milk"));

    // Fuzzy, and in order: "hm" finds "Home" by its letters.
    let fuzzy = call(&app, json!({ "kind": "palette", "query": "hm" })).await;
    assert!(fuzzy["value"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .any(|row| row["title"] == "Home"));
}

/// Task `56bf4598`: typing a task's id into the palette finds that task. It used to match titles
/// only, so an id — which shares no letters with the title — found nothing, while the search box
/// right next to it resolved the same text.
#[tokio::test]
async fn task_56bf4598_the_palette_finds_a_task_by_its_identifier() {
    let app = app_with(StubTransport::new());
    let mut numbered = crate::model::Task::new("t1", "Reply routing");
    numbered.identifier = Some("AWTD-1405".into());
    app.store.upsert_task(&numbered).expect("stores");

    let found = call(&app, json!({ "kind": "palette", "query": "awtd-1405" })).await;
    assert_eq!(found["ok"], true, "{found}");
    let tasks: Vec<&serde_json::Value> = found["value"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|row| row["kind"] == "task")
        .collect();
    assert_eq!(tasks.len(), 1, "{found}");
    assert_eq!(tasks[0]["id"], "t1");
}

/// The editing session lives in the core and is stepped by command (task e71ed760): opening a
/// second editor names the first as the one to commit, a stale end across the boundary commits
/// nothing, cancel is the only transition that reverts, and commit-all empties the session.
#[tokio::test]
async fn the_editing_session_is_stepped_by_command_task_e71ed760() {
    let app = app_with(StubTransport::new());

    let opened = call(&app, json!({ "kind": "beginEditing", "editor": "title" })).await;
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(opened["value"]["active"], "title");
    assert_eq!(opened["value"]["commit"], serde_json::Value::Null);

    let switched = call(
        &app,
        json!({ "kind": "beginEditing", "editor": "description" }),
    )
    .await;
    assert_eq!(switched["value"]["active"], "description");
    assert_eq!(
        switched["value"]["commit"], "title",
        "the first editor is committed"
    );

    let stale = call(&app, json!({ "kind": "endEditing", "editor": "title" })).await;
    assert_eq!(
        stale["value"]["commit"],
        serde_json::Value::Null,
        "a blur after the hand-off"
    );
    assert_eq!(
        stale["value"]["active"], "description",
        "the hand-off stands"
    );

    let cancelled = call(
        &app,
        json!({ "kind": "cancelEditing", "editor": "description" }),
    )
    .await;
    assert_eq!(cancelled["value"]["cancel"], "description");
    assert_eq!(cancelled["value"]["commit"], serde_json::Value::Null);
    assert_eq!(cancelled["value"]["active"], serde_json::Value::Null);

    call(&app, json!({ "kind": "beginEditing", "editor": "lists" })).await;
    let closed = call(&app, json!({ "kind": "commitAllEditing" })).await;
    assert_eq!(closed["value"]["commit"], "lists");
    assert_eq!(closed["value"]["active"], serde_json::Value::Null);
    let idle = call(&app, json!({ "kind": "commitAllEditing" })).await;
    assert_eq!(
        idle["value"]["commit"],
        serde_json::Value::Null,
        "nothing open, nothing to commit"
    );
}
