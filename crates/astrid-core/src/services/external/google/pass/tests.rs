//! `ExternalSyncService`'s Google tests.
//!
//! These were `mod tests` inside `google.rs` until task bb4f69ca gave them their own file.
//! `super` still means `external::google` at this depth, so no test body needed touching — the
//! move is the whole change.

use super::*;

use crate::api::{ApiClient, StubTransport};
use crate::model::date;
use crate::platform::{FixedClock, MemorySecureStore};
use crate::services::Context;
use crate::store::Store;
use std::sync::Arc;

struct Fixture {
    service: ExternalSyncService,
    store: Arc<Store>,
    transport: Arc<StubTransport>,
}

/// A pass over one link, with the four requests it makes scripted in the order it makes them:
/// the deletion, the pull, and the task-link fetch each of pull and push does.
fn fixture(delete_status: u16, pulled: serde_json::Value) -> Fixture {
    let transport = Arc::new(
        StubTransport::new()
            // The deletion goes first, so its answer is queued first.
            .push_json("google/tasks", delete_status, json!({}))
            .push_json("google/tasks", 200, json!({ "items": pulled }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] })),
    );
    let store = Arc::new(Store::in_memory().expect("opens"));
    let context = Context::new(
        Arc::new(ApiClient::new(
            "https://astrid.cc",
            transport.clone(),
            Arc::new(MemorySecureStore::new()),
        )),
        store.clone(),
        Arc::new(FixedClock::at(
            date::parse("2026-09-07T12:00:00Z").expect("an instant"),
        )),
    );
    Fixture {
        service: context.external(),
        store,
        transport,
    }
}

/// The same, with the caller scripting the whole conversation.
fn fixture_with(transport: StubTransport) -> Fixture {
    let transport = Arc::new(transport);
    let store = Arc::new(Store::in_memory().expect("opens"));
    let context = Context::new(
        Arc::new(ApiClient::new(
            "https://astrid.cc",
            transport.clone(),
            Arc::new(MemorySecureStore::new()),
        )),
        store.clone(),
        Arc::new(FixedClock::at(
            date::parse("2026-09-07T12:00:00Z").expect("an instant"),
        )),
    );
    Fixture {
        service: context.external(),
        store,
        transport,
    }
}

/// An account in one of the all-lists modes, with one remote list and no links.
fn auto_link_transport(mode: &str) -> StubTransport {
    StubTransport::new()
        .push_json(
            "/api/v1/integrations",
            200,
            json!({
                "integrations": [{
                    "provider": "GOOGLE_TASKS",
                    "metadata": { "googleSyncMode": mode },
                }],
            }),
        )
        .push_json(
            "google/tasklists",
            200,
            json!({
                "tasklists": [{ "id": "c1", "name": "Groceries" }],
                "defaultId": "default-list",
            }),
        )
        .push_json("google/links", 200, json!({ "links": [] }))
        .fallback(Ok(crate::api::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        }))
}

fn a_list(id: &str, name: &str) -> crate::model::TaskList {
    crate::model::TaskList::new(id, name)
}

fn link() -> ExternalLink {
    ExternalLink {
        id: "link-1".into(),
        astrid_list_id: "l1".into(),
        remote_container_id: "tasklist-1".into(),
        remote_container_name: None,
        cursor: None,
    }
}

/// A deletion made on the web reaches this device as metadata on the integration. Without
/// this, the next pull imports it again and somebody's deleted task is back.
#[tokio::test]
async fn the_servers_tombstones_arrive_with_the_status() {
    let fixture = fixture(200, json!([]));
    let answer = json!({
        "integrations": [{
            "provider": "GOOGLE_TASKS",
            "metadata": { "tombstonedRemoteIds": "r1, r2" },
        }],
    });
    fixture
        .service
        .merge_server_tombstones(&answer)
        .expect("merges");

    let held = ledger::tombstoned(&fixture.store, PROVIDER_KEY);
    assert!(held.contains(&"r1".to_string()));
    assert!(held.contains(&"r2".to_string()));
}

