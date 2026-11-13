//! AWTD2-56 — the Google pass at parity with Apple's `GoogleTasksSyncService`.
//!
//! Against a stateful fake of astrid-web's proxy (`api/v1/sync/google/{tasks,task-links}`), so a
//! test asserts what a pass leaves behind — on this machine and over there — rather than the order
//! it happened to ask in.
//!
//! Split out of `google.rs` by task bb4f69ca, unchanged; `super` still means `external::google`.

use super::*;

use crate::api::{ApiClient, HttpRequest, HttpResponse, HttpTransport, Method, TransportError};
use crate::model::date;
use crate::platform::{FixedClock, MemorySecureStore};
use crate::services::Context;
use crate::store::Store;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, Mutex};

const NOW: &str = "2026-09-07T12:00:00Z";
const CONTAINER: &str = "tasklist-1";

/// What the proxy holds: Google's items for one task list, and the server's link rows.
#[derive(Default)]
struct Proxy {
    /// Proxy-shaped items (`remoteId`, `title`, `metadata`, …).
    items: Vec<Value>,
    /// What a cursor pull answers with, when it is not everything.
    window: Option<Vec<Value>>,
    links: Vec<Value>,
    /// The complete listing (`full=1`) says it was cut short.
    full_truncated: bool,
    /// The complete listing fails.
    full_fails: bool,
    /// Titles whose push Google refuses.
    refuse: Vec<String>,
    next_id: u32,
    sync_mode: String,
    excluded: String,
}

struct FakeProxy {
    state: Mutex<Proxy>,
    sent: Mutex<Vec<HttpRequest>>,
}

fn ok(body: Value) -> std::result::Result<HttpResponse, TransportError> {
    status(200, body)
}

fn status(code: u16, body: Value) -> std::result::Result<HttpResponse, TransportError> {
    Ok(HttpResponse {
        status: code,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.to_string().into_bytes(),
    })
}

#[async_trait]
impl HttpTransport for FakeProxy {
    async fn send(
        &self,
        request: HttpRequest,
    ) -> std::result::Result<HttpResponse, TransportError> {
        self.sent.lock().expect("lock").push(request.clone());
        let body: Value = request
            .body
            .as_deref()
            .and_then(|body| serde_json::from_slice(body).ok())
            .unwrap_or(Value::Null);
        let mut proxy = self.state.lock().expect("lock");
        let url = request.url.as_str();
        if url.contains("/api/v1/integrations") {
            return ok(json!({ "integrations": [{
                "provider": "GOOGLE_TASKS",
                "metadata": {
                    "googleSyncMode": proxy.sync_mode,
                    "excludedTasklists": proxy.excluded,
                },
            }] }));
        }
        if url.contains("google/task-links") {
            return match request.method {
                Method::Get => ok(json!({ "links": proxy.links })),
                Method::Put => {
                    let task_id = body["astridTaskId"].clone();
                    proxy.links.retain(|link| link["astridTaskId"] != task_id);
                    proxy.links.push(json!({
                        "astridTaskId": task_id,
                        "remoteId": body["remoteId"],
                        "remoteContainerId": body["remoteContainerId"],
                        "astridUpdatedAt": body["astridUpdatedAt"],
                        "remoteUpdatedAt": body["remoteUpdatedAt"],
                    }));
                    ok(json!({ "link": {} }))
                }
                _ => ok(json!({})),
            };
        }
        if url.contains("google/tasks") {
            match request.method {
                Method::Get if url.contains("full=1") => {
                    if proxy.full_fails {
                        return status(502, json!({ "error": "Google error" }));
                    }
                    return ok(json!({
                        "items": proxy.items,
                        "cursor": null,
                        "truncated": proxy.full_truncated,
                    }));
                }
                Method::Get => {
                    let items = proxy.window.clone().unwrap_or_else(|| proxy.items.clone());
                    return ok(json!({ "items": items, "cursor": "c-next", "truncated": false }));
                }
                Method::Post if body["action"] == "commitCursor" => {
                    return ok(json!({ "ok": true }))
                }
                Method::Post => {
                    let title = body["title"].as_str().unwrap_or_default().to_string();
                    if proxy.refuse.contains(&title) {
                        return status(404, json!({ "error": "Google error" }));
                    }
                    let stamp = "2026-09-07T12:00:05.000Z";
                    let completed = body["completed"].as_bool().unwrap_or(false);
                    if let Some(remote_id) = body["remoteId"].as_str() {
                        let Some(item) = proxy
                            .items
                            .iter_mut()
                            .find(|item| item["remoteId"] == remote_id)
                        else {
                            return status(404, json!({ "error": "Google error" }));
                        };
                        item["title"] = json!(title);
                        item["notes"] = body["notes"].clone();
                        item["completed"] = json!(completed);
                        item["remoteUpdatedAt"] = json!(stamp);
                        return ok(json!({ "remoteId": remote_id, "remoteUpdatedAt": stamp }));
                    }
                    proxy.next_id += 1;
                    let remote_id = format!("{CONTAINER}:new{}", proxy.next_id);
                    let parent = body["parentRemoteId"]
                        .as_str()
                        .and_then(|parent| parent.split(':').next_back())
                        .unwrap_or_default()
                        .to_string();
                    proxy.items.push(json!({
                        "remoteId": remote_id,
                        "title": title,
                        "notes": body["notes"],
                        "completed": completed,
                        "remoteUpdatedAt": stamp,
                        "metadata": { "parent": parent, "deleted": "" },
                    }));
                    return ok(json!({ "remoteId": remote_id, "remoteUpdatedAt": stamp }));
                }
                Method::Delete => {
                    let remote = url
                        .split("remoteId=")
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let remote = remote.replace("%3A", ":");
                    proxy
                        .items
                        .retain(|item| item["remoteId"] != remote.as_str());
                    return ok(json!({ "success": true }));
                }
                _ => {}
            }
        }
        ok(json!({}))
    }
}

