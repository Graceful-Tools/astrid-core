//! Sync, the outbox, the journal and the cache.

use super::*;

/// Offline is not an error the user needs to see: the write is journalled and will go.
#[tokio::test]
async fn a_write_made_offline_still_succeeds() {
    let app = app_with(StubTransport::new().fallback(Err(
        crate::api::TransportError::Unreachable("no network".into()),
    )));
    let created = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    assert_eq!(created["ok"], true);

    let stats = call(&app, json!({ "kind": "outboxStats" })).await;
    assert_eq!(stats["value"]["hasUnsentWork"], true);
}

#[tokio::test]
async fn a_sync_that_cannot_reach_the_server_reports_rather_than_fails() {
    let app = app_with(StubTransport::new().fallback(Err(
        crate::api::TransportError::Unreachable("no network".into()),
    )));
    let report = call(&app, json!({ "kind": "sync" })).await;
    assert_eq!(report["ok"], true);
    assert_eq!(report["value"]["fetched"], false);
}

/// A callback that arrives after signing out must not sign the user back in.
/// A refused write says why, where its count is shown — the Apple settings screen's readout.
#[tokio::test]
async fn the_outbox_stats_say_why_a_write_was_refused() {
    let app = app_with(StubTransport::new());
    let entry = crate::outbox::build(
        crate::outbox::kind::CREATE_TASK,
        json!({ "body": { "title": "x" } }),
        "temp_1",
        app.clock.now(),
    );
    crate::outbox::journal::enqueue(&app.store, &entry).expect("enqueues");
    crate::outbox::journal::mark_dead(&app.store, &entry.id, "403 not yours", app.clock.now())
        .expect("records");
    let stats = call(&app, json!({ "kind": "outboxStats" })).await;
    assert_eq!(stats["value"]["failed"], 1);
    assert_eq!(stats["value"]["deadLetters"][0]["kind"], "createTask");
    assert_eq!(stats["value"]["deadLetters"][0]["error"], "403 not yours");
}

/// Clearing the cache forgets rows but keeps what has not been sent.
#[tokio::test]
async fn clearing_the_cache_keeps_the_journal() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createTask", "title": "Unsent" })).await;
    call(&app, json!({ "kind": "clearCache" })).await;
    assert!(app.store.tasks().expect("reads").is_empty());
    assert_eq!(
        crate::outbox::journal::all(&app.store)
            .expect("reads")
            .len(),
        1
    );
}

/// An upgrade that runs again after a partial failure imports each write once.
#[tokio::test]
async fn a_write_is_imported_once() {
    let app = app_with(StubTransport::new());
    let import = json!({
        "kind": "importJournalEntry", "entryKind": "removeMember",
        "payload": { "listId": "l1", "userId": "u2" }, "clientRequestId": "cdmember-u2-delete"
    });
    call(&app, import.clone()).await;
    call(&app, import).await;
    assert_eq!(
        crate::outbox::journal::all(&app.store)
            .expect("reads")
            .len(),
        1
    );
}

/// The Apple apps' first launch after the move: their own cache goes in once, so an offline start
/// still shows everything — and never over a cache this crate already filled.
#[tokio::test]
async fn a_seed_fills_an_empty_cache_once() {
    let app = app_with(StubTransport::new());
    let seeded = call(
        &app,
        json!({ "kind": "seedCache",
                "lists": [{ "id": "l1", "name": "Home", "ownerId": "me", "privacy": "PRIVATE" }],
                "tasks": [{ "id": "t1", "title": "From Core Data", "listIds": ["l1"] }],
                "projects": [{ "id": "p1", "name": "Board" }],
                "channels": [{ "id": "c1", "listId": "l1" }],
                "messages": [{ "id": "m1", "channelId": "c1", "content": "offline history" }] }),
    )
    .await;
    assert_eq!(seeded["value"]["seeded"], true, "{seeded}");
    assert_eq!(
        call(&app, json!({ "kind": "projects" })).await["value"][0]["id"],
        "p1"
    );
    let chat = call(&app, json!({ "kind": "chatMessages", "channelId": "c1" })).await;
    assert_eq!(chat["value"][0]["content"], "offline history");
    let tasks = call(&app, json!({ "kind": "tasks" })).await;
    assert_eq!(tasks["value"][0]["title"], "From Core Data");
    let journal = call(&app, json!({ "kind": "outboxStats" })).await;
    assert_eq!(
        journal["value"]["hasUnsentWork"], false,
        "a seed journals nothing"
    );

    let again = call(
        &app,
        json!({ "kind": "seedCache", "tasks": [{ "id": "t2", "title": "Stale" }] }),
    )
    .await;
    assert_eq!(again["value"]["seeded"], false);
    let tasks = call(&app, json!({ "kind": "tasks" })).await;
    assert_eq!(tasks["value"].as_array().map(Vec::len), Some(1));
}

