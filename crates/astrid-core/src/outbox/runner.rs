//! Draining the journal.
//!
//! Ported from `astrid-ios/Astrid App/Core/Outbox/OutboxRunner.swift`.
//!
//! One pass does four things, in this order, and the order matters:
//!
//! 1. **Load and recover.** Anything a dead process left claimed becomes pending again.
//! 2. **Strand.** Entries whose dependency can never complete are dead-lettered *before* the batch
//!    is chosen, so they never occupy a lane a runnable entry needs.
//! 3. **Dispatch.** At most one entry per lane, up to the concurrency limit, oldest first.
//! 4. **Prune.** Completed entries nothing still refers to are dropped.
//!
//! The pass repeats until nothing is runnable, so a create that completes unblocks the edits
//! behind it within the same drain rather than on the next tick — which is what makes "type a
//! task, edit it, watch it sync" feel like one operation instead of three.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::api::ApiClient;
use crate::platform::Clock;
use crate::store::{Result, Store};

use super::entry::Entry;
use super::handlers::{self, Outcome};
use super::{journal, scheduler};

/// How many entries may be in flight at once.
///
/// Small on purpose. The lanes already stop two writes to one entity racing; the remaining
/// argument for concurrency is latency across unrelated entities, and past four the server's
/// rate limiting starts costing more than the parallelism wins.
pub const CONCURRENCY_LIMIT: usize = 4;

/// A cap on how many times one drain re-runs before giving the caller its turn back.
///
/// Not a correctness bound — the loop terminates on its own when nothing is runnable — but a
/// liveness one: a pathological journal must not hold the runner forever.
const MAX_PASSES: usize = 64;

/// What a drain did, for logging and for the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub completed: usize,
    pub retried: usize,
    pub dead_lettered: usize,
    pub pruned: usize,
}

impl DrainReport {
    pub fn did_anything(&self) -> bool {
        self.completed + self.retried + self.dead_lettered > 0
    }
}

pub struct Runner {
    client: Arc<ApiClient>,
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
}

impl Runner {
    pub fn new(client: Arc<ApiClient>, store: Arc<Store>, clock: Arc<dyn Clock>) -> Self {
        Runner {
            client,
            store,
            clock,
        }
    }

    /// Run everything that can run, and keep running while completing one thing unblocks another.
    pub async fn drain(&self) -> Result<DrainReport> {
        let mut report = DrainReport::default();
        for _ in 0..MAX_PASSES {
            let now = self.clock.now();
            let entries = journal::load(&self.store)?;
            self.dead_letter_stranded(&entries, now, &mut report)?;

            let entries = journal::all(&self.store)?;
            let ready = scheduler::runnable(&entries, now, &HashSet::new());
            let batch = scheduler::concurrent_batch(&ready, CONCURRENCY_LIMIT);
            if batch.is_empty() {
                break;
            }

            for entry in batch {
                self.run_one(entry, now, &mut report).await?;
            }
        }

        report.pruned = journal::prune(&self.store, self.clock.now())?;
        Ok(report)
    }

    /// When the runner should wake itself next, if nothing is runnable now.
    pub fn next_wakeup(&self) -> Result<Option<DateTime<Utc>>> {
        let entries = journal::all(&self.store)?;
        Ok(scheduler::next_wakeup(
            &entries,
            self.clock.now(),
            &HashSet::new(),
        ))
    }

    async fn run_one(
        &self,
        entry: Entry,
        now: DateTime<Utc>,
        report: &mut DrainReport,
    ) -> Result<()> {
        // Claim it. Losing the race means another runner has it; there is nothing to do and
        // nothing to report.
        if !journal::mark_running(&self.store, &entry.id, now)? {
            return Ok(());
        }

        // Whatever temporary ids this entry names may have been resolved since it was written.
        let entry = self.resolve_temp_ids(entry, now)?;

        match handlers::perform(&self.client, &self.store, &entry).await {
            Outcome::Done(result) => {
                journal::mark_completed(&self.store, &entry.id, result.as_ref(), now)?;
                report.completed += 1;
            }
            Outcome::Retry(reason) => {
                let attempts = entry.attempts + 1;
                if scheduler::should_dead_letter(attempts) {
                    journal::mark_dead(
                        &self.store,
                        &entry.id,
                        &format!("out of attempts after {attempts}: {reason}"),
                        now,
                    )?;
                    report.dead_lettered += 1;
                } else {
                    journal::mark_retry(
                        &self.store,
                        &entry.id,
                        attempts,
                        scheduler::next_attempt_at(now, attempts),
                        &reason,
                        now,
                    )?;
                    report.retried += 1;
                }
            }
            Outcome::Dead(reason) => {
                journal::mark_dead(&self.store, &entry.id, &reason, now)?;
                report.dead_lettered += 1;
            }
        }
        Ok(())
    }