struct Pass {
    service: ExternalSyncService,
    store: Arc<Store>,
    proxy: Arc<FakeProxy>,
}

impl Pass {
    fn new(proxy: Proxy) -> Self {
        let proxy = Arc::new(FakeProxy {
            state: Mutex::new(proxy),
            sent: Mutex::new(Vec::new()),
        });
        let store = Arc::new(Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                proxy.clone(),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            Arc::new(FixedClock::at(date::parse(NOW).expect("an instant"))),
        );
        let mut user = crate::model::User::new("u1");
        user.name = Some("Ada".into());
        store
            .set_metadata(
                "account.current-user",
                &serde_json::to_string(&user).expect("encodes"),
            )
            .expect("writes");
        Pass {
            service: context.external(),
            store,
            proxy,
        }
    }

    async fn run(&self) -> PassReport {
        self.service
            .sync_google_link(&ExternalLink {
                id: "link-1".into(),
                astrid_list_id: "l1".into(),
                remote_container_id: CONTAINER.into(),
                remote_container_name: None,
                cursor: None,
            })
            .await
            .expect("a pass")
    }

    fn task(&self, id: &str) -> Option<Task> {
        self.store.task(id).expect("reads")
    }

    fn titled(&self, title: &str) -> Vec<Task> {
        self.store
            .tasks()
            .expect("reads")
            .into_iter()
            .filter(|task| task.title == title)
            .collect()
    }

    fn sent(&self, method: Method, fragment: &str) -> Vec<Value> {
        self.proxy
            .sent
            .lock()
            .expect("lock")
            .iter()
            .filter(|request| request.method == method && request.url.contains(fragment))
            .map(|request| {
                request
                    .body
                    .as_deref()
                    .and_then(|body| serde_json::from_slice(body).ok())
                    .unwrap_or(Value::Null)
            })
            .collect()
    }

    /// Pushes to Google: creates and patches, not cursor commits.
    fn pushes(&self) -> Vec<Value> {
        self.sent(Method::Post, "google/tasks")
            .into_iter()
            .filter(|body| body["action"].is_null())
            .collect()
    }

