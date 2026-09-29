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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::api::{endpoints, ApiClient, ApiError};
use crate::model::{date, Project, Task, TaskList};
use crate::outbox::Runner;
use crate::platform::Clock;
use crate::services::ServiceError;
use crate::store::Store;

/// Where the last successful pass got to.
///
/// The instant the pass *started*, not the instant it finished: an edit made on the web while
/// the fetch was in flight has an `updatedAt` between the two, and stamping the end would skip
/// it forever.
const LAST_SYNC_KEY: &str = "sync.last-completed";

/// How far behind the stamp a delta pass asks from.
///
/// The stamp is this machine's clock and `updatedAt` is the server's, and the two disagree by
/// however wrong this machine's clock is. Asking for a little more than strictly necessary costs
/// a few rows that [`is_newer`] then declines to apply; asking for a little less loses an edit
/// silently. Five minutes covers any clock a machine that can still sign in is likely to have.
fn delta_overlap() -> chrono::Duration {
    chrono::Duration::minutes(5)
}

/// How old the stamp may be before a pass stops asking for a delta and pulls everything.
///
/// A delta pass learns about deletions only from the tombstones the server lists beside the
/// rows, and the server prunes those after a retention window (astrid-web
/// `lib/sync-cursor-age.ts`, AWTD-993). A stamp older than the window would ask for deletions
/// the server has already forgotten, and the rows would stay on screen with nothing raising
/// anything. The web's own incremental paths cap their cursor at the same twenty-four hours, so
/// a machine that was shut for a long weekend comes back with a full pull, as a browser does.
fn max_delta_age() -> chrono::Duration {
    chrono::Duration::hours(24)
}

/// How often a person's pass looks again for the slot while a timer pass holds it.
const SLOT_POLL: Duration = Duration::from_millis(50);

/// What one pass did. Returned rather than logged, so the shell can say "12 new tasks" and the
/// tests can assert on something.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub lists_added: usize,
    pub lists_updated: usize,
    /// Lists the server has tombstoned since the last pass, and which are gone from the cache.
    pub lists_deleted: usize,
    pub tasks_added: usize,
    pub tasks_updated: usize,
    /// Tasks the server has tombstoned since the last pass, and which are gone from the cache.
    pub tasks_deleted: usize,
    pub tasks_unchanged: usize,
    /// Local writes the push step could not deliver. The pass carried on regardless.
    pub pushes_failed: usize,
    /// Whether the fetch actually happened. False means the pass ran offline and changed nothing,
    /// which is a different thing from a pass that found nothing.
    pub fetched: bool,
    /// The pass never ran, because another was already running. For a timer tick that is the
    /// right answer; for a person it means the wait ran out — see [`policy`].
    pub skipped: bool,
    /// Whether the fetch asked only for what moved since the last stamp, rather than for
    /// everything.
    pub delta: bool,
    /// Every task the cache now holds differently — added, changed or removed — so whoever is
    /// drawing can refresh exactly what moved.
    pub changed_task_ids: Vec<String>,
    /// The same for lists.
    pub changed_list_ids: Vec<String>,
}

impl SyncReport {
    /// Whether the cache is different from before the pass. What decides if anybody is told.
    pub fn changed_anything(&self) -> bool {
        !self.changed_task_ids.is_empty() || !self.changed_list_ids.is_empty()
    }
}

/// How many rows a full pass asks about one by one before deleting them. More wait for the next.
const MAX_CONFIRMED_PER_PASS: usize = 50;

pub struct SyncManager {
    client: Arc<ApiClient>,
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
    runner: Arc<Runner>,
    /// Whether a pass is running. The timer skips while it is; a person waits for it.
    in_flight: AtomicBool,
}