// ── The link that stops a twin being made twice ──────────────────────────────────────────

/// The bug this pins: a push that does not write the link down leaves the next pass with no
/// remote id, so it creates a *second* Google task — and one more every five minutes after.
#[tokio::test]
async fn a_pushed_task_is_linked_so_the_next_pass_patches_it_rather_than_making_another() {
    let fixture = fixture_with(
        StubTransport::new()
            // The pull: nothing to bring in.
            .push_json("google/tasks", 200, json!({ "items": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            // The push: the complete listing that proves no twin exists (AWTD2-56), the
            // create, and the link that follows it.
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json(
                "google/tasks",
                200,
                json!({ "items": [], "cursor": null, "truncated": false }),
            )
            .push_json(
                "google/tasks",
                200,
                json!({ "remoteId": "tasklist-1:r9", "remoteUpdatedAt": "2026-09-07T12:00:00Z" }),
            )
            .push_json("google/task-links", 200, json!({ "link": {} }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );
    let mut task = crate::model::Task::new("cm3real", "Buy milk");
    task.list_ids = Some(vec!["l1".into()]);
    fixture.store.upsert_task(&task).expect("writes");

    fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    let linked = fixture
        .transport
        .requests()
        .into_iter()
        .find(|request| {
            request.method.as_str() == "PUT" && request.url.contains("google/task-links")
        })
        .expect("the link was written down");
    let body: serde_json::Value =
        serde_json::from_slice(&linked.body.unwrap_or_default()).expect("a body");
    assert_eq!(body["astridTaskId"], "cm3real");
    assert_eq!(body["remoteId"], "tasklist-1:r9");
    assert_eq!(body["remoteContainerId"], "tasklist-1");
}

/// The link row is a foreign key onto the task, so the server rejects an id it has never seen.
/// Sending one would be a guaranteed 400 on every pass until the Outbox caught up.
#[tokio::test]
async fn a_task_that_has_not_reached_the_server_is_not_linked_yet() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json("google/tasks", 200, json!({ "items": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );
    let mut task = crate::model::Task::new("temp_abc", "Buy milk");
    task.list_ids = Some(vec!["l1".into()]);
    fixture.store.upsert_task(&task).expect("writes");

    fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    assert!(
        !fixture
            .transport
            .requests()
            .iter()
            .any(|request| request.method.as_str() == "PUT"),
        "nothing was linked, and nothing was pushed either"
    );
}

/// A pulled task is an Astrid task now. One written only to this machine's cache would be
/// invisible on the web, absent on the phone, and gone at the next sign-out.
#[tokio::test]
async fn a_pulled_task_is_written_through_the_journal_so_it_reaches_the_server() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json(
                "google/tasks",
                200,
                json!({ "items": [{ "remoteId": "tasklist-1:r1", "title": "Buy milk" }] }),
            )
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );

    let report = fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    assert_eq!(report.applied, 1);
    let queued = crate::outbox::journal::all(&fixture.store).expect("reads");
    assert!(
        queued
            .iter()
            .any(|entry| entry.kind == crate::outbox::kind::CREATE_TASK),
        "the create is on its way to astrid-web, not only in the cache"
    );
}

/// Two passes before the Outbox has been through must not pull the same item in twice. The
/// server does not know the link yet, so only this device's own note stops the duplicate.
#[tokio::test]
async fn a_second_pass_before_the_task_reaches_the_server_does_not_pull_it_in_again() {
    let items = json!({ "items": [{ "remoteId": "tasklist-1:r1", "title": "Buy milk" }] });
    let fixture = fixture_with(
        StubTransport::new()
            .push_json("google/tasks", 200, items.clone())
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json("google/tasks", 200, items)
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );

    fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("one");
    fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("two");

    let held = fixture.store.tasks().expect("reads");
    assert_eq!(
        held.iter().filter(|task| task.title == "Buy milk").count(),
        1,
        "one task, not one per pass"
    );
}

// ── My Tasks ↔ the default remote list ───────────────────────────────────────────────────

/// The account has to be known before My Tasks means anything: it is "assigned to me", and
/// without a "me" there is nothing to mirror.
fn signed_in(fixture: &Fixture) {
    let mut user = crate::model::User::new("u1");
    user.name = Some("Ada".into());
    fixture
        .store
        .set_metadata(
            "account.current-user",
            &serde_json::to_string(&user).expect("encodes"),
        )
        .expect("writes");
}

/// One conversation, in the order the pass has it: the deletion pass, the full listing, and
/// the task links each half asks for.
fn my_tasks_transport(mode: &str, items: serde_json::Value) -> StubTransport {
    StubTransport::new()
        .push_json(
            "/api/v1/integrations",
            200,
            json!({
                "integrations": [{
                    "provider": "GOOGLE_TASKS",
                    "metadata": { "googleSyncMode": mode },
                }],
            }),
        )
        .push_json("google/tasks", 200, json!({ "items": items }))
        .push_json("google/task-links", 200, json!({ "links": [] }))
        .fallback(Ok(crate::api::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        }))
}

/// A task Google filed nowhere becomes a task Astrid filed nowhere, assigned to you. Putting
/// it in a list would be inventing a list nobody made.
#[tokio::test]
async fn a_task_from_the_default_remote_list_becomes_an_unlisted_task_of_yours() {
    let fixture = fixture_with(my_tasks_transport(
        "all_google_to_astrid",
        json!([{ "remoteId": "default-list:r1", "title": "Ring the dentist" }]),
    ));
    signed_in(&fixture);

    let report = fixture
        .service
        .sync_my_tasks("default-list")
        .await
        .expect("a pass");

    assert_eq!(report.applied, 1);
    let made = fixture
        .store
        .tasks()
        .expect("reads")
        .into_iter()
        .find(|task| task.title == "Ring the dentist")
        .expect("the task");
    assert_eq!(made.assignee_id.as_deref(), Some("u1"));
    assert!(
        made.list_ids.unwrap_or_default().is_empty(),
        "unlisted, which is what My Tasks means"
    );
}

/// A mode that only mirrors outward does not pull, here as everywhere else.
#[tokio::test]
async fn my_tasks_does_not_pull_in_a_mode_that_only_mirrors_outward() {
    let fixture = fixture_with(my_tasks_transport(
        "all_astrid_to_google",
        json!([{ "remoteId": "default-list:r1", "title": "Ring the dentist" }]),
    ));
    signed_in(&fixture);

    let report = fixture
        .service
        .sync_my_tasks("default-list")
        .await
        .expect("a pass");

    assert_eq!(report.applied, 0);
    assert!(fixture.store.tasks().expect("reads").is_empty());
}

/// A task that gained a list, or lost the assignment that put it in My Tasks, still has a twin
/// in the default list — a second home for something that already has one.
#[tokio::test]
async fn a_task_that_has_left_my_tasks_has_its_twin_closed_out() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json(
                "/api/v1/integrations",
                200,
                json!({
                    "integrations": [{
                        "provider": "GOOGLE_TASKS",
                        "metadata": { "googleSyncMode": "all_bidirectional" },
                    }],
                }),
            )
            .push_json("google/tasks", 200, json!({ "items": [] }))
            .push_json(
                "google/task-links",
                200,
                json!({
                    "links": [{ "remoteId": "default-list:r1", "astridTaskId": "cm3real" }],
                }),
            )
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );
    signed_in(&fixture);
    // It has a list now, so it is not My Tasks any more.
    let mut task = crate::model::Task::new("cm3real", "Ring the dentist");
    task.assignee_id = Some("u1".into());
    task.list_ids = Some(vec!["l1".into()]);
    fixture.store.upsert_task(&task).expect("writes");

    fixture
        .service
        .sync_my_tasks("default-list")
        .await
        .expect("a pass");

    assert!(
        fixture.transport.requests().iter().any(|request| {
            request.method.as_str() == "DELETE" && request.url.contains("remoteId=default-list")
        }),
        "the twin in the default list was closed out"
    );
    assert!(
        ledger::tombstoned(&fixture.store, PROVIDER_KEY).contains(&"default-list:r1".to_string()),
        "and never re-imported"
    );
    assert!(
        fixture.store.task("cm3real").expect("reads").is_some(),
        "the task itself is untouched — it lives in its list now"
    );
}

/// Signed out there is no "me", so there is nothing this could mean.
#[tokio::test]
async fn my_tasks_does_nothing_when_nobody_is_signed_in() {
    let fixture = fixture_with(my_tasks_transport("all_bidirectional", json!([])));

    let report = fixture
        .service
        .sync_my_tasks("default-list")
        .await
        .expect("a pass");

    assert_eq!(report, PassReport::default());
}

// ── Auto-linking ─────────────────────────────────────────────────────────────────────────

/// The whole point of the adoption rules: somebody with "Groceries" on both sides ends up with
/// one list, not two called the same thing.
#[tokio::test]
async fn a_remote_list_adopts_the_local_one_of_the_same_name() {
    let fixture = fixture_with(auto_link_transport("all_google_to_astrid"));
    fixture
        .store
        .upsert_list(&a_list("cm3real", "Groceries"))
        .expect("writes");

    let report = fixture.service.auto_link_google().await.expect("links");

    assert_eq!(report.linked, 1);
    assert_eq!(report.lists_created, 0, "nothing was duplicated");
    let linked = fixture
        .transport
        .requests()
        .into_iter()
        .find(|request| request.url.contains("google/links") && request.method.as_str() == "POST")
        .expect("a link was made");
    let body: serde_json::Value =
        serde_json::from_slice(&linked.body.unwrap_or_default()).expect("a body");
    assert_eq!(body["astridListId"], "cm3real");
    assert_eq!(body["remoteContainerId"], "c1");
}

// ── "New List": create on Google and bind, in one gesture ────────────────────────────────

/// Task 6db679c3. Two writes, one gesture: the list is made on Google and bound here, and the
/// answer says which container it ended up on so the screen can draw the row it just made.
#[tokio::test]
async fn new_list_makes_the_google_list_and_binds_it() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json("google/tasklists", 200, json!({ "tasklists": [] }))
            .push_json("google/links", 200, json!({ "links": [] }))
            .push_json(
                "google/tasklists",
                200,
                json!({ "tasklist": { "id": "c9", "name": "Garden" } }),
            )
            .push_json(
                "google/links",
                200,
                json!({ "link": { "id": "link-9", "remoteContainerId": "c9" } }),
            ),
    );
    fixture
        .store
        .upsert_list(&a_list("cm3real", "Garden"))
        .expect("writes");

    let answer = fixture
        .service
        .create_and_link_google_list("cm3real", "Garden")
        .await
        .expect("links");

    assert_eq!(answer["containerId"], "c9");
    assert_eq!(answer["created"], true);
    assert_eq!(answer["link"]["id"], "link-9");
    let created = fixture
        .transport
        .requests()
        .into_iter()
        .find(|request| {
            request.url.contains("google/tasklists") && request.method.as_str() == "POST"
        })
        .expect("a tasklist was made");
    let body: serde_json::Value =
        serde_json::from_slice(&created.body.unwrap_or_default()).expect("a body");
    assert_eq!(body["title"], "Garden");
}

