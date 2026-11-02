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
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::api::{endpoints, ApiClient, ApiError};
use crate::model::{date, Project, Task, TaskList};
use crate::outbox::Runner;
use crate::platform::Clock;
use crate::services::ServiceError;
use crate::store::{self, Store};

/// Where the last successful pass got to.
///
/// The instant the pass *started*, not the instant it finished: an edit made on the web while
/// the fetch was in flight has an `updatedAt` between the two, and stamping the end would skip
/// it forever.
pub(crate) const LAST_SYNC_KEY: &str = "sync.last-completed";

/// The *server's* clock at the start of the last successful pass, verbatim as it sent it.
///
/// This is what a delta pass asks from, and [`LAST_SYNC_KEY`] is only how this machine judges
/// how long it has been away. The server reads it before it runs the query (astrid-web
/// `app/api/v1/tasks/route.ts`, AWTD-1470), so a write that lands while the query runs is newer
/// than the stamp and arrives next pass rather than being skipped.
///
/// Kept as the string the server sent rather than a parsed instant: it goes straight back as
/// `updatedSince`, and a reformat is a chance to lose precision for no gain.
///
/// Empty, or absent, means the last pass talked to a deployment too old to send one — see
/// [`delta_overlap`].
pub(crate) const LAST_SYNC_SERVER_KEY: &str = "sync.last-completed.server";

/// How far behind a *local* stamp a delta pass asks from.
///
/// Only for a deployment that sends no `serverTime`. Then the stamp is this machine's clock and
/// `updatedAt` is the server's, and the two disagree by however wrong this machine's clock is.
/// Asking for a little more than strictly necessary costs a few rows that [`is_newer`] then
/// declines to apply; asking for a little less loses an edit silently. Five minutes covers any
/// clock a machine that can still sign in is likely to have — which is the whole weakness of it,
/// and why a server-issued stamp needs no margin at all: a clock two hours fast skipped two hours
/// of other people's edits, silently, with nothing to raise (task 86109567).
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
    /// What the push step delivered (or refused for good), announced separately from what the
    /// pull brought in (AITD-454). Before this it was announced by nobody: a create sent by a
    /// pass's push swapped its optimistic row for the real one without a shell hearing which.
    /// `None` when the push step delivered nothing.
    pub delivered: Option<crate::outbox::Delivery>,
}

impl SyncReport {
    /// Whether the cache is different from before the pass. What decides if anybody is told.
    pub fn changed_anything(&self) -> bool {
        !self.changed_task_ids.is_empty() || !self.changed_list_ids.is_empty()
    }

    /// Fold one phase's counts into the pass's.
    ///
    /// Each apply phase runs on a blocking thread (task `bb20ca56`) and so cannot hold a borrow of
    /// the pass's report; it fills its own and the pass absorbs it. Only the fields an apply sets
    /// are folded — `fetched`, `skipped`, `delta` and `delivered` are the pass's own answer about
    /// itself, not any one phase's.
    fn absorb(&mut self, phase: SyncReport) {
        self.lists_added += phase.lists_added;
        self.lists_updated += phase.lists_updated;
        self.lists_deleted += phase.lists_deleted;
        self.tasks_added += phase.tasks_added;
        self.tasks_updated += phase.tasks_updated;
        self.tasks_deleted += phase.tasks_deleted;
        self.tasks_unchanged += phase.tasks_unchanged;
        self.changed_task_ids.extend(phase.changed_task_ids);
        self.changed_list_ids.extend(phase.changed_list_ids);
    }
}