    fn remote(&self, remote_id: &str) -> Option<Value> {
        let proxy = self.proxy.state.lock().expect("lock");
        proxy
            .items
            .iter()
            .find(|item| item["remoteId"] == remote_id)
            .cloned()
    }

    fn server_link(&self, task_id: &str) -> Option<Value> {
        let proxy = self.proxy.state.lock().expect("lock");
        proxy
            .links
            .iter()
            .find(|link| link["astridTaskId"] == task_id)
            .cloned()
    }
}

fn item(id: &str, title: &str, updated: &str) -> Value {
    json!({
        "remoteId": format!("{CONTAINER}:{id}"),
        "title": title,
        "notes": null,
        "completed": false,
        "dueDate": null,
        "completedAt": null,
        "remoteUpdatedAt": updated,
        "metadata": { "googleTaskId": id, "parent": "", "position": "", "deleted": "" },
    })
}

fn link_row(task: &str, remote: &str, astrid_at: &str, remote_at: &str) -> Value {
    json!({
        "astridTaskId": task,
        "remoteId": format!("{CONTAINER}:{remote}"),
        "remoteContainerId": CONTAINER,
        "astridUpdatedAt": astrid_at,
        "remoteUpdatedAt": remote_at,
    })
}

/// A task in the linked list, with a server id and a stamp.
fn listed(id: &str, title: &str, updated: &str) -> Task {
    let mut task = Task::new(id, title);
    task.list_ids = Some(vec!["l1".into()]);
    task.updated_at = date::parse(updated);
    task
}

// ── Last write wins ─────────────────────────────────────────────────────────────────────

/// AWTD2-56: an edit made here and not yet mirrored is newer than Google's change, so the pull
/// must leave it alone — and the push carries it out. The old pass overwrote it.
#[tokio::test]
async fn awtd2_56_a_pull_does_not_overwrite_a_fresher_local_edit() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Google's title", "2026-09-07T11:30:00.000Z")],
        links: vec![link_row(
            "t1",
            "r1",
            "2026-09-07T11:00:00.000Z",
            "2026-09-07T11:00:00.000Z",
        )],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "My edit", "2026-09-07T11:45:00Z"))
        .expect("writes");

    pass.run().await;

    assert_eq!(pass.task("t1").expect("kept").title, "My edit");
    assert_eq!(
        pass.remote("tasklist-1:r1").expect("there")["title"],
        "My edit",
        "and the local edit went out"
    );
}

/// AWTD2-56: the other half — a Google change newer than anything here is taken.
#[tokio::test]
async fn awtd2_56_a_newer_remote_change_is_taken() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Google's title", "2026-09-07T11:50:00.000Z")],
        links: vec![link_row(
            "t1",
            "r1",
            "2026-09-07T11:00:00.000Z",
            "2026-09-07T11:00:00.000Z",
        )],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Old title", "2026-09-07T11:00:00Z"))
        .expect("writes");

    let report = pass.run().await;

    assert_eq!(report.applied, 1);
    assert_eq!(pass.task("t1").expect("kept").title, "Google's title");
    let link = pass.server_link("t1").expect("linked");
    assert_eq!(
        link["remoteUpdatedAt"], "2026-09-07T11:50:00.000Z",
        "the pull watermark moved, so the change is not applied twice"
    );
    assert!(
        link["astridUpdatedAt"].is_string(),
        "and the local one, so the change is not echoed back"
    );
    assert!(
        pass.pushes().is_empty(),
        "nothing was echoed back: {:?}",
        pass.pushes()
    );
}

/// AWTD2-56: an item at its own watermark is our echo; nothing applies and nothing is sent.
#[tokio::test]
async fn awtd2_56_an_echo_is_neither_applied_nor_sent_back() {
    let stamp = "2026-09-07T11:00:00.000Z";
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Same", stamp)],
        links: vec![link_row("t1", "r1", stamp, stamp)],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Same", "2026-09-07T11:00:00Z"))
        .expect("writes");

    let report = pass.run().await;

    assert_eq!(report.applied, 0);
    assert!(pass.pushes().is_empty());
}

