//! Keeping the cache and the server in step.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/SyncManager.swift`.
//!
//! One pass, in this order, and the order is the part that matters:
//!
//! 1. **Push.** Drain the Outbox first, so what the person did reaches the server before the
//!    server's answer is used to update what they see.
//! 2. **Fetch.** Lists, then tasks.
//! 3. **Apply.** Newer wins, with [`conflict`] deciding when both sides moved.
//!
//! Pushing is best-effort and fetching happens regardless. That is not tidiness: the Apple pass
//! pushed with a bare `try`, so one stuck local write stopped remote changes arriving at all until
//! the app was relaunched (task 3173727d). A local write that cannot be delivered is the Outbox's
//! problem, and it already knows how to keep trying.

pub mod conflict;
pub mod policy;

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::api::{endpoints, ApiClient, ApiError};
use crate::model::{date, Project, Task, TaskList};
use crate::outbox::Runner;
use crate::platform::Clock;
use crate::services::ServiceError;
use crate::store::Store;

/// Where the last successful pass got to.
const LAST_SYNC_KEY: &str = "sync.last-completed";

/// What one pass did. Returned rather than logged, so the shell can say "12 new tasks" and the
/// tests can assert on something.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub lists_added: usize,
    pub lists_updated: usize,
    pub tasks_added: usize,
    pub tasks_updated: usize,
    pub tasks_unchanged: usize,
    /// Local writes the push step could not deliver. The pass carried on regardless.
    pub pushes_failed: usize,
    /// Whether the fetch actually happened. False means the pass ran offline and changed nothing,
    /// which is a different thing from a pass that found nothing.
    pub fetched: bool,
}

pub struct SyncManager {
    client: Arc<ApiClient>,
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
    runner: Arc<Runner>,
}

impl SyncManager {
    pub fn new(
        client: Arc<ApiClient>,
        store: Arc<Store>,
        clock: Arc<dyn Clock>,
        runner: Arc<Runner>,
    ) -> Self {
        SyncManager {
            client,
            store,
            clock,
            runner,
        }
    }

    /// When the last pass finished, if one ever has.
    pub fn last_sync(&self) -> Option<DateTime<Utc>> {
        self.store
            .metadata(LAST_SYNC_KEY)
            .ok()
            .flatten()
            .as_deref()
            .and_then(date::parse)
    }

    /// One pass: push, fetch, apply.
    ///
    /// Never fails on being offline. A pass that cannot reach the server reports `fetched: false`
    /// and leaves the cache exactly as it was — which is the whole offline story, and why the
    /// return type is a report rather than an error.
    pub async fn sync(&self) -> SyncReport {
        let mut report = SyncReport::default();

        // Push first. Best-effort: a write that will not go is the Outbox's problem, and it keeps
        // trying on its own schedule.
        match self.runner.drain().await {
            Ok(drain) => report.pushes_failed = drain.dead_lettered,
            Err(_) => report.pushes_failed += 1,
        }

        match self.fetch_and_apply().await {
            Ok(fetched) => {
                report.lists_added = fetched.lists_added;
                report.lists_updated = fetched.lists_updated;
                report.tasks_added = fetched.tasks_added;
                report.tasks_updated = fetched.tasks_updated;
                report.tasks_unchanged = fetched.tasks_unchanged;
                report.fetched = true;
                // The boards too. This was written and never called, so a column added or a
                // default renamed on the web never reached a board here (task e5214fba). A
                // server without boards answers 404, which is not a failed pass.
                if let Err(error) = self.sync_projects().await {
                    tracing::debug!(%error, "projects did not sync; the boards are as they were");
                }
                let _ = self
                    .store
                    .set_metadata(LAST_SYNC_KEY, &date::format(self.clock.now()));
            }
            Err(error) => {
                tracing::warn!(%error, "sync could not fetch; the cache is unchanged");
            }
        }
        report
    }