/// Run one apply phase on a blocking thread, off the tokio worker.
///
/// Task `bb20ca56`: an apply spans every row of every fetched page inside one transaction
/// (AITD-454), which is the one piece of store work long enough to matter — a read is
/// sub-millisecond and has its own connection, so neither is worth a thread hop. See the
/// `store` module header for why the split falls here and not inside `Store`.
///
/// A `JoinError` means the apply panicked while it held the connection, which is exactly the state
/// [`crate::store::StoreError::Lock`] describes: the mutex is poisoned and the cache cannot be
/// trusted. It is reported as that rather than re-panicked, because this unwinds through an FFI
/// call.
async fn applying<F>(work: F) -> Result<SyncReport, ServiceError>
where
    F: FnOnce() -> crate::store::Result<SyncReport> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(applied) => Ok(applied?),
        Err(_) => Err(crate::store::StoreError::Lock.into()),
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

    /// The server's own stamp from the last successful pass, if that server sent one.
    fn last_sync_server(&self) -> Option<String> {
        self.store
            .metadata(LAST_SYNC_SERVER_KEY)
            .ok()
            .flatten()
            .filter(|stamp| !stamp.is_empty())
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
        let mut delivered = None;
        match self.runner.drain().await {
            Ok(drain) => {
                pushes_failed = drain.dead_lettered;
                if drain.completed > 0 || drain.dead_lettered > 0 {
                    delivered = Some(drain.delivered);
                }
            }
            Err(_) => pushes_failed += 1,
        }

        // Stamped before the fetch, for the reason on `LAST_SYNC_KEY`. A first pass — or one
        // after a sign-out, which clears the cache — has no stamp and pulls everything.
        let started_at = self.clock.now();
        // How long this machine has been away is a question about THIS machine, so the local
        // stamp answers it — a server stamp compared against a local `now` would be exactly the
        // skew this fix removes. What to ask the server FROM is the server's own stamp, sent back
        // verbatim and with no margin; only a deployment that never sent one falls back to the
        // local clock minus [`delta_overlap`].
        let within_delta_age = self
            .last_sync()
            .is_some_and(|stamp| started_at - stamp <= max_delta_age());
        let since = if within_delta_age {
            self.last_sync_server().or_else(|| {
                self.last_sync()
                    .map(|stamp| date::format(stamp - delta_overlap()))
            })
        } else {
            None
        };

        match self.fetch_and_apply(since.clone(), started_at).await {
            Ok((mut report, server_time)) => {
                report.pushes_failed = pushes_failed;
                report.delivered = delivered;
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
                // Written every pass, including as an empty string when the server sent none, so
                // a device that talks to a current deployment and then to an older one falls back
                // rather than replaying a stamp that server never issued.
                let _ = self.store.set_metadata(
                    LAST_SYNC_SERVER_KEY,
                    server_time.as_deref().unwrap_or_default(),
                );
                report
            }
            Err(error) => {
                tracing::warn!(%error, "sync could not fetch; the cache is unchanged");
                SyncReport {
                    pushes_failed,
                    delivered,
                    ..SyncReport::default()
                }
            }
        }
    }

    /// Fetch and fold in. With `since`, only what moved after it — and what was deleted, which
    /// the server lists beside the rows so a client that never sees a deleted row can still stop
    /// showing it. A server too old to know `updatedSince` ignores it and answers with
    /// everything, which folds in exactly as a full pull would.
    /// Also answers with the server's own stamp for this pass, which becomes the next pass's
    /// cursor. `None` from a deployment that does not send one.
    async fn fetch_and_apply(
        &self,
        since_param: Option<String>,
        started_at: DateTime<Utc>,
    ) -> Result<(SyncReport, Option<String>), ServiceError> {
        use crate::outbox::{journal, kind};
        let mut report = SyncReport::default();
        let since = since_param.clone();
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
        // The pass's cursor comes from the FIRST request it made, not the last: the lists query ran
        // before the tasks query, so the tasks response's stamp is later, and storing that one
        // would skip a list edited in between. The earliest stamp of the pass costs a few rows
        // `is_newer` declines next pass; the latest loses an edit.
        let mut server_time = string_named(&body, endpoints::envelope::SERVER_TIME);
        // `deletedIds` is what the lists route sends (`app/api/v1/lists/route.ts`); the older name
        // is read too. Read under the wrong name, a list deleted elsewhere stayed for a day.
        let mut deleted_lists = ids_named(&body, endpoints::envelope::DELETED_TASKS);
        deleted_lists.extend(ids_named(&body, endpoints::envelope::DELETED_LISTS));
        let lists = Arc::new(crate::model::lenient::<TaskList>(unwrap_envelope(
            body,
            endpoints::envelope::LISTS,
        )));
        report_skips("lists", &lists.skipped);
        // One transaction for the lists, with nothing awaited inside it: the lock is never held
        // across the network (AITD-454). Row by row, each statement committed on its own, and a
        // list being drawn could see half a pass.
        //
        // On a blocking thread, because it spans every fetched row (task bb20ca56). The rows are
        // shared by `Arc` rather than moved: the absence pruning below still needs them.
        let applied = applying({
            let store = self.store.clone();
            let lists = lists.clone();
            let deleting_lists = deleting_lists.clone();
            move || {
                let mut report = SyncReport::default();
                store.transaction(|connection| {
                    // Read once, and only when a list is to be updated — the overlay of membership
                    // changes this device has not sent yet.
                    let mut journal: Option<Vec<crate::outbox::Entry>> = None;
                    for list in &lists.items {
                        if deleting_lists.contains(&list.id) {
                            continue;
                        }
                        match store::list_in(connection, &list.id)? {
                            Some(cached) => {
                                if is_newer(list.updated_at, cached.updated_at) {
                                    // The collection never carries the default due time (the
                                    // single-list read does); its absence here is not a cleared one.
                                    let mut list = list.clone();
                                    if list.default_due_time.is_none() {
                                        list.default_due_time = cached.default_due_time.clone();
                                    }
                                    if journal.is_none() {
                                        journal = Some(journal::all_in(connection)?);
                                    }
                                    let list = crate::services::members::with_pending_from(
                                        journal.as_deref().unwrap_or_default(),
                                        list,
                                    );
                                    store::upsert_list_in(connection, &list)?;
                                    report.lists_updated += 1;
                                    report.changed_list_ids.push(list.id.clone());
                                }
                            }
                            None => {
                                store::upsert_list_in(connection, list)?;
                                report.lists_added += 1;
                                report.changed_list_ids.push(list.id.clone());
                            }
                        }
                    }
                    for id in deleted_lists {
                        if store::list_in(connection, &id)?.is_some() {
                            store::delete_list_in(connection, &id)?;
                            report.lists_deleted += 1;
                            report.changed_list_ids.push(id);
                        }
                    }
                    Ok(())
                })?;
                Ok(report)
            }
        })
        .await?;
        report.absorb(applied);

        // Tasks come a page at a time, and the walk stops on a short page rather than on the
        // server's count — see `api::pagination` for why that count cannot be trusted.
        let fetched = self.fetch_all_tasks(since_param).await?;
        let (deleted_tasks, tasks_whole) = (fetched.deleted, fetched.whole);
        // Only if the lists route sent none — see the note there on why the earliest wins.
        if server_time.is_none() {
            server_time = fetched.server_time;
        }
        let tasks = Arc::new(fetched.tasks);
        // The tasks, likewise in one transaction once every page is in hand — and likewise on a
        // blocking thread, which for the task phase is the one that matters: this is the apply a
        // thousand-row first pass spends its time in (task bb20ca56).
        let applied = applying({
            let store = self.store.clone();
            let tasks = tasks.clone();
            let deleting_tasks = deleting_tasks.clone();
            move || {
                let mut report = SyncReport::default();
                store.transaction(|connection| {
                    // Tasks made here that the server has not answered for yet, by the idempotency
                    // key their create carries — so a pull that brings one's server copy first
                    // replaces it rather than showing it twice.
                    let made_here: std::collections::HashMap<String, String> =
                        store::temp_tasks_in(connection)?
                            .into_iter()
                            .filter_map(|task| {
                                task.client_request_id.clone().map(|key| (key, task.id))
                            })
                            .collect();
                    for task in tasks.iter() {
                        if deleting_tasks.contains(&task.id) {
                            continue;
                        }
                        if let Some(temp_id) = task
                            .client_request_id
                            .as_ref()
                            .and_then(|key| made_here.get(key))
                        {
                            store::record_id_mapping_in(
                                connection,
                                temp_id,
                                &task.id,
                                task.updated_at.unwrap_or(started_at),
                            )?;
                            store::delete_task_in(connection, temp_id)?;
                            report.changed_task_ids.push(temp_id.clone());
                        }
                        match store::task_in(connection, &task.id)? {
                            Some(cached) => {
                                if is_newer(task.updated_at, cached.updated_at) {
                                    // Both sides may have moved. The resolver decides field by
                                    // field.
                                    let resolved = conflict::resolve(&cached, task);
                                    store::upsert_task_in(connection, &resolved)?;
                                    report.tasks_updated += 1;
                                    report.changed_task_ids.push(task.id.clone());
                                } else {
                                    report.tasks_unchanged += 1;
                                }
                            }
                            None => {
                                store::upsert_task_in(connection, task)?;
                                report.tasks_added += 1;
                                report.changed_task_ids.push(task.id.clone());
                            }
                        }
                    }
                    for id in deleted_tasks {
                        if store::task_in(connection, &id)?.is_some() {
                            store::delete_task_in(connection, &id)?;
                            report.tasks_deleted += 1;
                            report.changed_task_ids.push(id);
                        }
                    }
                    Ok(())
                })?;
                Ok(report)
            }
        })
        .await?;
        report.absorb(applied);

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

        Ok((report, server_time))
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
    async fn fetch_all_tasks(&self, since: Option<String>) -> Result<TaskFetch, ApiError> {
        const PAGE: usize = 1000;
        let mut fetched = TaskFetch {
            tasks: Vec::new(),
            deleted: Vec::new(),
            whole: true,
            server_time: None,
        };
        // An empty `cursor` starts a keyset walk; `None` means offset paging, which is where a
        // deployment too old to understand `cursor` ends up (below).
        let mut cursor: Option<String> = Some(String::new());
        let mut offset = 0usize;
        loop {
            let mut request = self
                .client
                .get(endpoints::TASKS)
                .query("limit", Some(PAGE.to_string()))
                // The per-task embedded member arrays are large and redundant: permission
                // checks resolve membership from the list, not from a copy stapled to every
                // task. Older servers ignore the parameter and send them anyway.
                .query("leanListMembers", Some("1".to_string()))
                .query("updatedSince", since.clone());
            request = match &cursor {
                Some(value) => request.query("cursor", Some(value.clone())),
                None => request.query("offset", Some(offset.to_string())),
            };
            let body = self.client.send(request).await?;
            // Nothing else is asked for: the replica this fills is what the detail view
            // renders offline, so a page trimmed of anything more is refused rather than
            // stored (AITD-499).
            require_only_omitted(&body, &OMITTED_BY_REQUEST)?;
            if fetched.server_time.is_none() {
                fetched.server_time = string_named(&body, endpoints::envelope::SERVER_TIME);
            }
            // Absent on the last page, and on every offset page. Opaque — never parsed.
            let next = string_named(&body, endpoints::envelope::NEXT_CURSOR);
            fetched
                .deleted
                .extend(ids_named(&body, endpoints::envelope::DELETED_TASKS));
            let page =
                crate::model::lenient::<Task>(unwrap_envelope(body, endpoints::envelope::TASKS));
            report_skips("tasks", &page.skipped);
            if !page.skipped.is_empty() {
                fetched.whole = false;
            }
            let items = page.into_items();
            let received = items.len();
            fetched.tasks.extend(items);

            if cursor.is_some() {
                if let Some(next) = next {
                    cursor = Some(next);
                    continue;
                }
                // No `nextCursor`. On a deployment that sends `serverTime` the two shipped
                // together (AWTD-1470), so its absence is the end of the walk and nothing else.
                if fetched.server_time.is_some() {
                    break;
                }
                // Otherwise `cursor` was ignored and this was offset page zero under the default
                // order. Stopping here would silently truncate a library at one page, so fall
                // back to the walk this client has always done.
                if received < PAGE {
                    break;
                }
                cursor = None;
                offset = PAGE;
                continue;
            }
            // Offset paging stops on a short page rather than on the server's count — see
            // `api::pagination` for why that count cannot be trusted.
            if received < PAGE {
                break;
            }
            offset += PAGE;
        }
        Ok(fetched)
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
/// What the task walk came back with.
struct TaskFetch {
    tasks: Vec<Task>,
    /// Gathered across pages rather than read off the first: losing them on page two would bring
    /// a deleted task back on the machines with the most tasks, which are the machines whose
    /// owners would notice least.
    deleted: Vec<String>,
    /// False if any page held a row that would not decode, which makes the listing unable to say
    /// what is missing.
    whole: bool,
    /// The server's clock, from the first page that carried one.
    server_time: Option<String>,
}

/// A top-level string from a response envelope. `None` for absent, null, or any other type —
/// every caller's answer to a server that does not send one is to fall back, not to fail.
fn string_named(body: &serde_json::Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

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

/// The task fields this client asks the server to leave out, as `meta.omitted` would echo them.
/// `leanListMembers=1` is the older spelling of `omit=listMembers`, and the server does not echo
/// it today — naming it here is what keeps the guard right if it ever starts to. Never
/// `description`: see [`require_only_omitted`].
const OMITTED_BY_REQUEST: [&str; 1] = ["listMembers"];

/// Refuse a task page trimmed of anything but `requested` — AITD-499, matching the Swift read's
/// `TasksResponse.requireOnlyOmitted` (AITD-498).
///
/// `GET /api/v1/tasks` takes `?omit=` (and the older `leanListMembers=1`) and answers with
/// `meta.omitted` naming what it actually left out — absent, not empty, on a full response, and
/// absent from a server that predates it. That echo is the only thing that tells "not sent" from
/// "has none": an omitted field is missing from the task object rather than null, and
/// [`Task::description`] is `#[serde(default)]`, so a trimmed row decodes with an empty
/// description that the merge then writes over the real text — and the next edit pushes the blank
/// back. The server also IGNORES an `omit` name it does not know, so the echo is equally the only
/// proof that a request was understood.
///
/// Refusing the page rather than merging field by field is deliberate: the sync fills the replica
/// the detail view renders offline, so core keeps asking for the full shape, and a page that is
/// not the full shape is a bug somewhere else. [`ApiError::Decode`] is not retryable, and a pass
/// that returns it leaves the cache exactly as it was.
fn require_only_omitted(body: &serde_json::Value, requested: &[&str]) -> Result<(), ApiError> {
    let unrequested: Vec<&str> = body
        .get(endpoints::envelope::META)
        .and_then(|meta| meta.get(endpoints::envelope::OMITTED))
        .and_then(serde_json::Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|field| !requested.contains(field))
                .collect()
        })
        .unwrap_or_default();
    if unrequested.is_empty() {
        return Ok(());
    }
    Err(ApiError::Decode(format!(
        "the server left fields out of a task page this client did not ask to drop: {}",
        unrequested.join(", ")
    )))
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

    /// The same, keeping the transport so a test can assert on what was actually asked for.
    fn fixture_with_transport(transport: StubTransport) -> (Fixture, Arc<StubTransport>) {
        let transport = Arc::new(transport);
        let store = Arc::new(Store::in_memory().expect("opens"));
        let clock = Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z"));
        let client = Arc::new(ApiClient::new(
            "https://astrid.cc",
            transport.clone(),
            Arc::new(MemorySecureStore::new()),
        ));
        let runner = Arc::new(Runner::new(client.clone(), store.clone(), clock.clone()));
        (
            Fixture {
                sync: SyncManager::new(client, store.clone(), clock, runner),
                store,
            },
            transport,
        )
    }

    /// Every task URL the transport was asked for, in order.
    fn task_urls(transport: &StubTransport) -> Vec<String> {
        transport
            .requests()
            .into_iter()
            .map(|request| request.url)
            .filter(|url| url.contains("/api/v1/tasks?"))
            .collect()
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

    /// AITD-454: a pass folds its rows in one transaction per phase — the lists, then the tasks —
    /// rather than one auto-committed statement per row. Forty tasks were over a hundred commits,
    /// each its own journal write, with a list being drawn able to see half a pass.
    #[tokio::test]
    async fn aitd_454_a_pass_merges_in_a_transaction_not_row_by_row() {
        let tasks: Vec<serde_json::Value> = (0..40)
            .map(|n| json!({ "id": format!("t{n}"), "title": "x", "listIds": ["l1"] }))
            .collect();
        let fixture = fixture(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [{ "id": "l1", "name": "Home" }, { "id": "l2", "name": "Work" }] }),
                )
                .push_json("/api/v1/tasks", 200, json!({ "tasks": tasks }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );
        let commits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        fixture
            .store
            .transaction(|connection| {
                let commits = commits.clone();
                connection.commit_hook(Some(move || {
                    commits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    false
                }));
                Ok(())
            })
            .expect("hooks");
        commits.store(0, std::sync::atomic::Ordering::SeqCst);

        let report = fixture.sync.sync().await;
        assert_eq!(report.tasks_added, 40);
        assert_eq!(report.lists_added, 2);
        let commits = commits.load(std::sync::atomic::Ordering::SeqCst);
        assert!(commits <= 10, "{commits} commits for one pass");
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

    /// Task `86109567`: the delta cursor is the server's own stamp, sent back verbatim and with
    /// no skew margin.
    ///
    /// This machine's clock reads 12:00 and the server's reads 10:00 — two hours fast, the case
    /// the five-minute margin never covered. Asking from `12:00 − 5 min` would skip every edit
    /// anyone else made between 10:00 and 11:55, with nothing to raise.
    #[tokio::test]
    async fn task_86109567_the_delta_cursor_is_the_servers_stamp_not_this_machines_clock() {
        let (fixture, transport) = fixture_with_transport(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [], "serverTime": "2026-09-07T10:00:00Z" }),
                )
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [], "serverTime": "2026-09-07T10:00:00Z" }),
                )
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [], "serverTime": "2026-09-07T10:30:00Z" }),
                )
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [], "serverTime": "2026-09-07T10:30:00Z" }),
                ),
        );

        assert!(fixture.sync.sync().await.fetched, "the first pass runs");
        let second = fixture.sync.sync().await;
        assert!(second.delta, "the second pass is a delta pass");

        let asked = task_urls(&transport);
        let second_ask = asked.last().expect("a second task request");
        assert!(
            second_ask.contains("updatedSince=2026-09-07T10%3A00%3A00Z"),
            "the server's own stamp goes back verbatim, not the local clock: {second_ask}"
        );
        assert!(
            !second_ask.contains("11%3A55"),
            "no five-minute margin on a server stamp: {second_ask}"
        );
    }

    /// Task `86109567`: the pass stores the EARLIEST server stamp it was given, which is the one
    /// from its first request. The lists query ran before the tasks query, so storing the tasks
    /// stamp would skip a list edited between the two.
    #[tokio::test]
    async fn task_86109567_the_stored_cursor_is_the_first_stamp_of_the_pass() {
        let (fixture, transport) = fixture_with_transport(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [], "serverTime": "2026-09-07T10:00:00Z" }),
                )
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [], "serverTime": "2026-09-07T10:00:09Z" }),
                )
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );

        fixture.sync.sync().await;
        fixture.sync.sync().await;

        let second_ask = task_urls(&transport).last().expect("a second ask").clone();
        assert!(
            second_ask.contains("updatedSince=2026-09-07T10%3A00%3A00Z"),
            "the lists stamp, not the later tasks one: {second_ask}"
        );
    }

    /// Task `86109567`: a deployment that sends no `serverTime` keeps the old behaviour — the
    /// local clock with the five-minute margin — rather than losing its delta pass entirely.
    #[tokio::test]
    async fn task_86109567_a_server_without_a_stamp_still_asks_from_the_local_clock() {
        let (fixture, transport) = fixture_with_transport(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] }))
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json("/api/v1/tasks", 200, json!({ "tasks": [] })),
        );

        fixture.sync.sync().await;
        assert!(fixture.sync.sync().await.delta, "still a delta pass");

        let second_ask = task_urls(&transport).last().expect("a second ask").clone();
        assert!(
            second_ask.contains("updatedSince=2026-09-07T11%3A55%3A00Z"),
            "the local stamp less the margin: {second_ask}"
        );
    }

    /// Task `86109567`: the walk follows the server's `nextCursor` rather than counting offsets,
    /// so a row that moves mid-walk cannot push another off a page and out of the pass.
    #[tokio::test]
    async fn task_86109567_the_walk_follows_the_servers_cursor() {
        let (fixture, transport) = fixture_with_transport(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [{ "id": "l1", "name": "Home" }], "serverTime": "2026-09-07T10:00:00Z" }),
                )
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({
                        "tasks": [{ "id": "t1", "title": "one", "listIds": ["l1"] }],
                        "serverTime": "2026-09-07T10:00:00Z",
                        "nextCursor": "page-two",
                    }),
                )
                // No `nextCursor`: the last page, even though it is not short.
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({
                        "tasks": [{ "id": "t2", "title": "two", "listIds": ["l1"] }],
                        "serverTime": "2026-09-07T10:00:00Z",
                    }),
                ),
        );

        let report = fixture.sync.sync().await;
        assert_eq!(report.tasks_added, 2, "both pages are applied: {report:?}");

        let asked = task_urls(&transport);
        assert_eq!(asked.len(), 2, "two pages, then stop: {asked:?}");
        assert!(
            asked[0].contains("cursor=&") || asked[0].ends_with("cursor="),
            "an empty cursor starts the walk: {}",
            asked[0]
        );
        assert!(
            asked[1].contains("cursor=page-two"),
            "the second page is asked for by the server's cursor: {}",
            asked[1]
        );
        assert!(
            !asked.iter().any(|url| url.contains("offset=")),
            "a cursor walk never counts offsets: {asked:?}"
        );
    }

    /// Task `86109567`: a deployment that ignores `cursor` must not be read as "one page and
    /// done". Without `serverTime` to vouch for it, a full page with no `nextCursor` means the
    /// parameter was dropped, and the walk falls back to offsets rather than truncating a library.
    #[tokio::test]
    async fn task_86109567_a_server_that_ignores_the_cursor_falls_back_to_offsets() {
        let full_page: Vec<serde_json::Value> = (0..1000)
            .map(|n| json!({ "id": format!("t{n}"), "title": "x", "listIds": ["l1"] }))
            .collect();
        let (fixture, transport) = fixture_with_transport(
            StubTransport::new()
                .push_json(
                    "/api/v1/lists",
                    200,
                    json!({ "lists": [{ "id": "l1", "name": "Home" }] }),
                )
                .push_json("/api/v1/tasks", 200, json!({ "tasks": full_page }))
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({ "tasks": [{ "id": "last", "title": "y", "listIds": ["l1"] }] }),
                ),
        );

        let report = fixture.sync.sync().await;
        assert_eq!(
            report.tasks_added, 1001,
            "the page past the first is not lost: {report:?}"
        );

        let asked = task_urls(&transport);
        assert_eq!(asked.len(), 2, "two requests: {}", asked.len());
        assert!(
            asked[1].contains("offset=1000"),
            "the walk continues by offset: {}",
            asked[1]
        );
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

    /// AITD-499: a page the server trimmed must not blank what it left out.
    ///
    /// `Task::description` is `#[serde(default)]`, so a task object WITHOUT the key decodes as
    /// `""` — and a newer server version writes that over the real text, which the next edit then
    /// pushes back. `meta.omitted` is the only thing that tells "not sent" from "has none", so a
    /// page naming a field this request never asked to drop is refused and the cache kept.
    #[tokio::test]
    async fn aitd_499_a_page_trimmed_of_description_is_refused_and_the_cache_kept() {
        let fixture = fixture(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({
                        "tasks": [{
                            "id": "t1", "title": "Buy milk",
                            "updatedAt": "2026-09-07T12:05:00Z"
                        }],
                        "meta": { "total": 1, "omitted": ["description"] }
                    }),
                ),
        );
        let mut cached = Task::new("t1", "Buy milk");
        cached.description = Some("The long note the person typed".to_string());
        cached.updated_at = date::parse("2026-09-07T12:00:00Z");
        fixture.store.upsert_task(&cached).expect("stores");

        let report = fixture.sync.sync().await;
        assert!(!report.fetched, "a trimmed page is not a pass: {report:?}");
        assert_eq!(
            fixture
                .store
                .task("t1")
                .expect("reads")
                .expect("present")
                .description,
            Some("The long note the person typed".to_string()),
            "the description the server did not send is the one on the device"
        );
        assert_eq!(
            fixture.sync.last_sync(),
            None,
            "a refused page leaves no delta stamp, so the next pass pulls everything"
        );
    }

    /// The guard is about UNREQUESTED omissions. This client sends `leanListMembers=1`, so a
    /// server that echoes that one is doing what it was asked and the page still applies.
    #[tokio::test]
    async fn aitd_499_a_page_that_dropped_only_what_was_asked_for_still_applies() {
        let fixture = fixture(empty_task_pages(
            StubTransport::new()
                .push_json("/api/v1/lists", 200, json!({ "lists": [] }))
                .push_json(
                    "/api/v1/tasks",
                    200,
                    json!({
                        "tasks": [{ "id": "t1", "title": "Buy milk" }],
                        "meta": { "total": 1, "omitted": ["listMembers"] }
                    }),
                ),
        ));

        let report = fixture.sync.sync().await;
        assert!(report.fetched);
        assert_eq!(report.tasks_added, 1);
    }

    #[test]
    fn aitd_499_the_omission_guard_reads_the_echo_rather_than_assuming() {
        let requested = ["listMembers"];
        // A full response carries no `omitted` at all — absent, not empty.
        require_only_omitted(&json!({ "tasks": [], "meta": { "total": 0 } }), &requested)
            .expect("a full page");
        require_only_omitted(&json!({ "tasks": [] }), &requested).expect("no meta at all");
        require_only_omitted(
            &json!({ "tasks": [], "meta": { "omitted": [] } }),
            &requested,
        )
        .expect("an empty echo");
        require_only_omitted(
            &json!({ "tasks": [], "meta": { "omitted": ["listMembers"] } }),
            &requested,
        )
        .expect("what was asked for");
        // Not an array, so not an echo.
        require_only_omitted(
            &json!({ "tasks": [], "meta": { "omitted": "description" } }),
            &requested,
        )
        .expect("nothing readable to act on");

        let refused = require_only_omitted(
            &json!({ "tasks": [], "meta": { "omitted": ["listMembers", "description"] } }),
            &requested,
        )
        .expect_err("a field nobody asked to drop");
        assert!(
            matches!(&refused, ApiError::Decode(message) if message.contains("description")
                && !message.contains("listMembers")),
            "{refused:?}"
        );
        assert!(
            !refused.is_retryable(),
            "sending the same request again cannot help"
        );
    }
}