/// AWTD2-56: a twin known only to this device, whose local edit is newer than Google's, is
/// linked on the server without claiming the two sides agree — so the edit still goes out.
#[tokio::test]
async fn awtd2_56_a_twin_known_only_here_still_sends_its_newer_edit() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Google's title", "2026-09-07T11:30:00.000Z")],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "My edit", "2026-09-07T11:45:00Z"))
        .expect("writes");
    ledger::remember_links(
        &pass.store,
        PROVIDER_KEY,
        CONTAINER,
        [("t1".to_string(), "tasklist-1:r1".to_string())],
    )
    .expect("remembers");
    pass.run().await;

    assert_eq!(pass.task("t1").expect("kept").title, "My edit");
    assert_eq!(
        pass.remote("tasklist-1:r1").expect("there")["title"],
        "My edit"
    );
}

// ── Duplicates ──────────────────────────────────────────────────────────────────────────

/// AWTD2-56: a link whose task this machine does not hold (not loaded yet, or deleted here and
/// not yet on the server) is not an invitation to make a second task.
#[tokio::test]
async fn awtd2_56_a_linked_item_whose_task_is_missing_here_is_not_imported_again() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
        links: vec![link_row(
            "t-elsewhere",
            "r1",
            "2026-09-07T11:00:00.000Z",
            "2026-09-07T11:00:00.000Z",
        )],
        ..Default::default()
    });

    pass.run().await;

    assert!(pass.titled("Buy milk").is_empty(), "no second task");
}

/// AWTD2-56: the first pass over a list somebody already keeps on both sides adopts the local
/// task of the same title rather than making a second one.
#[tokio::test]
async fn awtd2_56_a_pulled_item_adopts_the_one_local_task_of_its_title() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert_eq!(pass.titled("Buy milk").len(), 1, "adopted, not duplicated");
    assert_eq!(
        pass.server_link("t1").expect("linked")["remoteId"],
        "tasklist-1:r1"
    );
    assert!(
        pass.pushes().is_empty(),
        "and no twin was made for it either"
    );
}

/// AWTD2-56: two local tasks of that title is a guess, and a guess is not adopted.
#[tokio::test]
async fn awtd2_56_an_ambiguous_title_is_not_adopted() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
        ..Default::default()
    });
    for id in ["t1", "t2"] {
        pass.store
            .upsert_task(&listed(id, "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");
    }

    pass.run().await;

    for id in ["t1", "t2"] {
        assert_ne!(
            pass.server_link(id).map(|link| link["remoteId"].clone()),
            Some(json!("tasklist-1:r1")),
            "{id} was not guessed at"
        );
    }
}

/// AWTD2-56: before creating a twin, the push looks through the complete listing for an
/// unlinked one of the same title — an item outside this pass's cursor window included.
#[tokio::test]
async fn awtd2_56_a_push_adopts_an_unlinked_remote_item_of_the_same_title() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Buy milk", "2026-09-01T00:00:00.000Z")],
        window: Some(Vec::new()),
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert!(
        pass.pushes().is_empty(),
        "no twin made: {:?}",
        pass.pushes()
    );
    assert_eq!(
        pass.server_link("t1").expect("linked")["remoteId"],
        "tasklist-1:r1"
    );
}