    async fn fetch_and_apply(&self) -> Result<SyncReport, ServiceError> {
        let mut report = SyncReport::default();

        let lists = self
            .client
            .send_collection::<TaskList>(
                self.client.get(endpoints::LISTS),
                Some(endpoints::envelope::LISTS),
            )
            .await?;
        report_skips("lists", &lists.skipped);
        for list in &lists.items {
            match self.store.list(&list.id)? {
                Some(cached) => {
                    if is_newer(list.updated_at, cached.updated_at) {
                        self.store.upsert_list(list)?;
                        report.lists_updated += 1;
                    }
                }
                None => {
                    self.store.upsert_list(list)?;
                    report.lists_added += 1;
                }
            }
        }

        // Tasks come a page at a time, and the walk stops on a short page rather than on the
        // server's count — see `api::pagination` for why that count cannot be trusted.
        let tasks = self.fetch_all_tasks().await?;
        for task in &tasks {
            match self.store.task(&task.id)? {
                Some(cached) => {
                    if is_newer(task.updated_at, cached.updated_at) {
                        // Both sides may have moved. The resolver decides field by field.
                        let resolved = conflict::resolve(&cached, task);
                        self.store.upsert_task(&resolved)?;
                        report.tasks_updated += 1;
                    } else {
                        report.tasks_unchanged += 1;
                    }
                }
                None => {
                    self.store.upsert_task(task)?;
                    report.tasks_added += 1;
                }
            }
        }

        Ok(report)
    }

    async fn fetch_all_tasks(&self) -> Result<Vec<Task>, ApiError> {
        const PAGE: usize = 1000;
        crate::api::fetch_all(PAGE, |limit, offset| async move {
            let request = self
                .client
                .get(endpoints::TASKS)
                .query("limit", Some(limit.to_string()))
                .query("offset", Some(offset.to_string()))
                // The per-task embedded member arrays are large and redundant: permission checks
                // resolve membership from the list, not from a copy stapled to every task. Older
                // servers ignore the parameter and send them anyway.
                .query("leanListMembers", Some("1".to_string()));
            let page = self
                .client
                .send_collection::<Task>(request, Some(endpoints::envelope::TASKS))
                .await?;
            report_skips("tasks", &page.skipped);
            Ok(crate::api::Page {
                items: page.into_items(),
                total: None,
            })
        })
        .await
    }

    /// Fetch the projects. Separate from the main pass because a deployment without boards answers
    /// 404, and a pass that treated that as a failure would stop syncing tasks on an older server.
    pub async fn sync_projects(&self) -> Result<usize, ServiceError> {
        let projects = self
            .client
            .send_collection::<Project>(
                self.client.get(endpoints::PROJECTS),
                Some(endpoints::envelope::PROJECTS),
            )
            .await?;
        report_skips("projects", &projects.skipped);
        let items = projects.into_items();
        self.store.upsert_projects(&items)?;
        Ok(items.len())
    }
}