/// Task 6db679c3. The partial failure this command has to survive: an earlier attempt made the
/// Google list and then could not bind it. The second attempt adopts that orphan by name — the
/// rule [`auto_link::astrid_to_google`] already applies — rather than leaving a new list of the
/// same name on somebody's Google account every time they press the button again.
#[tokio::test]
async fn new_list_adopts_an_unlinked_google_list_of_the_same_name() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json(
                "google/tasklists",
                200,
                json!({ "tasklists": [{ "id": "c-orphan", "name": "Garden" }] }),
            )
            .push_json("google/links", 200, json!({ "links": [] }))
            .push_json(
                "google/links",
                200,
                json!({ "link": { "id": "link-9", "remoteContainerId": "c-orphan" } }),
            ),
    );
    fixture
        .store
        .upsert_list(&a_list("cm3real", "Garden"))
        .expect("writes");

    let answer = fixture
        .service
        .create_and_link_google_list("cm3real", "Garden")
        .await
        .expect("links");

    assert_eq!(answer["containerId"], "c-orphan");
    assert_eq!(answer["created"], false);
    assert!(
        !fixture.transport.requests().into_iter().any(|request| {
            request.url.contains("google/tasklists") && request.method.as_str() == "POST"
        }),
        "nothing was made on Google: the orphan was bound instead"
    );
}