/// AWTD2-56: a deleted Google item is not a twin to adopt — linking to it would have the next
/// absence pass delete the local task. (Apple adopts it; see docs/CONTRACTS.md D39.)
#[tokio::test]
async fn awtd2_56_a_deleted_remote_item_is_not_adopted() {
    let mut deleted = item("r1", "Buy milk", "2026-09-01T00:00:00.000Z");
    deleted["metadata"]["deleted"] = json!("1");
    let pass = Pass::new(Proxy {
        items: vec![deleted],
        window: Some(Vec::new()),
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert_ne!(
        pass.server_link("t1").expect("linked")["remoteId"],
        "tasklist-1:r1"
    );
    assert!(pass.task("t1").is_some(), "and the task is still here");
}

/// AITD-463 (D39): an item this device deleted is not a twin to adopt either, though Google
/// still lists it live — the iOS guard this replaces (`GooglePushTwin.find`) refused it too.
#[tokio::test]
async fn aitd463_a_tombstoned_remote_item_is_not_adopted() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Buy milk", "2026-09-01T00:00:00.000Z")],
        window: Some(Vec::new()),
        ..Default::default()
    });
    ledger::record_tombstone(&pass.store, PROVIDER_KEY, "tasklist-1:r1").expect("records");
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert_ne!(
        pass.server_link("t1").expect("linked")["remoteId"],
        "tasklist-1:r1"
    );
    assert!(pass.task("t1").is_some(), "and the task is still here");
}

/// AITD-463 (D39): among same-title items the deleted one and the one already linked are
/// passed over, and the live unlinked one is adopted.
#[tokio::test]
async fn aitd463_adoption_skips_deleted_and_linked_items_for_the_live_unlinked_one() {
    let mut deleted = item("r1", "Buy milk", "2026-09-01T00:00:00.000Z");
    deleted["metadata"]["deleted"] = json!("1");
    let stamp = "2026-09-07T11:00:00.000Z";
    let pass = Pass::new(Proxy {
        items: vec![
            deleted,
            item("r2", "Buy milk", stamp),
            item("r3", "Buy milk", "2026-09-01T00:00:00.000Z"),
            item("r4", "Bread", "2026-09-01T00:00:00.000Z"),
        ],
        window: Some(Vec::new()),
        links: vec![link_row("t0", "r2", stamp, stamp)],
        ..Default::default()
    });
    let mut other = listed("t0", "Buy milk", stamp);
    other.updated_at = date::parse(stamp);
    pass.store.upsert_task(&other).expect("writes");
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert_eq!(
        pass.server_link("t1").expect("linked")["remoteId"],
        "tasklist-1:r3"
    );
}

/// AITD-463 (D39): My Tasks' push makes the same refusal — the iOS guard covered both sites.
#[tokio::test]
async fn aitd463_my_tasks_does_not_adopt_a_deleted_remote_item() {
    let mut deleted = item("r1", "Ring the dentist", "2026-09-01T00:00:00.000Z");
    deleted["metadata"]["deleted"] = json!("1");
    let pass = Pass::new(Proxy {
        items: vec![deleted],
        sync_mode: "all_bidirectional".into(),
        ..Default::default()
    });
    let mut mine = Task::new("t1", "Ring the dentist");
    mine.assignee_id = Some("u1".into());
    mine.updated_at = date::parse("2026-09-07T10:00:00Z");
    pass.store.upsert_task(&mine).expect("writes");

    pass.service.sync_my_tasks(CONTAINER).await.expect("a pass");

    assert_ne!(
        pass.server_link("t1").map(|link| link["remoteId"].clone()),
        Some(json!("tasklist-1:r1"))
    );
    assert!(pass.task("t1").is_some(), "and the task is still here");
}