/// Whether the server's version is worth applying.
///
/// A version with no timestamp is treated as older, not newer: a thin payload must not overwrite a
/// full one just because it arrived second.
fn is_newer(incoming: Option<DateTime<Utc>>, cached: Option<DateTime<Utc>>) -> bool {
    match (incoming, cached) {
        (Some(incoming), Some(cached)) => incoming > cached,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Say what a lenient decode dropped. A row that vanishes silently is worse than a response that
/// fails loudly, which is the whole reason [`crate::model::lenient`] reports rather than discards.
fn report_skips(what: &str, skipped: &[crate::model::Skipped]) {
    for skip in skipped {
        tracing::warn!(
            collection = what,
            index = skip.index,
            id = skip.id.as_deref().unwrap_or("?"),
            reason = %skip.reason,
            "a row could not be read and was skipped"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{StubTransport, TransportError};
    use crate::platform::{FixedClock, MemorySecureStore};
    use serde_json::json;

    struct Fixture {
        sync: SyncManager,
        store: Arc<Store>,
    }

    fn fixture(transport: StubTransport) -> Fixture {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let clock = Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z"));
        let client = Arc::new(ApiClient::new(
            "https://astrid.cc",
            Arc::new(transport),
            Arc::new(MemorySecureStore::new()),
        ));
        let runner = Arc::new(Runner::new(client.clone(), store.clone(), clock.clone()));
        Fixture {
            sync: SyncManager::new(client, store.clone(), clock, runner),
            store,
        }
    }

    fn empty_task_pages(transport: StubTransport) -> StubTransport {
        transport.push_json("/api/v1/tasks", 200, json!({ "tasks": [] }))
    }

    #[tokio::test]
    async fn a_first_pass_brings_everything_in() {
        let fixture = fixture(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [{ "id": "l1", "name": "Home" }] }),
                )
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [{ "id": "t1", "title": "Buy milk", "listIds": ["l1"] }] }),
                )
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );

        let report = fixture.sync.sync().await;
        assert!(report.fetched);
        assert_eq!(report.lists_added, 1);
        assert_eq!(report.tasks_added, 1);
        assert_eq!(fixture.store.tasks_in_list("l1").expect("reads").len(), 1);
        assert_eq!(
            fixture.sync.last_sync().map(date::format).as_deref(),
            Some("2026-09-07T12:00:00Z")
        );
    }

    /// Offline is not a failure the user has to be told about — it is Tuesday. The cache stays as
    /// it was and the report says nothing was fetched.
    #[tokio::test]
    async fn a_pass_that_cannot_reach_the_server_changes_nothing_and_says_so() {
        let fixture =
            fixture(StubTransport::new().fallback(Err(TransportError::Unreachable("dns".into()))));
        fixture
            .store
            .upsert_task(&Task::new("t1", "Buy milk"))
            .expect("stores");

        let report = fixture.sync.sync().await;
        assert!(!report.fetched);
        assert_eq!(fixture.store.tasks().expect("reads").len(), 1);
        assert_eq!(
            fixture.sync.last_sync(),
            None,
            "a failed pass is not a pass"
        );
    }

    /// A thin payload arriving second must not overwrite a full one. "No timestamp" is older, not
    /// newer.
    #[tokio::test]
    async fn a_server_version_that_is_not_newer_is_left_alone() {
        let fixture = fixture(empty_task_pages(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [{ "id": "t1", "title": "Stale" }] }),
                ),
        ));
        let mut cached = Task::new("t1", "Current");
        cached.updated_at = date::parse("2026-09-07T12:00:00Z");
        fixture.store.upsert_task(&cached).expect("stores");

        let report = fixture.sync.sync().await;
        assert_eq!(report.tasks_unchanged, 1);
        assert_eq!(
            fixture
                .store
                .task("t1")
                .expect("reads")
                .expect("present")
                .title,
            "Current"
        );
    }

    /// The conflict rules apply on the way in: a completion made here survives a newer server
    /// version that has not heard about it.
    #[tokio::test]
    async fn a_newer_server_version_is_merged_rather_than_pasted_over() {
        let fixture = fixture(empty_task_pages(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [{
                        "id": "t1", "title": "Renamed elsewhere", "completed": false,
                        "updatedAt": "2026-09-07T12:05:00Z"
                    }] }),
                ),
        ));
        let mut cached = Task::new("t1", "Buy milk");
        cached.completed = true;
        cached.updated_at = date::parse("2026-09-07T12:00:00Z");
        fixture.store.upsert_task(&cached).expect("stores");

        fixture.sync.sync().await;
        let merged = fixture.store.task("t1").expect("reads").expect("present");
        assert_eq!(merged.title, "Renamed elsewhere");
        assert!(merged.completed, "the local completion survives the merge");
    }

    /// One unreadable row costs that row. The account's other tasks still arrive.
    #[tokio::test]
    async fn one_bad_row_does_not_empty_the_account() {
        let fixture = fixture(empty_task_pages(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [
                        { "id": "t1", "title": "Fine" },
                        { "title": "No id at all" },
                        { "id": "t3", "title": "Also fine" }
                    ] }),
                ),
        ));
        let report = fixture.sync.sync().await;
        assert_eq!(report.tasks_added, 2);
    }

    /// The pagination walk stops on a short page. A full page always costs one more request,
    /// because a full page might be the last one.
    #[tokio::test]
    async fn tasks_are_walked_page_by_page_until_a_short_one() {
        let mut transport =
            StubTransport::new().push_json("/api/v1/lists", 200, json!({ "lists": [] }));
        transport = transport.push_json(
            "/api/v1/tasks",
            200,
            json!({ "tasks": [{ "id": "t1" }, { "id": "t2" }] }),
        );
        let fixture = fixture(transport);

        // The first page is short (2 < 1000), so exactly one task request is made.
        let report = fixture.sync.sync().await;
        assert_eq!(report.tasks_added, 2);
    }

    /// A local write that will not go must not stop remote changes arriving — task 3173727d.
    #[tokio::test]
    async fn a_stuck_local_write_does_not_stop_the_fetch() {
        let fixture = fixture(
            StubTransport::new()
                // The push fails permanently...
                .push_json("/api/v1/tasks", 403, json!({ "error": "not yours" }))
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                // ...and the fetch still happens.
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [{ "id": "t9", "title": "From the server" }] }),
                ),
        );
        crate::outbox::journal::enqueue(
            &fixture.store,
            &crate::outbox::Entry::new(
                "e1",
                crate::outbox::kind::CREATE_TASK,
                json!({ "body": { "title": "Stuck" } }),
                "temp_stuck",
                date::parse("2026-09-07T12:00:00Z").expect("an instant"),
            ),
        )
        .expect("enqueues");

        let report = fixture.sync.sync().await;
        assert_eq!(report.pushes_failed, 1);
        assert!(report.fetched, "the fetch has to happen anyway");
        assert!(fixture.store.task("t9").expect("reads").is_some());
    }
}