/// Task 6db679c3. A container already bound to another list is not an orphan — adopting it
/// would silently move somebody's sync from one list to another.
#[tokio::test]
async fn new_list_does_not_adopt_a_google_list_that_is_already_linked() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json(
                "google/tasklists",
                200,
                json!({ "tasklists": [{ "id": "c-taken", "name": "Garden" }] }),
            )
            .push_json(
                "google/links",
                200,
                json!({
                    "links": [{
                        "id": "link-1",
                        "astridListId": "other",
                        "remoteContainerId": "c-taken",
                    }],
                }),
            )
            .push_json(
                "google/tasklists",
                200,
                json!({ "tasklist": { "id": "c9", "name": "Garden" } }),
            )
            .push_json(
                "google/links",
                200,
                json!({ "link": { "id": "link-9", "remoteContainerId": "c9" } }),
            ),
    );
    fixture
        .store
        .upsert_list(&a_list("cm3real", "Garden"))
        .expect("writes");

    let answer = fixture
        .service
        .create_and_link_google_list("cm3real", "Garden")
        .await
        .expect("links");

    assert_eq!(answer["containerId"], "c9");
    assert_eq!(answer["created"], true);
}

/// Task 6db679c3. The name the server stores for the container travels with the link, so a
/// screen never has to render an opaque Google tasklist id while it waits for a second call.
#[tokio::test]
async fn a_link_carries_the_containers_name() {
    let fixture = fixture_with(StubTransport::new().push_json(
        "google/links",
        200,
        json!({
            "links": [{
                "id": "link-1",
                "astridListId": "cm3real",
                "remoteContainerId": "c1",
                "remoteContainerName": "Garden (Google)",
            }],
        }),
    ));

    let links = fixture
        .service
        .links(Provider::GoogleTasks)
        .await
        .expect("reads");

    assert_eq!(
        links[0].remote_container_name.as_deref(),
        Some("Garden (Google)")
    );
}