    /// Dead-letter everything that can never run, so it stops occupying a lane and stops looking
    /// to the user like a write still on its way.
    fn dead_letter_stranded(
        &self,
        entries: &[Entry],
        now: DateTime<Utc>,
        report: &mut DrainReport,
    ) -> Result<()> {
        let mut doomed = scheduler::stranded(entries);
        doomed.extend(scheduler::temp_stranded(entries));
        for id in doomed {
            journal::mark_dead(
                &self.store,
                &id,
                "a write it depended on failed permanently",
                now,
            )?;
            report.dead_lettered += 1;
        }
        Ok(())
    }

    /// Rewrite every `temp_` id in the payload that the server has since given a real one.
    ///
    /// Done generically, over the whole payload, rather than field by field: an entry enqueued
    /// against a task that had not been created yet can name it in `taskId`, in a `listIds` array,
    /// or in a `parentTaskId`, and a per-field list is a list that will be missing the next field
    /// somebody adds. The rewrite is persisted, so a later attempt does not have to redo it — and
    /// so an entry whose ids resolved is legible in the journal.
    fn resolve_temp_ids(&self, mut entry: Entry, now: DateTime<Utc>) -> Result<Entry> {
        let mut changed = false;
        rewrite_ids(
            &mut entry.payload,
            &mut |id| match self.store.resolve_id(id) {
                Ok(resolved) if resolved != id => {
                    changed = true;
                    Some(resolved)
                }
                _ => None,
            },
        );
        if changed {
            journal::update_payload(&self.store, &entry.id, &entry.payload, now)?;
        }
        Ok(entry)
    }
}