/// AATD-18. A UI-test build seeds who is signed in, exactly as sign-in records it, so My Tasks —
/// which is only what is assigned to the reader (D25) — has someone to be about.
#[tokio::test]
async fn aatd_18_a_seed_can_say_who_is_signed_in() {
    let app = app_with(StubTransport::new());
    call(
        &app,
        json!({ "kind": "seedCache",
                "currentUser": { "id": "u-me", "name": "Ada", "email": "ada@example.com" },
                "tasks": [{ "id": "t1", "title": "Mine", "assigneeId": "u-me", "creatorId": "u-me" },
                          { "id": "t2", "title": "Theirs", "assigneeId": "u-them", "creatorId": "u-them" }] }),
    )
    .await;
    assert_eq!(
        call(&app, json!({ "kind": "currentUser" })).await["value"]["id"],
        "u-me"
    );
    let mine = call(&app, json!({ "kind": "myTasksList" })).await;
    let rows = call(
        &app,
        json!({ "kind": "rowsForList", "listId": mine["value"]["id"] }),
    )
    .await;
    let titles: Vec<_> = rows["value"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["title"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(titles, ["Mine"], "{rows}");
}

/// A write another client's journal queued offline goes out from this one, as it was, under its
/// original idempotency key.
#[tokio::test]
async fn an_imported_write_is_journaled_verbatim_under_its_own_key() {
    let app = app_with(StubTransport::new());
    let answer = call(
        &app,
        json!({ "kind": "importJournalEntry", "entryKind": "updateTask",
                "payload": { "taskId": "temp_x", "body": { "completed": true } },
                "clientRequestId": "crid-1", "tempId": "temp_x" }),
    )
    .await;
    assert_eq!(answer["ok"], true, "{answer}");
    let journal = crate::outbox::journal::all(&app.store).expect("reads");
    let entry = journal.last().expect("journaled");
    assert_eq!(entry.kind, "updateTask");
    assert_eq!(entry.client_request_id, "crid-1");
    assert_eq!(entry.temp_id.as_deref(), Some("temp_x"));
    assert_eq!(entry.payload["body"]["completed"], true);

    let refused = call(
        &app,
        json!({ "kind": "importJournalEntry", "entryKind": "somethingElse",
                "payload": {}, "clientRequestId": "crid-2" }),
    )
    .await;
    assert_eq!(refused["ok"], false);
}

/// AATD-55: Settings' "Last Sync" — outboxStats says when the last pass was, or null before one.
#[tokio::test]
async fn aatd55_outbox_stats_say_when_the_last_sync_was() {
    let app = app_with(StubTransport::new());
    let before = call(&app, json!({ "kind": "outboxStats" })).await;
    assert!(before["value"]["lastSync"].is_null(), "{before}");
    app.store
        .set_metadata(crate::sync::LAST_SYNC_KEY, "2026-10-19T21:00:00Z")
        .expect("stores");
    let after = call(&app, json!({ "kind": "outboxStats" })).await;
    assert_eq!(
        after["value"]["lastSync"], "2026-10-19T21:00:00Z",
        "{after}"
    );
}