/// Manual is the default and has to stay one: a mode this build does not recognise must not
/// start making lists on somebody's account.
#[tokio::test]
async fn manual_mode_links_nothing() {
    let fixture = fixture_with(auto_link_transport("manual"));
    fixture
        .store
        .upsert_list(&a_list("cm3real", "Groceries"))
        .expect("writes");

    let report = fixture.service.auto_link_google().await.expect("links");

    assert_eq!(report, AutoLinkReport::default());
}

#[tokio::test]
async fn a_mode_this_build_does_not_know_is_manual() {
    let fixture = fixture_with(auto_link_transport("all_the_things_v3"));
    let settings = fixture.service.auto_link_settings().await.expect("reads");
    assert_eq!(settings.mode, SyncMode::Manual);
}

/// A list made here has a temporary id until it reaches the server. Linking a remote list to
/// that id would attach the link to something about to be given a different one.
#[tokio::test]
async fn a_list_made_here_waits_for_its_real_id_before_it_is_linked() {
    let fixture = fixture_with(auto_link_transport("all_google_to_astrid"));

    let report = fixture.service.auto_link_google().await.expect("links");

    assert_eq!(report.lists_created, 1);
    assert_eq!(report.waiting_to_be_created, 1);
    assert_eq!(report.linked, 0);
    assert!(
        fixture
            .store
            .lists()
            .expect("reads")
            .iter()
            .any(|list| list.name == "Groceries"),
        "and the list is there, so the next pass adopts it rather than making another"
    );
}