/// AWTD2-56: when the complete listing was cut short, the twin may simply be past its end —
/// so known twins are patched and none is created.
#[tokio::test]
async fn awtd2_56_no_twin_is_created_when_the_listing_was_truncated() {
    let pass = Pass::new(Proxy {
        window: Some(Vec::new()),
        full_truncated: true,
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    let report = pass.run().await;

    assert!(pass.pushes().is_empty(), "{:?}", pass.pushes());
    assert_eq!(report.pushed, 0);
}

/// AWTD2-56: nor when the listing could not be had at all — unknown is not empty.
#[tokio::test]
async fn awtd2_56_no_twin_is_created_when_the_listing_failed() {
    let pass = Pass::new(Proxy {
        window: Some(Vec::new()),
        full_fails: true,
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert!(pass.pushes().is_empty(), "{:?}", pass.pushes());
}

/// AWTD2-56: a subtask is created under its parent's twin, not at the top level, so the round
/// trip keeps it nested.
#[tokio::test]
async fn awtd2_56_a_new_subtask_is_created_under_its_parents_twin() {
    let stamp = "2026-09-07T10:00:00.000Z";
    let pass = Pass::new(Proxy {
        items: vec![item("p", "Parent", stamp)],
        window: Some(Vec::new()),
        links: vec![link_row("parent", "p", stamp, stamp)],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("parent", "Parent", "2026-09-07T10:00:00Z"))
        .expect("writes");
    let mut child = listed("child", "Child", "2026-09-07T10:00:00Z");
    child.parent_task_id = Some("parent".into());
    pass.store.upsert_task(&child).expect("writes");

    pass.run().await;

    let created = pass
        .pushes()
        .into_iter()
        .find(|body| body["title"] == "Child")
        .expect("the child was pushed");
    assert_eq!(created["parentRemoteId"], "tasklist-1:p");
}

/// AWTD2-56: a link the server files under another container is that container's pass's to
/// push. Patching it from here addresses the wrong task list.
#[tokio::test]
async fn awtd2_56_a_link_from_another_container_is_not_pushed_here() {
    let pass = Pass::new(Proxy {
        links: vec![json!({
            "astridTaskId": "t1",
            "remoteId": "other-list:r1",
            "remoteContainerId": "other-list",
            "astridUpdatedAt": null,
            "remoteUpdatedAt": null,
        })],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert!(
        !pass
            .pushes()
            .iter()
            .any(|body| body["remoteId"] == "other-list:r1"),
        "{:?}",
        pass.pushes()
    );
}

/// AWTD2-56: a failed push is not watermarked, so it is tried again next pass without any
/// stamp of its own; the rest of the list still goes.
#[tokio::test]
async fn awtd2_56_a_failed_push_is_retried_and_does_not_stop_the_list() {
    let pass = Pass::new(Proxy {
        refuse: vec!["First".into()],
        ..Default::default()
    });
    for (id, title) in [("t1", "First"), ("t2", "Second")] {
        pass.store
            .upsert_task(&listed(id, title, "2026-09-07T10:00:00Z"))
            .expect("writes");
    }

    let report = pass.run().await;
    assert_eq!(report.pushed, 1, "the second task still went");

    pass.proxy.state.lock().expect("lock").refuse.clear();
    let report = pass.run().await;
    assert_eq!(report.pushed, 1, "and the first went on the next pass");
    assert!(pass.server_link("t1").is_some());
}

// ── What only a complete listing can show ───────────────────────────────────────────────

/// AWTD2-56: a pair whose completion drifted (a botched pass, an item outside every cursor
/// window since) is repaired from the complete listing when the local task is untouched.
#[tokio::test]
async fn awtd2_56_completion_drift_is_repaired_from_the_complete_listing() {
    let stamp = "2026-09-07T10:00:00.000Z";
    let mut done = item("r1", "Buy milk", stamp);
    done["completed"] = json!(true);
    done["completedAt"] = json!("2026-09-06T08:00:00.000Z");
    let pass = Pass::new(Proxy {
        items: vec![done],
        window: Some(Vec::new()),
        links: vec![link_row("t1", "r1", stamp, stamp)],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    let task = pass.task("t1").expect("kept");
    assert!(task.completed, "the drift was repaired");
    assert_eq!(task.completed_at, date::parse("2026-09-06T08:00:00Z"));
}

/// AWTD2-56: an item gone from a complete listing was deleted over there (Google models a move
/// as delete and insert), so its local twin goes — through the service, and tombstoned.
#[tokio::test]
async fn awtd2_56_a_twin_absent_from_a_complete_listing_is_deleted_here() {
    let stamp = "2026-09-07T10:00:00.000Z";
    let pass = Pass::new(Proxy {
        items: vec![item("r2", "Keep", stamp)],
        window: Some(Vec::new()),
        links: vec![
            link_row("t1", "r1", stamp, stamp),
            link_row("t2", "r2", stamp, stamp),
        ],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Gone", "2026-09-07T10:00:00Z"))
        .expect("writes");
    pass.store
        .upsert_task(&listed("t2", "Keep", "2026-09-07T10:00:00Z"))
        .expect("writes");

    let report = pass.run().await;

    assert_eq!(report.deleted_locally, 1);
    assert!(pass.task("t1").is_none());
    assert!(pass.task("t2").is_some());
    assert!(ledger::tombstoned(&pass.store, PROVIDER_KEY).contains(&"tasklist-1:r1".to_string()));
    assert!(
        crate::outbox::journal::all(&pass.store)
            .expect("reads")
            .iter()
            .any(|entry| entry.kind == crate::outbox::kind::DELETE_TASK),
        "the deletion reaches astrid-web"
    );
}

/// AWTD2-56: a twin the push has just made is absent from the listing fetched before it, and
/// that absence must not read as a deletion.
#[tokio::test]
async fn awtd2_56_a_twin_made_this_pass_is_not_deleted_by_its_absence() {
    let pass = Pass::new(Proxy {
        window: Some(Vec::new()),
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
        .expect("writes");

    let report = pass.run().await;

    assert_eq!(report.pushed, 1);
    assert_eq!(report.deleted_locally, 0);
    assert!(pass.task("t1").is_some());
}

/// AWTD2-56: the complete listing is fetched for deletions at most every five minutes when
/// nothing else in the pass needs it.
#[tokio::test]
async fn awtd2_56_the_complete_listing_is_not_fetched_every_pass() {
    let pass = Pass::new(Proxy {
        window: Some(Vec::new()),
        ..Default::default()
    });

    pass.run().await;
    pass.run().await;

    let full = pass
        .proxy
        .sent
        .lock()
        .expect("lock")
        .iter()
        .filter(|request| request.url.contains("full=1"))
        .count();
    assert_eq!(
        full, 1,
        "the second pass, a moment later, did not fetch it again"
    );
}

/// AWTD2-56: a truncated listing proves nothing about absence. Nothing is deleted.
#[tokio::test]
async fn awtd2_56_a_truncated_listing_deletes_nothing() {
    let stamp = "2026-09-07T10:00:00.000Z";
    let pass = Pass::new(Proxy {
        window: Some(Vec::new()),
        full_truncated: true,
        links: vec![link_row("t1", "r1", stamp, stamp)],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Still here", "2026-09-07T10:00:00Z"))
        .expect("writes");

    let report = pass.run().await;

    assert_eq!(report.deleted_locally, 0);
    assert!(pass.task("t1").is_some());
}

/// AWTD2-56: one item this build cannot read makes the listing incomplete, not empty. Read as
/// empty, every linked task in the list would be deleted for being absent from it.
#[tokio::test]
async fn awtd2_56_an_unreadable_listing_deletes_nothing() {
    let stamp = "2026-09-07T10:00:00.000Z";
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Still here", stamp), json!({ "title": "no id" })],
        window: Some(Vec::new()),
        links: vec![link_row("t1", "r1", stamp, stamp)],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Still here", "2026-09-07T10:00:00Z"))
        .expect("writes");
    pass.store
        .upsert_task(&listed("t2", "Unlinked", "2026-09-07T10:00:00Z"))
        .expect("writes");

    let report = pass.run().await;

    assert_eq!(report.deleted_locally, 0);
    assert!(pass.task("t1").is_some());
    assert!(
        !pass.pushes().iter().any(|body| body["title"] == "Unlinked"),
        "and an incomplete listing creates no twin either"
    );
}

/// AWTD2-56: a failed listing proves nothing either.
#[tokio::test]
async fn awtd2_56_a_failed_listing_deletes_nothing() {
    let stamp = "2026-09-07T10:00:00.000Z";
    let pass = Pass::new(Proxy {
        window: Some(Vec::new()),
        full_fails: true,
        links: vec![link_row("t1", "r1", stamp, stamp)],
        ..Default::default()
    });
    pass.store
        .upsert_task(&listed("t1", "Still here", "2026-09-07T10:00:00Z"))
        .expect("writes");

    pass.run().await;

    assert!(pass.task("t1").is_some());
}

/// AWTD2-56: completed history comes in as completed tasks — with Google's completion time —
/// never as open ones, and once.
#[tokio::test]
async fn awtd2_56_completed_history_is_backfilled_as_completed_and_once() {
    let mut done = item("r1", "Filed taxes", "2026-09-01T00:00:00.000Z");
    done["completed"] = json!(true);
    done["completedAt"] = json!("2026-08-31T09:00:00.000Z");
    let pass = Pass::new(Proxy {
        items: vec![done],
        ..Default::default()
    });

    let report = pass.run().await;
    pass.run().await;

    let imported = pass.titled("Filed taxes");
    assert_eq!(imported.len(), 1, "once, not once a pass");
    assert!(imported[0].completed, "completed, not open");
    assert_eq!(
        imported[0].completed_at,
        date::parse("2026-08-31T09:00:00Z")
    );
    assert_eq!(report.backfilled, 1);
}

// ── The cursor ──────────────────────────────────────────────────────────────────────────

/// AWTD2-56: a pulled item that could not be linked on the server (its task has not reached
/// astrid-web yet) keeps the cursor where it was, so the window is pulled again and the link
/// written then — instead of the item never being seen again.
#[tokio::test]
async fn awtd2_56_the_cursor_waits_for_every_pulled_item_to_be_linked() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
        ..Default::default()
    });

    pass.run().await;

    assert!(
        !pass
            .sent(Method::Post, "google/tasks")
            .iter()
            .any(|body| body["action"] == "commitCursor"),
        "not committed while the task's create is still in the journal"
    );
}

// ── My Tasks runs the same pass ─────────────────────────────────────────────────────────

/// AWTD2-56: My Tasks adopts by title too, among the unlisted tasks assigned to you.
#[tokio::test]
async fn awtd2_56_my_tasks_adopts_the_one_local_task_of_its_title() {
    let pass = Pass::new(Proxy {
        items: vec![item("r1", "Ring the dentist", "2026-09-07T11:30:00.000Z")],
        sync_mode: "all_bidirectional".into(),
        ..Default::default()
    });
    let mut mine = Task::new("t1", "Ring the dentist");
    mine.assignee_id = Some("u1".into());
    mine.updated_at = date::parse("2026-09-07T10:00:00Z");
    pass.store.upsert_task(&mine).expect("writes");

    pass.service.sync_my_tasks(CONTAINER).await.expect("a pass");

    assert_eq!(pass.titled("Ring the dentist").len(), 1);
    assert!(pass.server_link("t1").is_some());
    assert!(pass.pushes().is_empty(), "{:?}", pass.pushes());
}

// ── Linking by hand ─────────────────────────────────────────────────────────────────────

/// AWTD2-56: linking a list by hand takes back an earlier "no" — here and on the account — or
/// the all-lists modes go on refusing a list somebody has since chosen.
#[tokio::test]
async fn awtd2_56_linking_by_hand_clears_the_exclusion() {
    let pass = Pass::new(Proxy {
        excluded: "c1,c2".into(),
        ..Default::default()
    });
    ledger::exclude(&pass.store, PROVIDER_KEY, "c1").expect("excludes");

    pass.service
        .link(Provider::GoogleTasks, "l1", "c1")
        .await
        .expect("links");

    assert!(!ledger::excluded(&pass.store, PROVIDER_KEY).contains(&"c1".to_string()));
    let patched = pass.sent(Method::Patch, "/api/v1/integrations");
    assert_eq!(
        patched.last().expect("the account was told")["metadata"]["excludedTasklists"],
        "c2"
    );
}