/// Walk a JSON value, offering every `temp_`-prefixed string to `resolve`.
fn rewrite_ids(value: &mut serde_json::Value, resolve: &mut impl FnMut(&str) -> Option<String>) {
    match value {
        serde_json::Value::String(text) => {
            if crate::model::is_temp_id(text) {
                if let Some(resolved) = resolve(text) {
                    *text = resolved;
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_ids(item, resolve);
            }
        }
        serde_json::Value::Object(fields) => {
            for (_, field) in fields.iter_mut() {
                rewrite_ids(field, resolve);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{StubTransport, TransportError};
    use crate::model::date;
    use crate::outbox::entry::kind;
    use crate::platform::{FixedClock, MemorySecureStore};
    use chrono::Duration;

    fn t0() -> DateTime<Utc> {
        date::parse("2026-09-07T12:00:00Z").expect("an instant")
    }

    struct Fixture {
        runner: Runner,
        store: Arc<Store>,
        clock: Arc<FixedClock>,
        transport: Arc<StubTransport>,
    }

    fn fixture(transport: StubTransport) -> Fixture {
        let transport = Arc::new(transport);
        let store = Arc::new(Store::in_memory().expect("opens"));
        let clock = Arc::new(FixedClock::at(t0()));
        let client = Arc::new(ApiClient::new(
            "https://astrid.cc",
            transport.clone(),
            Arc::new(MemorySecureStore::new()),
        ));
        Fixture {
            runner: Runner::new(client, store.clone(), clock.clone()),
            store,
            clock,
            transport,
        }
    }

    fn enqueue(store: &Store, entry: Entry) -> Entry {
        journal::enqueue(store, &entry).expect("enqueues")
    }

    fn create_task(id: &str, temp_id: &str) -> Entry {
        Entry::new(
            id,
            kind::CREATE_TASK,
            serde_json::json!({ "body": { "title": "Buy milk" } }),
            temp_id,
            t0(),
        )
        .for_temp_id(temp_id)
    }

    #[tokio::test]
    async fn a_drain_sends_what_is_ready_and_marks_it_done() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/tasks",
            200,
            serde_json::json!({ "task": { "id": "cm3real", "title": "Buy milk" } }),
        ));
        enqueue(&fixture.store, create_task("e1", "temp_1"));

        let report = fixture.runner.drain().await.expect("drains");
        assert_eq!(report.completed, 1);
        assert_eq!(fixture.transport.requests().len(), 1);
    }

    /// The whole point of one drain rather than one entry per tick: creating the task unblocks the
    /// edit behind it in the same pass, so it reads as one operation instead of three ticks.
    #[tokio::test]
    async fn completing_a_dependency_unblocks_its_dependent_within_the_same_drain() {
        let fixture = fixture(
            StubTransport::new()
                .push_json(
                    "/api/v1/tasks",
                    200,
                    serde_json::json!({ "task": { "id": "cm3real", "title": "Buy milk" } }),
                )
                .push_json(
                    "/api/v1/tasks/cm3real",
                    200,
                    serde_json::json!({ "task": { "id": "cm3real", "title": "Buy oat milk" } }),
                ),
        );
        enqueue(&fixture.store, create_task("create", "temp_1"));
        enqueue(
            &fixture.store,
            Entry::new(
                "update",
                kind::UPDATE_TASK,
                serde_json::json!({ "taskId": "temp_1", "body": { "title": "Buy oat milk" } }),
                "crid-update",
                t0(),
            )
            .depending_on(["create".to_string()])
            .for_temp_id("temp_1"),
        );

        let report = fixture.runner.drain().await.expect("drains");
        assert_eq!(report.completed, 2);
        // And the edit went to the id the server gave, not to the temporary one.
        let urls: Vec<String> = fixture
            .transport
            .requests()
            .into_iter()
            .map(|request| request.url)
            .collect();
        assert!(urls[1].ends_with("/api/v1/tasks/cm3real"), "{urls:?}");
    }

    /// The temporary id can be anywhere in the payload, so the rewrite walks the whole thing.
    #[tokio::test]
    async fn a_temporary_id_is_resolved_wherever_it_appears_in_the_payload() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/tasks/cm3parent",
            200,
            serde_json::json!({ "task": { "id": "cm3parent" } }),
        ));
        fixture
            .store
            .record_id_mapping("temp_parent", "cm3parent", t0())
            .expect("records");
        enqueue(
            &fixture.store,
            Entry::new(
                "e1",
                kind::UPDATE_TASK,
                serde_json::json!({
                    "taskId": "temp_parent",
                    "body": { "listIds": ["l1", "temp_parent"] }
                }),
                "crid",
                t0(),
            ),
        );

        fixture.runner.drain().await.expect("drains");
        let body: serde_json::Value = serde_json::from_slice(
            fixture.transport.requests()[0]
                .body
                .as_ref()
                .expect("a body"),
        )
        .expect("valid JSON");
        assert_eq!(body["listIds"][1], "cm3parent");
        // And it is written back, so a later attempt does not have to work it out again.
        let stored = journal::entry(&fixture.store, "e1")
            .expect("reads")
            .expect("present");
        assert_eq!(stored.payload["taskId"], "cm3parent");
    }

    #[tokio::test]
    async fn being_offline_retries_with_a_backoff_rather_than_giving_up() {
        let fixture =
            fixture(StubTransport::new().fallback(Err(TransportError::Unreachable("dns".into()))));
        enqueue(&fixture.store, create_task("e1", "temp_1"));

        let report = fixture.runner.drain().await.expect("drains");
        assert_eq!(report.retried, 1);

        let entry = journal::entry(&fixture.store, "e1")
            .expect("reads")
            .expect("present");
        assert_eq!(entry.attempts, 1);
        assert_eq!(entry.next_attempt_at, t0() + scheduler::backoff(1));
        assert_eq!(
            fixture.runner.next_wakeup().expect("reads"),
            Some(entry.next_attempt_at)
        );
    }

    /// A backoff that is not yet up means the drain does nothing at all, rather than hammering.
    #[tokio::test]
    async fn an_entry_in_backoff_is_left_alone_until_its_time() {
        let fixture =
            fixture(StubTransport::new().fallback(Err(TransportError::Unreachable("dns".into()))));
        enqueue(&fixture.store, create_task("e1", "temp_1"));

        fixture.runner.drain().await.expect("drains");
        let sent_after_first = fixture.transport.requests().len();
        fixture.runner.drain().await.expect("drains");
        assert_eq!(
            fixture.transport.requests().len(),
            sent_after_first,
            "a second drain inside the backoff must send nothing"
        );

        fixture.clock.advance(Duration::seconds(60));
        fixture.runner.drain().await.expect("drains");
        assert!(fixture.transport.requests().len() > sent_after_first);
    }

    #[tokio::test]
    async fn a_write_the_server_refuses_is_dead_lettered_with_its_reason() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/tasks",
            403,
            serde_json::json!({ "error": "not yours" }),
        ));
        enqueue(&fixture.store, create_task("e1", "temp_1"));

        let report = fixture.runner.drain().await.expect("drains");
        assert_eq!(report.dead_lettered, 1);
        let entry = journal::entry(&fixture.store, "e1")
            .expect("reads")
            .expect("present");
        assert_eq!(entry.status, super::super::entry::Status::FailedPermanent);
        assert!(entry.last_error.expect("a reason").contains("403"));
    }

    /// An edit to a task whose create was refused would otherwise sit pending forever, looking
    /// like a write still on its way.
    #[tokio::test]
    async fn edits_to_a_task_whose_create_was_refused_are_dead_lettered_too() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/tasks",
            403,
            serde_json::json!({ "error": "not yours" }),
        ));
        enqueue(&fixture.store, create_task("create", "temp_1"));
        enqueue(
            &fixture.store,
            Entry::new(
                "update",
                kind::UPDATE_TASK,
                serde_json::json!({ "taskId": "temp_1", "body": { "title": "x" } }),
                "crid-update",
                t0(),
            )
            .for_temp_id("temp_1"),
        );

        fixture.runner.drain().await.expect("drains");
        let update = journal::entry(&fixture.store, "update")
            .expect("reads")
            .expect("present");
        assert_eq!(update.status, super::super::entry::Status::FailedPermanent);
    }

    /// Eight failures and it stops. A queue that retries forever is a queue that never drains.
    #[tokio::test]
    async fn an_entry_gives_up_after_its_attempts_are_gone() {
        let fixture = fixture(StubTransport::new().fallback(Err(TransportError::Timeout)));
        enqueue(&fixture.store, create_task("e1", "temp_1"));

        for _ in 0..scheduler::MAX_ATTEMPTS {
            fixture.runner.drain().await.expect("drains");
            fixture.clock.advance(Duration::seconds(600));
        }

        let entry = journal::entry(&fixture.store, "e1")
            .expect("reads")
            .expect("present");
        assert_eq!(entry.status, super::super::entry::Status::FailedPermanent);
        assert!(entry
            .last_error
            .expect("a reason")
            .contains("out of attempts"));
    }

    /// Two writes to one task never go together — that race is how a rename comes back.
    #[tokio::test]
    async fn two_writes_to_one_task_are_sent_one_after_the_other() {
        let fixture = fixture(
            StubTransport::new()
                .push_json(
                    "/api/v1/tasks/t1",
                    200,
                    serde_json::json!({ "task": { "id": "t1", "title": "first" } }),
                )
                .push_json(
                    "/api/v1/tasks/t1",
                    200,
                    serde_json::json!({ "task": { "id": "t1", "title": "second" } }),
                ),
        );
        for (id, title) in [("a", "first"), ("b", "second")] {
            enqueue(
                &fixture.store,
                Entry::new(
                    id,
                    kind::UPDATE_TASK,
                    serde_json::json!({ "taskId": "t1", "body": { "title": title } }),
                    format!("crid-{id}"),
                    t0(),
                ),
            );
        }

        fixture.runner.drain().await.expect("drains");
        let bodies: Vec<String> = fixture
            .transport
            .requests()
            .into_iter()
            .map(|request| String::from_utf8_lossy(&request.body.unwrap_or_default()).into_owned())
            .collect();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].contains("first"), "{bodies:?}");
        assert!(bodies[1].contains("second"), "{bodies:?}");
    }

    #[tokio::test]
    async fn a_drain_with_nothing_to_do_does_nothing() {
        let fixture = fixture(StubTransport::new());
        let report = fixture.runner.drain().await.expect("drains");
        assert!(!report.did_anything());
        assert!(fixture.transport.requests().is_empty());
    }

    /// The recovery path end to end: an entry left claimed by a process that died runs on the next
    /// drain rather than wedging its lane forever.
    #[tokio::test]
    async fn an_entry_left_in_flight_by_a_crash_runs_again() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/tasks",
            200,
            serde_json::json!({ "task": { "id": "cm3real" } }),
        ));
        enqueue(&fixture.store, create_task("e1", "temp_1"));
        journal::mark_running(&fixture.store, "e1", t0()).expect("claims");

        let report = fixture.runner.drain().await.expect("drains");
        assert_eq!(report.completed, 1);
    }
}