/// Without this, deleting an auto-linked list is pointless: the next pass sees an unlinked
/// remote list and makes it again, and again after that.
#[tokio::test]
async fn a_remote_list_somebody_said_no_to_is_not_offered_again() {
    let fixture = fixture_with(auto_link_transport("all_google_to_astrid"));
    ledger::exclude(&fixture.store, PROVIDER_KEY, "c1").expect("excludes");

    let report = fixture.service.auto_link_google().await.expect("links");

    assert_eq!(report.linked, 0);
    assert_eq!(report.lists_created, 0);
    assert_eq!(report.failed, 0);
    assert!(
        fixture.transport.requests().iter().any(|request| {
            request.method.as_str() == "PATCH" && request.url.contains("integrations")
        }),
        "and the account is told, so the other devices stop offering it too"
    );
}

/// The default remote list pairs with My Tasks, not with a list of its own — so a mode that
/// only mirrors outward leaves it alone.
#[tokio::test]
async fn the_default_remote_list_is_not_made_into_an_ordinary_list_when_mirroring_outward() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json(
                "/api/v1/integrations",
                200,
                json!({
                    "integrations": [{
                        "provider": "GOOGLE_TASKS",
                        "metadata": { "googleSyncMode": "all_astrid_to_google" },
                    }],
                }),
            )
            .push_json(
                "google/tasklists",
                200,
                json!({
                    "tasklists": [{ "id": "default-list", "name": "My Tasks" }],
                    "defaultId": "default-list",
                }),
            )
            .push_json("google/links", 200, json!({ "links": [] }))
            .push_json(
                "google/tasklists",
                200,
                json!({ "tasklist": { "id": "c9", "name": "Work" } }),
            )
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );
    fixture
        .store
        .upsert_list(&a_list("cm3real", "Work"))
        .expect("writes");

    let report = fixture.service.auto_link_google().await.expect("links");

    assert_eq!(
        report.containers_created, 1,
        "the local list got a remote one of its own"
    );
    assert_eq!(report.linked, 1);
}

/// An account with no Google integration, and an account whose metadata has no tombstones,
/// both have to be ordinary rather than an error.
#[tokio::test]
async fn a_status_without_tombstones_is_not_a_problem() {
    let fixture = fixture(200, json!([]));
    fixture
        .service
        .merge_server_tombstones(&json!({ "integrations": [] }))
        .expect("merges");
    fixture
        .service
        .merge_server_tombstones(&json!({}))
        .expect("merges");

    assert!(ledger::tombstoned(&fixture.store, PROVIDER_KEY).is_empty());
}

/// The whole point of the ledger: a task deleted here takes its twin with it, on a later pass,
/// with nothing but what was written down at delete time.
#[tokio::test]
async fn a_task_deleted_here_has_its_twin_removed_over_there() {
    let fixture = fixture(200, json!([]));
    ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

    let report = fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    assert_eq!(report.removed_remotely, 1);
    assert!(
        ledger::pending(&fixture.store, PROVIDER_KEY).is_empty(),
        "the work is done, so it stops being pending"
    );
    assert!(
        ledger::tombstoned(&fixture.store, PROVIDER_KEY).contains(&"r1".to_string()),
        "but the deletion stays a fact, or the next pull brings it back"
    );
    let sent = fixture.transport.requests();
    assert_eq!(sent[0].method.as_str(), "DELETE");
    assert!(sent[0].url.contains("remoteId=r1"), "{}", sent[0].url);
}

/// A pass covers one container. Deleting out of another would remove somebody's task from a
/// list this pass has nothing to do with.
#[tokio::test]
async fn a_pending_deletion_from_another_container_is_left_alone() {
    let fixture = fixture(200, json!([]));
    ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "other-list").expect("records");

    let report = fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    assert_eq!(report.removed_remotely, 0);
    assert_eq!(ledger::pending(&fixture.store, PROVIDER_KEY).len(), 1);
}

/// Somebody deleted it over there too. That is the work finished, not a failure to retry for
/// ever.
#[tokio::test]
async fn a_twin_that_is_already_gone_stops_being_retried() {
    let fixture = fixture(404, json!([]));
    ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

    let report = fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    assert_eq!(report.removed_remotely, 0, "nothing was removed by us");
    assert!(ledger::pending(&fixture.store, PROVIDER_KEY).is_empty());
}