/// Clears the in-flight flag however the pass ends, so a pass that panics does not leave every
/// later one believing it is still running.
struct InFlight<'a>(&'a AtomicBool);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
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
            in_flight: AtomicBool::new(false),
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

    /// One pass, because a person asked: push, fetch, apply.
    ///
    /// If a timer pass is already running this waits for it and then runs, rather than returning
    /// having done nothing — task 3173727d, where a refresh that landed during the sixty-second
    /// pass finished its animation having fetched nothing. The wait is bounded by
    /// [`policy::WAIT_FOR_SLOT_SECS`]; past it the report says `skipped`, which is honest, and
    /// the spinner stops.
    ///
    /// Never fails on being offline. A pass that cannot reach the server reports `fetched: false`
    /// and leaves the cache exactly as it was — which is the whole offline story, and why the
    /// return type is a report rather than an error.
    pub async fn sync(&self) -> SyncReport {
        self.sync_as(true).await
    }

    /// One pass, because the timer fired.
    ///
    /// A pass already in flight is doing this work, so a tick that finds one gives the slot up
    /// at once and says so.
    pub async fn sync_in_background(&self) -> SyncReport {
        self.sync_as(false).await
    }

    async fn sync_as(&self, user_initiated: bool) -> SyncReport {
        let asked_at = tokio::time::Instant::now();
        loop {
            match policy::admission(self.in_flight.load(Ordering::SeqCst), user_initiated) {
                policy::Admission::Start => {
                    if self
                        .in_flight
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        break;
                    }
                    // Somebody took the slot between the look and the claim. Ask again.
                }
                policy::Admission::Skip => {
                    return SyncReport {
                        skipped: true,
                        ..SyncReport::default()
                    };
                }
                policy::Admission::WaitForInFlight => {
                    if asked_at.elapsed() >= Duration::from_secs(policy::WAIT_FOR_SLOT_SECS) {
                        return SyncReport {
                            skipped: true,
                            ..SyncReport::default()
                        };
                    }
                    tokio::time::sleep(SLOT_POLL).await;
                }
            }
        }
        let _slot = InFlight(&self.in_flight);
        // Signing out: this pass would be writing the departing account's data.
        let Some(_pass) = self.store.session().enter() else {
            return SyncReport {
                skipped: true,
                ..SyncReport::default()
            };
        };
        self.run_pass().await
    }

    async fn run_pass(&self) -> SyncReport {
        let mut pushes_failed = 0;

        // Push first. Best-effort: a write that will not go is the Outbox's problem, and it keeps
        // trying on its own schedule.
        match self.runner.drain().await {
            Ok(drain) => pushes_failed = drain.dead_lettered,
            Err(_) => pushes_failed += 1,
        }

        // Stamped before the fetch, for the reason on `LAST_SYNC_KEY`. A first pass — or one
        // after a sign-out, which clears the cache — has no stamp and pulls everything.
        let started_at = self.clock.now();
        let since = self
            .last_sync()
            .filter(|stamp| started_at - *stamp <= max_delta_age())
            .map(|stamp| stamp - delta_overlap());

        match self.fetch_and_apply(since, started_at).await {
            Ok(mut report) => {
                report.pushes_failed = pushes_failed;
                report.fetched = true;
                report.delta = since.is_some();
                // The boards too. This was written and never called, so a column added or a
                // default renamed on the web never reached a board here (task e5214fba). A
                // server without boards answers 404, which is not a failed pass.
                if let Err(error) = self.sync_projects().await {
                    tracing::debug!(%error, "projects did not sync; the boards are as they were");
                }
                let _ = self
                    .store
                    .set_metadata(LAST_SYNC_KEY, &date::format(started_at));
                report
            }
            Err(error) => {
                tracing::warn!(%error, "sync could not fetch; the cache is unchanged");
                SyncReport {
                    pushes_failed,
                    ..SyncReport::default()
                }
            }
        }
    }

    /// Fetch and fold in. With `since`, only what moved after it — and what was deleted, which
    /// the server lists beside the rows so a client that never sees a deleted row can still stop
    /// showing it. A server too old to know `updatedSince` ignores it and answers with
    /// everything, which folds in exactly as a full pull would.
    async fn fetch_and_apply(
        &self,
        since: Option<DateTime<Utc>>,
        started_at: DateTime<Utc>,
    ) -> Result<SyncReport, ServiceError> {
        use crate::outbox::{journal, kind};
        let mut report = SyncReport::default();
        let since_param = since.map(date::format);
        // Rows deleted here whose delete is on its way — or has just landed, while a fetch that
        // began before it was in flight. The server still lists them; a pull must not bring them
        // back (the Apple apps' `recentlyDeletedIds`).
        let deleting_tasks =
            journal::ids_named(&self.store, Some(&[kind::DELETE_TASK]), "taskId", true)?;
        let deleting_lists =
            journal::ids_named(&self.store, Some(&[kind::DELETE_LIST]), "listId", true)?;

        let body = self
            .client
            .send(
                self.client
                    .get(endpoints::LISTS)
                    .query("updatedSince", since_param.clone()),
            )
            .await?;
        // `deletedIds` is what the lists route sends (`app/api/v1/lists/route.ts`); the older name
        // is read too. Read under the wrong name, a list deleted elsewhere stayed for a day.
        let mut deleted_lists = ids_named(&body, endpoints::envelope::DELETED_TASKS);
        deleted_lists.extend(ids_named(&body, endpoints::envelope::DELETED_LISTS));
        let lists =
            crate::model::lenient::<TaskList>(unwrap_envelope(body, endpoints::envelope::LISTS));
        report_skips("lists", &lists.skipped);
        for list in &lists.items {
            if deleting_lists.contains(&list.id) {
                continue;
            }
            match self.store.list(&list.id)? {
                Some(cached) => {
                    if is_newer(list.updated_at, cached.updated_at) {
                        // The collection never carries the default due time (the single-list
                        // read does); its absence here is not a cleared one.
                        let mut list = list.clone();
                        if list.default_due_time.is_none() {
                            list.default_due_time = cached.default_due_time.clone();
                        }
                        let list =
                            crate::services::members::with_pending_applied(&self.store, list);
                        self.store.upsert_list(&list)?;
                        report.lists_updated += 1;
                        report.changed_list_ids.push(list.id.clone());
                    }
                }
                None => {
                    self.store.upsert_list(list)?;
                    report.lists_added += 1;
                    report.changed_list_ids.push(list.id.clone());
                }
            }
        }
        for id in deleted_lists {
            if self.store.list(&id)?.is_some() {
                self.store.delete_list(&id)?;
                report.lists_deleted += 1;
                report.changed_list_ids.push(id);
            }
        }

        // Tasks come a page at a time, and the walk stops on a short page rather than on the
        // server's count — see `api::pagination` for why that count cannot be trusted.
        let (tasks, deleted_tasks, tasks_whole) = self.fetch_all_tasks(since_param).await?;
        // Tasks made here that the server has not answered for yet, by the idempotency key their
        // create carries — so a pull that brings one's server copy first replaces it rather than
        // showing it twice.
        let made_here: std::collections::HashMap<String, String> = self
            .store
            .tasks()?
            .into_iter()
            .filter(|task| crate::model::is_temp_id(&task.id))
            .filter_map(|task| task.client_request_id.clone().map(|key| (key, task.id)))
            .collect();
        for task in &tasks {
            if deleting_tasks.contains(&task.id) {
                continue;
            }
            if let Some(temp_id) = task
                .client_request_id
                .as_ref()
                .and_then(|key| made_here.get(key))
            {
                self.store.record_id_mapping(
                    temp_id,
                    &task.id,
                    task.updated_at.unwrap_or(started_at),
                )?;
                self.store.delete_task(temp_id)?;
                report.changed_task_ids.push(temp_id.clone());
            }
            match self.store.task(&task.id)? {
                Some(cached) => {
                    if is_newer(task.updated_at, cached.updated_at) {
                        // Both sides may have moved. The resolver decides field by field.
                        let resolved = conflict::resolve(&cached, task);
                        self.store.upsert_task(&resolved)?;
                        report.tasks_updated += 1;
                        report.changed_task_ids.push(task.id.clone());
                    } else {
                        report.tasks_unchanged += 1;
                    }
                }
                None => {
                    self.store.upsert_task(task)?;
                    report.tasks_added += 1;
                    report.changed_task_ids.push(task.id.clone());
                }
            }
        }
        for id in deleted_tasks {
            if self.store.task(&id)?.is_some() {
                self.store.delete_task(&id)?;
                report.tasks_deleted += 1;
                report.changed_task_ids.push(id);
            }
        }

        // A full pass carries no tombstones, so what the server no longer has is known only by
        // its absence. Left alone it would stay forever — a list deleted on the web while this
        // machine was away (Apple task 53071260). Absence is not deletion for a row this machine
        // made or changed and has not sent, nor for one that moved after the pass began.
        if since.is_none() {
            let unsent_tasks = journal::ids_named(&self.store, None, "taskId", false)?;
            let unsent_lists = journal::ids_named(&self.store, None, "listId", false)?;
            let fresh = |updated_at: Option<DateTime<Utc>>| {
                updated_at.is_some_and(|at| at > started_at - delta_overlap())
            };
            let seen_tasks: std::collections::HashSet<&str> =
                tasks.iter().map(|task| task.id.as_str()).collect();
            // A row that would not decode is still on the server; with one, the listing cannot say
            // what is missing, so nothing is taken on its word this pass.
            let absent_tasks: Vec<String> = if tasks_whole {
                self.store
                    .tasks()?
                    .into_iter()
                    .filter(|cached| {
                        !seen_tasks.contains(cached.id.as_str())
                            && !crate::model::is_temp_id(&cached.id)
                            && !unsent_tasks.contains(&cached.id)
                            && !fresh(cached.updated_at)
                    })
                    .map(|cached| cached.id)
                    .collect()
            } else {
                Vec::new()
            };
            // Absent from a paged listing is not proof: a row shifts pages when another is deleted
            // mid-walk, and a feed can leave out what the account can still open. Each one is asked
            // about on its own and only a server that says it is gone deletes it — a bounded number
            // per pass, the rest on the next.
            for id in absent_tasks.into_iter().take(MAX_CONFIRMED_PER_PASS) {
                if self.confirmed_gone(endpoints::task(&id)).await {
                    self.store.delete_task(&id)?;
                    report.tasks_deleted += 1;
                    report.changed_task_ids.push(id);
                }
            }
            let seen_lists: std::collections::HashSet<&str> =
                lists.items.iter().map(|list| list.id.as_str()).collect();
            let absent_lists: Vec<String> = if lists.skipped.is_empty() {
                self.store
                    .lists()?
                    .into_iter()
                    .filter(|cached| {
                        !seen_lists.contains(cached.id.as_str())
                            && !crate::model::is_temp_id(&cached.id)
                            && !unsent_lists.contains(&cached.id)
                            && !cached.is_virtual.unwrap_or(false)
                            && !fresh(cached.updated_at)
                    })
                    .map(|cached| cached.id)
                    .collect()
            } else {
                Vec::new()
            };
            for id in absent_lists.into_iter().take(MAX_CONFIRMED_PER_PASS) {
                if self.confirmed_gone(endpoints::list(&id)).await {
                    self.store.delete_list(&id)?;
                    report.lists_deleted += 1;
                    report.changed_list_ids.push(id);
                }
            }
        }

        Ok(report)
    }

    /// Whether the server says this row is gone — not found, or no longer this account's. Any other
    /// answer, a network failure included, keeps it.
    async fn confirmed_gone(&self, path: String) -> bool {
        matches!(
            self.client.send(self.client.get(path)).await,
            Err(ApiError::Http {
                status: 403 | 404 | 410,
                ..
            })
        )
    }

    /// Every task page, and every tombstone the pages carried.
    ///
    /// The tombstones are gathered across pages rather than read off the first: losing them on
    /// page two would bring a deleted task back on the machines with the most tasks, which are
    /// the machines whose owners would notice least.
    async fn fetch_all_tasks(
        &self,
        since: Option<String>,
    ) -> Result<(Vec<Task>, Vec<String>, bool), ApiError> {
        const PAGE: usize = 1000;
        let deleted = Mutex::new(Vec::new());
        let whole = std::sync::atomic::AtomicBool::new(true);
        let tasks = crate::api::fetch_all(PAGE, |limit, offset| {
            let since = since.clone();
            let deleted = &deleted;
            let whole = &whole;
            async move {
                let request = self
                    .client
                    .get(endpoints::TASKS)
                    .query("limit", Some(limit.to_string()))
                    .query("offset", Some(offset.to_string()))
                    // The per-task embedded member arrays are large and redundant: permission
                    // checks resolve membership from the list, not from a copy stapled to every
                    // task. Older servers ignore the parameter and send them anyway.
                    .query("leanListMembers", Some("1".to_string()))
                    .query("updatedSince", since);
                let body = self.client.send(request).await?;
                deleted
                    .lock()
                    .expect("tombstone lock")
                    .extend(ids_named(&body, endpoints::envelope::DELETED_TASKS));
                let page = crate::model::lenient::<Task>(unwrap_envelope(
                    body,
                    endpoints::envelope::TASKS,
                ));
                report_skips("tasks", &page.skipped);
                if !page.skipped.is_empty() {
                    whole.store(false, std::sync::atomic::Ordering::SeqCst);
                }
                Ok::<crate::api::Page<Task>, ApiError>(crate::api::Page {
                    items: page.into_items(),
                    total: None,
                })
            }
        })
        .await?;
        let deleted = deleted.into_inner().expect("tombstone lock");
        Ok((tasks, deleted, whole.into_inner()))
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

/// The rows under `key`, or the whole body when the key is absent. Both shapes have been seen on
/// the same route — see [`ApiClient::send_collection`], which this mirrors so the delta fields
/// beside the rows can be read first.
fn unwrap_envelope(body: serde_json::Value, key: &str) -> serde_json::Value {
    match body.get(key) {
        Some(rows) => rows.clone(),
        None => body,
    }
}

/// The string ids under `key`, or nothing. A tombstone that is not a string is not a tombstone.
fn ids_named(body: &serde_json::Value, key: &str) -> Vec<String> {
    body.get(key)
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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

    fn task(id: &str, updated_at: &str) -> Task {
        serde_json::from_value(json!({ "id": id, "title": id, "updatedAt": updated_at }))
            .expect("a task")
    }

    /// Journal a write that is waiting out a backoff, so the pass's drain leaves it unsent.
    fn journal(store: &Store, kind: &str, payload: serde_json::Value, key: &str) {
        let now = date::parse("2026-09-07T12:00:00Z").expect("an instant");
        let entry = crate::outbox::Entry::new(format!("entry-{key}"), kind, payload, key, now);
        let entry = match key.starts_with("temp_") {
            true => entry.for_temp_id(key),
            false => entry,
        };
        crate::outbox::journal::enqueue(store, &entry).expect("enqueues");
        crate::outbox::journal::mark_retry(
            store,
            &entry.id,
            1,
            date::parse("2026-09-07T13:00:00Z").expect("an instant"),
            "later",
            now,
        )
        .expect("marks");
    }

    /// Deleted here, and the delete has not reached the server yet: the server still has the task,
    /// and a pull that brings it back would put a task the person just deleted back on screen until
    /// the delete lands (the Apple apps' `recentlyDeletedIds`, which this crate had no equivalent
    /// of).
    #[tokio::test]
    async fn a_pull_does_not_bring_back_a_task_deleted_here() {
        let fixture = fixture(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                // ...and the server still has the task.
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [{ "id": "t1", "title": "Deleted here", "updatedAt": "2026-09-07T11:00:00Z" }] }),
                ),
        );
        journal(
            &fixture.store,
            crate::outbox::kind::DELETE_TASK,
            json!({ "taskId": "t1" }),
            "delete-t1",
        );

        let report = fixture.sync.sync().await;
        assert!(report.fetched);
        assert!(
            fixture.store.task("t1").expect("reads").is_none(),
            "a task deleted here stays deleted while its delete is on its way"
        );
    }

    /// The pull can bring the server's copy of a task this machine made while the create's answer
    /// is still on its way. The copy is that task, not a second one: it takes the temporary row's
    /// place, and the temporary id keeps pointing at it.
    #[tokio::test]
    async fn a_pull_that_brings_a_task_made_here_replaces_its_temporary_copy() {
        let fixture = fixture(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [{ "id": "real-a", "title": "Made here",
                                        "clientRequestId": "temp_a",
                                        "updatedAt": "2026-09-07T11:59:00Z" }] }),
                ),
        );
        let mut made_here = task("temp_a", "2026-09-07T11:58:00Z");
        made_here.client_request_id = Some("temp_a".into());
        fixture.store.upsert_task(&made_here).expect("stores");
        journal(
            &fixture.store,
            crate::outbox::kind::CREATE_TASK,
            json!({ "body": { "title": "Made here" } }),
            "temp_a",
        );

        fixture.sync.sync().await;
        assert!(
            fixture.store.task("temp_a").expect("reads").is_none(),
            "no duplicate"
        );
        assert!(fixture.store.task("real-a").expect("reads").is_some());
        assert_eq!(fixture.store.resolve_id("temp_a").expect("reads"), "real-a");
    }

    /// A full pass — the first, or one after more than a day away — gets no tombstones, so rows the
    /// server no longer has are recognised by their absence. Left alone they would stay forever: a
    /// list deleted on the web while this machine was away (Apple task 53071260).
    ///
    /// Absence is not always deletion. A row this machine made or changed and has not sent, and one
    /// that moved since the pass began (a create confirmed by the delivery loop while the fetch was
    /// in flight — Apple task f07dff56), are kept.
    #[tokio::test]
    async fn a_full_pass_forgets_what_the_server_no_longer_has_and_nothing_else() {
        let fixture = fixture(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] }))
                .push_json("/api/v1/tasks/gone", 404, json!({ "error": "not found" }))
                .push_json(
                    "/api/v1/lists/gone-list",
                    404,
                    json!({ "error": "not found" }),
                ),
        );
        let store = &fixture.store;
        store
            .upsert_task(&task("gone", "2026-09-01T00:00:00Z"))
            .expect("stores");
        store
            .upsert_task(&task("temp_new", "2026-09-01T00:00:00Z"))
            .expect("stores");
        store
            .upsert_task(&task("edited", "2026-09-01T00:00:00Z"))
            .expect("stores");
        store
            .upsert_task(&task("just-confirmed", "2026-09-07T12:00:30Z"))
            .expect("stores");
        store
            .upsert_list(&TaskList::new("gone-list", "Deleted on the web"))
            .expect("stores");
        journal(
            store,
            crate::outbox::kind::UPDATE_TASK,
            json!({ "taskId": "edited", "body": { "title": "Mine" } }),
            "edit-1",
        );

        let report = fixture.sync.sync().await;
        assert!(!report.delta, "no stamp: a full pass");
        assert!(
            store.task("gone").expect("reads").is_none(),
            "deleted elsewhere"
        );
        assert!(
            store.list("gone-list").expect("reads").is_none(),
            "deleted elsewhere"
        );
        assert!(
            store.task("temp_new").expect("reads").is_some(),
            "never sent"
        );
        assert!(
            store.task("edited").expect("reads").is_some(),
            "an edit still on its way"
        );
        assert!(
            store.task("just-confirmed").expect("reads").is_some(),
            "moved after the pass began"
        );
    }

    fn requests_to(transport: &StubTransport, path: &str) -> Vec<String> {
        transport
            .requests()
            .iter()
            .filter(|request| request.url.contains(path))
            .map(|request| request.url.clone())
            .collect()
    }

    /// The first pass has nothing to go on and asks for everything; the second asks only for
    /// what moved since the first — from a little before the stamp, because the stamp is this
    /// machine's clock and `updatedAt` is the server's.
    #[tokio::test]
    async fn a_second_pass_asks_only_for_what_moved_since_the_first() {
        let transport = Arc::new(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] }))
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );
        let fixture = {
            let store = Arc::new(Store::in_memory().expect("opens"));
            let clock = Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z"));
            let client = Arc::new(ApiClient::new(
                "https://astrid.cc",
                transport.clone(),
                Arc::new(MemorySecureStore::new()),
            ));
            let runner = Arc::new(Runner::new(client.clone(), store.clone(), clock.clone()));
            SyncManager::new(client, store, clock, runner)
        };

        let first = fixture.sync().await;
        assert!(!first.delta, "nothing to be incremental from yet");
        let full = requests_to(&transport, "/api/v1/tasks");
        assert!(
            !full[0].contains("updatedSince"),
            "the first pass asks for everything: {}",
            full[0]
        );

        let second = fixture.sync().await;
        assert!(second.delta);
        let urls = requests_to(&transport, "/api/v1/tasks");
        let delta = &urls[1];
        assert!(
            delta.contains("updatedSince=2026-09-07T11%3A55%3A00Z")
                || delta.contains("updatedSince=2026-09-07T11:55:00Z"),
            "five minutes before the stamp, not the stamp itself: {delta}"
        );
        let lists = requests_to(&transport, "/api/v1/lists");
        assert!(
            lists[1].contains("updatedSince="),
            "lists too: {}",
            lists[1]
        );
    }

    /// A stamp older than the server keeps tombstones for is no use as a cursor (AWTD-993): the
    /// deletions it would ask about have been pruned, and the deleted rows would stay on screen
    /// forever. So a machine that was shut for a long weekend pulls everything, as the web does
    /// after the same twenty-four hours.
    #[tokio::test]
    async fn a_stamp_older_than_a_day_asks_for_everything_again() {
        let fixture = fixture(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] }))
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );
        // A stamp from a day and a minute before the clock: just past the window.
        fixture
            .store
            .set_metadata(LAST_SYNC_KEY, "2026-09-06T11:59:00Z")
            .expect("stamps");

        let report = fixture.sync.sync().await;
        assert!(!report.delta, "a stale stamp is not a cursor");

        // Inside the window the same machine asks for a delta, as before.
        fixture
            .store
            .set_metadata(LAST_SYNC_KEY, "2026-09-06T12:30:00Z")
            .expect("stamps");
        let report = fixture.sync.sync().await;
        assert!(
            report.delta,
            "twenty-three and a half hours is still a cursor"
        );
    }

    /// A delta fetch never sees a deleted row, so the server lists the ids beside the rows. A
    /// client that ignored them would keep showing tasks that are gone — forever, since nothing
    /// later would mention them either.
    #[tokio::test]
    async fn a_delta_pass_removes_what_the_server_has_deleted() {
        let fixture = fixture(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [], "deletedIds": ["l1", "never-had-it"] }),
                )
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [], "deletedIds": ["t1"] }),
                ),
        );
        fixture
            .store
            .set_metadata(LAST_SYNC_KEY, "2026-09-07T11:00:00Z")
            .expect("stamps");
        fixture
            .store
            .upsert_list(&TaskList::new("l1", "Home"))
            .expect("stores");
        fixture
            .store
            .upsert_task(&Task::new("t1", "Gone elsewhere"))
            .expect("stores");
        fixture
            .store
            .upsert_task(&Task::new("t2", "Still here"))
            .expect("stores");

        let report = fixture.sync.sync().await;

        assert_eq!(report.tasks_deleted, 1);
        assert_eq!(
            report.lists_deleted, 1,
            "an id never cached is not a deletion here"
        );
        assert!(fixture.store.task("t1").expect("reads").is_none());
        assert!(fixture.store.task("t2").expect("reads").is_some());
        assert!(fixture.store.list("l1").expect("reads").is_none());
        assert_eq!(report.changed_task_ids, vec!["t1".to_string()]);
        assert_eq!(report.changed_list_ids, vec!["l1".to_string()]);
        assert!(report.changed_anything());
    }

    /// The stamp is the instant the pass started, not the instant it finished. An edit made on
    /// the web while the fetch was in flight sits between the two, and stamping the end would
    /// skip it on every later pass.
    #[tokio::test]
    async fn the_stamp_is_when_the_pass_started() {
        let fixture = fixture(empty_task_pages(StubTransport::new().push_json(
            "/api/v1/lists",
            200,
            json!({ "lists": [] }),
        )));
        fixture.sync.sync().await;
        assert_eq!(
            fixture.sync.last_sync().map(date::format).as_deref(),
            Some("2026-09-07T12:00:00Z")
        );
    }

    /// The report names what moved, so whoever is drawing can refresh exactly that.
    #[tokio::test]
    async fn the_report_names_every_row_the_cache_now_holds_differently() {
        let fixture = fixture(empty_task_pages(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [{ "id": "l1", "name": "Home" }] }),
                )
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [
                        { "id": "t1", "title": "New" },
                        { "id": "t2", "title": "Unchanged" }
                    ] }),
                ),
        ));
        let mut cached = Task::new("t2", "Unchanged");
        cached.updated_at = date::parse("2026-09-07T12:00:00Z");
        fixture.store.upsert_task(&cached).expect("stores");

        let report = fixture.sync.sync().await;

        assert_eq!(report.changed_task_ids, vec!["t1".to_string()]);
        assert_eq!(report.changed_list_ids, vec!["l1".to_string()]);
    }

    /// A pass that found nothing new has nothing to announce.
    #[tokio::test]
    async fn a_pass_that_found_nothing_changed_nothing() {
        let fixture = fixture(empty_task_pages(StubTransport::new().push_json(
            "/api/v1/lists",
            200,
            json!({ "lists": [] }),
        )));
        let report = fixture.sync.sync().await;
        assert!(report.fetched);
        assert!(!report.changed_anything());
    }

    /// The timer half of task 3173727d: a tick that lands while a pass is running gives the
    /// slot up at once. The pass in flight is doing this work.
    #[tokio::test(start_paused = true)]
    async fn a_timer_tick_during_a_pass_is_skipped() {
        let transport = Arc::new(StubTransport::new());
        let fixture = {
            let store = Arc::new(Store::in_memory().expect("opens"));
            let clock = Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z"));
            let client = Arc::new(ApiClient::new(
                "https://astrid.cc",
                transport.clone(),
                Arc::new(MemorySecureStore::new()),
            ));
            let runner = Arc::new(Runner::new(client.clone(), store.clone(), clock.clone()));
            SyncManager::new(client, store, clock, runner)
        };
        fixture.in_flight.store(true, Ordering::SeqCst);

        let report = fixture.sync_in_background().await;

        assert!(report.skipped);
        assert!(!report.fetched);
        assert!(
            transport.requests().is_empty(),
            "a skipped tick sends nothing"
        );
    }

    /// The person half of task 3173727d: a refresh somebody asked for waits for the pass in
    /// flight and then runs, rather than returning having fetched nothing while the spinner
    /// says otherwise.
    #[tokio::test(start_paused = true)]
    async fn a_person_waits_for_the_pass_in_flight_and_then_runs() {
        let fixture = fixture(empty_task_pages(StubTransport::new().push_json(
            "/api/v1/lists",
            200,
            json!({ "lists": [] }),
        )));
        let sync = Arc::new(fixture.sync);
        sync.in_flight.store(true, Ordering::SeqCst);

        let holder = sync.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            holder.in_flight.store(false, Ordering::SeqCst);
        });

        let report = sync.sync().await;

        assert!(!report.skipped, "the wait has to end in a pass");
        assert!(report.fetched);
        assert!(
            !sync.in_flight.load(Ordering::SeqCst),
            "the slot is given back afterwards"
        );
    }

    /// Bounded: a refresh that cannot get in has to stop rather than leave the spinner turning.
    #[tokio::test(start_paused = true)]
    async fn a_person_who_cannot_get_the_slot_stops_rather_than_spinning() {
        let fixture = fixture(StubTransport::new());
        fixture.sync.in_flight.store(true, Ordering::SeqCst);

        let report = fixture.sync.sync().await;

        assert!(report.skipped);
        assert!(!report.fetched);
    }

    /// Absent from a paged listing is not proof a task is gone: one the server still answers for
    /// is kept, and a listing with a row that would not decode prunes nothing at all.
    #[tokio::test]
    async fn a_full_pass_deletes_only_what_the_server_confirms_is_gone() {
        let fixture = fixture(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] }))
                .push_json(
                    "/api/v1/tasks/still-there",
                    200,
                    json!({ "task": { "id": "still-there" } }),
                ),
        );
        fixture
            .store
            .upsert_task(&task("still-there", "2026-09-01T00:00:00Z"))
            .expect("stores");
        fixture.sync.sync().await;
        assert!(
            fixture.store.task("still-there").expect("reads").is_some(),
            "the server still has it"
        );
    }

    #[tokio::test]
    async fn a_listing_with_an_unreadable_row_prunes_nothing() {
        let transport = StubTransport::new()
            .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
            .push_json("/api/v1/tasks", 200, json!({ "tasks": [{ "id": 7 }] }));
        let sent = transport.recorded.clone();
        let fixture = fixture(transport);
        fixture
            .store
            .upsert_task(&task("maybe", "2026-09-01T00:00:00Z"))
            .expect("stores");
        fixture.sync.sync().await;
        assert!(fixture.store.task("maybe").expect("reads").is_some());
        assert!(
            !sent
                .lock()
                .expect("lock")
                .iter()
                .any(|request| request.url.contains("/api/v1/tasks/maybe")),
            "not even asked about"
        );
    }

    /// The lists route names its deletions `deletedIds`. Read as `deletedListIds`, a list deleted
    /// elsewhere stayed in the sidebar until the next full pass.
    #[tokio::test]
    async fn a_list_deleted_elsewhere_goes_on_a_delta_pass() {
        let fixture = fixture(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] }))
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [], "deletedIds": ["l-gone"] }),
                )
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );
        fixture.sync.sync().await;
        fixture
            .store
            .upsert_list(&TaskList::new("l-gone", "Deleted on the web"))
            .expect("stores");
        let second = fixture.sync.sync().await;
        assert!(second.delta, "{second:?}");
        assert!(fixture.store.list("l-gone").expect("reads").is_none());
    }

    /// The lists collection never carries a list's default due time; a pass must not clear it.
    #[tokio::test]
    async fn a_pass_keeps_a_lists_default_due_time() {
        let fixture = fixture(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [
                    { "id": "l1", "name": "Work", "updatedAt": "2026-09-07T12:30:00Z" }
                ] }),
                )
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );
        let cached: TaskList = serde_json::from_value(json!({
            "id": "l1", "name": "Work", "defaultDueTime": "09:00", "updatedAt": "2026-09-01T00:00:00Z"
        }))
        .expect("a list");
        fixture.store.upsert_list(&cached).expect("stores");
        fixture.sync.sync().await;
        let list = fixture.store.list("l1").expect("reads").expect("kept");
        assert_eq!(list.default_due_time.as_deref(), Some("09:00"));
    }
}