/// A server having a bad minute must not lose a deletion — the twin would stay for ever.
#[tokio::test]
async fn a_deletion_the_server_refused_is_kept_for_the_next_pass() {
    let fixture = fixture(500, json!([]));
    ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

    fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    assert_eq!(ledger::pending(&fixture.store, PROVIDER_KEY).len(), 1);
}

/// Without this, the deletion undoes itself: the twin is removed, the pull still lists it, and
/// the task comes back on every pass for ever.
#[tokio::test]
async fn a_pull_refuses_to_bring_back_what_was_deleted_here() {
    let fixture = fixture(200, json!([{ "remoteId": "r1", "title": "Buy milk" }]));
    ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

    let report = fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");

    assert_eq!(report.applied, 0);
    assert!(
        fixture.store.task("ext_r1").expect("reads").is_none(),
        "the task somebody deleted did not come back"
    );
}

/// The proxy's real shapes (`api/v1/sync/google/{tasks,task-links}`): links under `links`
/// naming `astridTaskId`, and a deletion as `metadata.deleted: "1"`. Read with other names,
/// every link looked absent and no remote deletion was ever seen.
#[tokio::test]
async fn a_deletion_in_google_reaches_the_linked_task_in_the_proxys_shapes() {
    let fixture = fixture_with(
        StubTransport::new()
            .push_json(
                "google/tasks",
                200,
                json!({ "items": [{
                    "remoteId": "tasklist-1:r1", "title": "Buy milk",
                    "metadata": { "googleTaskId": "r1", "parent": "", "deleted": "1" }
                }], "cursor": null, "truncated": false }),
            )
            .push_json(
                "google/task-links",
                200,
                json!({ "links": [{ "remoteId": "tasklist-1:r1", "astridTaskId": "t-local",
                                    "remoteContainerId": "tasklist-1" }] }),
            )
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );
    fixture
        .store
        .upsert_task(&crate::model::Task::new("t-local", "Buy milk"))
        .expect("stores");

    let report = fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");
    assert_eq!(report.deleted_locally, 1, "{report:?}");
    assert!(fixture.store.task("t-local").expect("reads").is_none());
    let journal = crate::outbox::journal::all(&fixture.store).expect("reads");
    assert!(
        journal
            .iter()
            .any(|entry| entry.kind == crate::outbox::kind::DELETE_TASK),
        "the deletion reaches astrid-web, or the next pull brings the task back"
    );
}

/// A task pulled before its create had reached astrid-web is matched through this device's own
/// note on the next pass — and must then be linked on the server too, or every other device
/// makes a second twin of it.
#[tokio::test]
async fn a_twin_known_only_here_is_linked_on_the_server_once_it_can_be() {
    let transport = StubTransport::new()
        .push_json(
            "google/tasks",
            200,
            json!({ "items": [{ "remoteId": "tasklist-1:r1", "title": "Buy milk" }] }),
        )
        .push_json("google/task-links", 200, json!({ "links": [] }))
        .push_json("google/task-links", 200, json!({ "links": [] }))
        .fallback(Ok(crate::api::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        }));
    let sent = transport.recorded.clone();
    let fixture = fixture_with(transport);
    fixture
        .store
        .upsert_task(&crate::model::Task::new("t-real", "Buy milk"))
        .expect("stores");
    ledger::remember_links(
        &fixture.store,
        PROVIDER_KEY,
        "tasklist-1",
        [("t-real".to_string(), "tasklist-1:r1".to_string())],
    )
    .expect("remembers");

    fixture
        .service
        .sync_google_link(&link())
        .await
        .expect("a pass");
    let linked = sent.lock().expect("lock").iter().any(|request| {
        request.method == crate::api::Method::Put
            && request.url.contains("google/task-links")
            && request
                .body
                .as_ref()
                .is_some_and(|body| String::from_utf8_lossy(body).contains("t-real"))
    });
    assert!(linked, "the server was told which task mirrors r1");
}
