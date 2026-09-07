//! What runs, when, in what order, and when to give up.
//!
//! Ported from `astrid-ios/Astrid App/Core/Outbox/OutboxScheduler.swift` together with its tests,
//! which are the specification.
//!
//! Pure on purpose. This is the highest-risk logic in the crate — a bug here breaks every offline
//! write, silently, on someone else's machine — so it holds no state, does no I/O, and every rule
//! in it is a function a test can call directly. [`super::runner`] owns the journal and applies
//! what this decides.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};

use super::entry::{kind, Entry, Status};

/// Give up on an entry after this many failed attempts.
pub const MAX_ATTEMPTS: i64 = 8;

/// The first retry waits this long; each one after doubles, up to [`MAX_BACKOFF_SECS`].
pub const BASE_BACKOFF_SECS: i64 = 2;

/// Backoff is clamped here so a long-failing entry still tries periodically rather than drifting
/// into never.
pub const MAX_BACKOFF_SECS: i64 = 300;

/// How long a completed entry is kept before pruning. Referenced ones are kept regardless.
pub const COMPLETED_RETENTION_SECS: i64 = 3600;

/// How long to wait after the `attempts`-th failure.
pub fn backoff(attempts: i64) -> Duration {
    let exponent = attempts.saturating_sub(1).clamp(0, 32) as u32;
    let seconds = BASE_BACKOFF_SECS.saturating_mul(1i64 << exponent);
    Duration::seconds(seconds.min(MAX_BACKOFF_SECS))
}

/// When an entry that has failed `attempts` times may next run.
pub fn next_attempt_at(now: DateTime<Utc>, attempts: i64) -> DateTime<Utc> {
    now + backoff(attempts)
}

/// Statuses that will not succeed on a retry, so the entry is dead-lettered at once.
///
/// Auth (401), validation (400/422) and gone (403/404/410). Everything else — including 408, 429
/// and every 5xx — is the server's problem or the network's, and waiting is the right answer.
pub fn is_permanent_failure(status: u16) -> bool {
    matches!(status, 400 | 401 | 403 | 404 | 410 | 422)
}

/// True once an entry has burned through its attempts.
pub fn should_dead_letter(attempts: i64) -> bool {
    attempts >= MAX_ATTEMPTS
}

pub fn dependencies_satisfied(entry: &Entry, completed: &HashSet<String>) -> bool {
    entry.depends_on.iter().all(|id| completed.contains(id))
}

/// Whether one entry may run at this instant.
pub fn is_runnable(
    entry: &Entry,
    now: DateTime<Utc>,
    completed: &HashSet<String>,
    in_flight: &HashSet<String>,
) -> bool {
    entry.status == Status::Pending
        && now >= entry.next_attempt_at
        && !in_flight.contains(&entry.id)
        && dependencies_satisfied(entry, completed)
}

fn completed_ids(entries: &[Entry]) -> HashSet<String> {
    entries
        .iter()
        .filter(|entry| entry.status == Status::Completed)
        .map(|entry| entry.id.clone())
        .collect()
}

/// Everything that should be dispatched now, oldest first.
///
/// Completion is derived from the journal itself rather than passed in, so a dependency that
/// finished a moment ago is already visible here.
pub fn runnable(entries: &[Entry], now: DateTime<Utc>, in_flight: &HashSet<String>) -> Vec<Entry> {
    let completed = completed_ids(entries);
    let mut ready: Vec<Entry> = entries
        .iter()
        .filter(|entry| is_runnable(entry, now, &completed, in_flight))
        .cloned()
        .collect();
    ready.sort_by_key(|entry| (entry.created_at, entry.sequence));
    ready
}

/// At most one entry per lane, oldest first, up to `limit`.
///
/// Bounded concurrency across unrelated entities, strict FIFO within one. See
/// [`Entry::serialization_key`] for what a lane is.
pub fn concurrent_batch(ready: &[Entry], limit: usize) -> Vec<Entry> {
    if limit == 0 {
        return Vec::new();
    }
    let mut ordered = ready.to_vec();
    ordered.sort_by_key(|entry| (entry.created_at, entry.sequence));

    let mut lanes = HashSet::new();
    let mut batch = Vec::new();
    for entry in ordered {
        if !lanes.insert(entry.serialization_key()) {
            continue;
        }
        batch.push(entry);
        if batch.len() == limit {
            break;
        }
    }
    batch
}

/// When the runner should wake itself up next.
///
/// `None` when something is runnable already (draining handles it now) or when nothing is waiting
/// on the clock — an entry blocked on a dependency is woken by that dependency completing, not by
/// a timer, and setting one for it would spin.
pub fn next_wakeup(
    entries: &[Entry],
    now: DateTime<Utc>,
    in_flight: &HashSet<String>,
) -> Option<DateTime<Utc>> {
    let completed = completed_ids(entries);
    if entries
        .iter()
        .any(|entry| is_runnable(entry, now, &completed, in_flight))
    {
        return None;
    }
    entries
        .iter()
        .filter(|entry| {
            entry.status == Status::Pending
                && !in_flight.contains(&entry.id)
                && dependencies_satisfied(entry, &completed)
                && entry.next_attempt_at > now
        })
        .map(|entry| entry.next_attempt_at)
        .min()
}

/// Drop completed entries that are old **and** no longer referenced by anything pending.
///
/// Everything else is kept: a completed entry a pending one still depends on is what that
/// dependency resolves against, and pruning it would strand the dependent.
pub fn pruned(entries: &[Entry], now: DateTime<Utc>) -> Vec<Entry> {
    let referenced: HashSet<&String> = entries
        .iter()
        .filter(|entry| matches!(entry.status, Status::Pending | Status::Running))
        .flat_map(|entry| entry.depends_on.iter())
        .collect();
    let cutoff = now - Duration::seconds(COMPLETED_RETENTION_SECS);
    entries
        .iter()
        .filter(|entry| {
            entry.status != Status::Completed
                || referenced.contains(&entry.id)
                || entry.updated_at > cutoff
        })
        .cloned()
        .collect()
}

/// Pending entries that can never run, because a dependency has permanently failed or is not
/// there at all.
///
/// Propagates: a dependent of a stranded entry is itself stranded. Without this they sit pending
/// forever, which looks to the user like a write that neither landed nor failed.
pub fn stranded(entries: &[Entry]) -> HashSet<String> {
    let by_id: HashMap<&str, &Entry> = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let completed = completed_ids(entries);

    let mut stranded: HashSet<String> = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for entry in entries {
            if entry.status != Status::Pending || stranded.contains(&entry.id) {
                continue;
            }
            let doomed = entry.depends_on.iter().any(|dependency| {
                match by_id.get(dependency.as_str()) {
                    Some(found) => {
                        found.status == Status::FailedPermanent || stranded.contains(dependency)
                    }
                    // Absent and not remembered as completed: nothing will ever complete it.
                    None => !completed.contains(dependency),
                }
            });
            if doomed {
                stranded.insert(entry.id.clone());
                changed = true;
            }
        }
    }
    stranded
}

/// Pending entries that name a temporary task id whose creating entry has permanently failed.
///
/// These have no explicit dependency edge — an edit to a not-yet-synced task is enqueued on its
/// own — so [`stranded`] cannot see them, and they would wait forever for an id that is never
/// coming.
pub fn temp_stranded(entries: &[Entry]) -> HashSet<String> {
    let failed_creates: HashSet<&String> = entries
        .iter()
        .filter(|entry| entry.kind == kind::CREATE_TASK && entry.status == Status::FailedPermanent)
        .filter_map(|entry| entry.temp_id.as_ref())
        .collect();
    if failed_creates.is_empty() {
        return HashSet::new();
    }
    entries
        .iter()
        .filter(|entry| {
            entry.status == Status::Pending
                && entry.kind != kind::CREATE_TASK
                && entry
                    .temp_id
                    .as_ref()
                    .map(|id| failed_creates.contains(id))
                    .unwrap_or(false)
        })
        .map(|entry| entry.id.clone())
        .collect()
}

/// Normalise a journal read back from disk.
///
/// An entry persisted as running was in flight when the process died. On relaunch it is neither
/// runnable, nor prunable, nor stranded — it simply wedges. Resetting it to pending is safe
/// because every kind is idempotent by its `client_request_id`: replaying a write the server
/// already applied returns the same row rather than making a second one.
pub fn recovered_on_load(entries: &[Entry]) -> Vec<Entry> {
    entries
        .iter()
        .map(|entry| {
            let mut entry = entry.clone();
            if entry.status == Status::Running {
                entry.status = Status::Pending;
            }
            entry
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn t0() -> DateTime<Utc> {
        date::parse("2026-09-07T12:00:00Z").expect("an instant")
    }

    struct Build {
        entry: Entry,
    }

    fn entry(id: &str) -> Build {
        Build {
            entry: Entry::new(
                id,
                kind::CREATE_COMMENT,
                serde_json::json!({}),
                format!("crid-{id}"),
                t0(),
            ),
        }
    }

    impl Build {
        fn kind(mut self, kind: &str) -> Self {
            self.entry.kind = kind.to_string();
            self
        }
        fn status(mut self, status: Status) -> Self {
            self.entry.status = status;
            self
        }
        fn depends_on(mut self, ids: &[&str]) -> Self {
            self.entry.depends_on = ids.iter().map(|id| id.to_string()).collect();
            self
        }
        fn temp(mut self, temp_id: &str) -> Self {
            self.entry.temp_id = Some(temp_id.to_string());
            self
        }
        fn due_in(mut self, seconds: i64) -> Self {
            self.entry.next_attempt_at = t0() + Duration::seconds(seconds);
            self
        }
        fn created_at(mut self, offset: i64) -> Self {
            self.entry.created_at = t0() + Duration::seconds(offset);
            self
        }
        fn updated_at(mut self, offset: i64) -> Self {
            self.entry.updated_at = t0() + Duration::seconds(offset);
            self
        }
        fn build(self) -> Entry {
            self.entry
        }
    }

    fn ids(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.id.as_str()).collect()
    }

    // ── Backoff ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn backoff_grows_and_is_capped() {
        assert!(backoff(1) < backoff(2));
        assert!(backoff(2) < backoff(3));
        assert_eq!(backoff(1), Duration::seconds(BASE_BACKOFF_SECS));
        assert_eq!(backoff(100), Duration::seconds(MAX_BACKOFF_SECS));
    }

    /// The exponent is bounded before it is shifted. A machine that comes back after a month with
    /// a large attempt count must not overflow into a negative delay — which would make a doomed
    /// entry the busiest thing in the app.
    #[test]
    fn an_absurd_attempt_count_still_produces_the_cap() {
        assert_eq!(backoff(i64::MAX), Duration::seconds(MAX_BACKOFF_SECS));
        assert_eq!(backoff(0), Duration::seconds(BASE_BACKOFF_SECS));
    }

    #[test]
    fn the_next_attempt_is_now_plus_the_backoff() {
        assert_eq!(next_attempt_at(t0(), 1), t0() + backoff(1));
    }

    // ── Giving up ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn auth_validation_and_gone_are_permanent_and_nothing_else_is() {
        for status in [400, 401, 403, 404, 410, 422] {
            assert!(is_permanent_failure(status), "{status} should be permanent");
        }
        for status in [408, 429, 500, 502, 503] {
            assert!(!is_permanent_failure(status), "{status} should be retried");
        }
    }

    #[test]
    fn it_dead_letters_only_once_the_attempts_are_gone() {
        assert!(!should_dead_letter(MAX_ATTEMPTS - 1));
        assert!(should_dead_letter(MAX_ATTEMPTS));
    }

    // ── Dependencies and runnability ─────────────────────────────────────────────────────────

    #[test]
    fn a_dependency_must_be_completed_not_merely_present() {
        let dependent = entry("c").depends_on(&["a", "b"]).build();
        assert!(!dependencies_satisfied(
            &dependent,
            &HashSet::from(["a".to_string()])
        ));
        assert!(dependencies_satisfied(
            &dependent,
            &HashSet::from(["a".to_string(), "b".to_string()])
        ));
        assert!(dependencies_satisfied(&entry("x").build(), &HashSet::new()));
    }

    #[test]
    fn running_requires_pending_due_free_and_unblocked() {
        let none = HashSet::new();
        assert!(is_runnable(&entry("1").build(), t0(), &none, &none));
        assert!(!is_runnable(
            &entry("2").due_in(60).build(),
            t0(),
            &none,
            &none
        ));
        assert!(!is_runnable(
            &entry("1").build(),
            t0(),
            &none,
            &HashSet::from(["1".to_string()])
        ));
        for status in [Status::Running, Status::Completed, Status::FailedPermanent] {
            assert!(!is_runnable(
                &entry("3").status(status).build(),
                t0(),
                &none,
                &none
            ));
        }
        assert!(!is_runnable(
            &entry("6").depends_on(&["nope"]).build(),
            t0(),
            &none,
            &none
        ));
    }

    #[test]
    fn the_ready_set_is_dependency_aware_and_oldest_first() {
        let entries = vec![
            entry("d").created_at(20).build(),
            entry("c").depends_on(&["missing"]).created_at(5).build(),
            entry("b").depends_on(&["a"]).created_at(10).build(),
            entry("a").status(Status::Completed).created_at(0).build(),
        ];
        assert_eq!(
            ids(&runnable(&entries, t0(), &HashSet::new())),
            vec!["b", "d"]
        );
    }

    // ── Lanes ────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_batch_takes_the_oldest_entry_from_each_distinct_lane() {
        let ready = vec![
            entry("a1").temp("task-a").created_at(0).build(),
            entry("a2").temp("task-a").created_at(1).build(),
            entry("b1").temp("task-b").created_at(2).build(),
            entry("c1").temp("task-c").created_at(3).build(),
        ];
        assert_eq!(ids(&concurrent_batch(&ready, 2)), vec!["a1", "b1"]);
    }

    /// Two writes to one task in flight together is how "my rename came back" happens.
    #[test]
    fn two_entries_in_one_lane_never_go_together() {
        let ready = vec![
            entry("a1").temp("same").created_at(0).build(),
            entry("a2").temp("same").created_at(1).build(),
        ];
        assert_eq!(ids(&concurrent_batch(&ready, 4)), vec!["a1"]);
    }

    #[test]
    fn a_limit_of_zero_dispatches_nothing() {
        assert!(concurrent_batch(&[entry("a").build()], 0).is_empty());
    }

    // ── Waking up ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn nothing_is_scheduled_when_something_can_run_right_now() {
        let entries = vec![entry("a").build(), entry("b").due_in(60).build()];
        assert_eq!(next_wakeup(&entries, t0(), &HashSet::new()), None);
    }

    #[test]
    fn the_wakeup_is_the_earliest_entry_that_is_only_waiting_on_the_clock() {
        let entries = vec![
            entry("a").due_in(120).build(),
            entry("b").due_in(30).build(),
            entry("c").due_in(10).depends_on(&["nobody"]).build(),
        ];
        assert_eq!(
            next_wakeup(&entries, t0(), &HashSet::new()),
            Some(t0() + Duration::seconds(30)),
            "a dependency-blocked entry is woken by its dependency, not by a timer"
        );
    }

    // ── Pruning ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn old_completed_entries_go_and_referenced_ones_stay() {
        let now = t0() + Duration::seconds(COMPLETED_RETENTION_SECS * 2);
        let entries = vec![
            entry("old").status(Status::Completed).updated_at(0).build(),
            entry("referenced")
                .status(Status::Completed)
                .updated_at(0)
                .build(),
            entry("recent")
                .status(Status::Completed)
                .updated_at(COMPLETED_RETENTION_SECS * 2 - 5)
                .build(),
            entry("waiting").depends_on(&["referenced"]).build(),
            entry("dead")
                .status(Status::FailedPermanent)
                .updated_at(0)
                .build(),
        ];
        let survivors = pruned(&entries, now);
        let kept = ids(&survivors);
        assert!(!kept.contains(&"old"));
        assert!(
            kept.contains(&"referenced"),
            "a dependency still being waited on must stay"
        );
        assert!(kept.contains(&"recent"));
        assert!(kept.contains(&"waiting"));
        assert!(
            kept.contains(&"dead"),
            "a dead letter is evidence, not litter"
        );
    }

    // ── Stranding ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_dependent_of_a_dead_letter_is_stranded_and_so_is_its_own_dependent() {
        let entries = vec![
            entry("root").status(Status::FailedPermanent).build(),
            entry("child").depends_on(&["root"]).build(),
            entry("grandchild").depends_on(&["child"]).build(),
            entry("unrelated").build(),
        ];
        let stranded = stranded(&entries);
        assert!(stranded.contains("child"));
        assert!(stranded.contains("grandchild"));
        assert!(!stranded.contains("unrelated"));
    }

    #[test]
    fn a_dependency_that_is_simply_gone_strands_its_dependent() {
        let entries = vec![entry("orphan").depends_on(&["evaporated"]).build()];
        assert!(stranded(&entries).contains("orphan"));
    }

    /// A pruned-but-completed dependency is not a stranding: it finished, which is the whole point
    /// of having waited for it.
    #[test]
    fn a_completed_dependency_still_in_the_journal_does_not_strand() {
        let entries = vec![
            entry("done").status(Status::Completed).build(),
            entry("next").depends_on(&["done"]).build(),
        ];
        assert!(stranded(&entries).is_empty());
    }

    /// An edit to a task whose create dead-lettered has no dependency edge to strand on. Without
    /// this rule it waits forever for an id that is never coming.
    #[test]
    fn edits_to_a_task_whose_create_failed_are_stranded() {
        let entries = vec![
            entry("create")
                .kind(kind::CREATE_TASK)
                .status(Status::FailedPermanent)
                .temp("temp_1")
                .build(),
            entry("update")
                .kind(kind::UPDATE_TASK)
                .temp("temp_1")
                .build(),
            entry("delete")
                .kind(kind::DELETE_TASK)
                .temp("temp_1")
                .build(),
            entry("other")
                .kind(kind::UPDATE_TASK)
                .temp("temp_2")
                .build(),
        ];
        let stranded = temp_stranded(&entries);
        assert_eq!(stranded.len(), 2);
        assert!(stranded.contains("update") && stranded.contains("delete"));
    }

    #[test]
    fn edits_wait_while_their_create_is_still_trying_or_has_succeeded() {
        for status in [Status::Pending, Status::Completed, Status::Running] {
            let entries = vec![
                entry("create")
                    .kind(kind::CREATE_TASK)
                    .status(status)
                    .temp("temp_1")
                    .build(),
                entry("update")
                    .kind(kind::UPDATE_TASK)
                    .temp("temp_1")
                    .build(),
            ];
            assert!(temp_stranded(&entries).is_empty(), "for {status:?}");
        }
    }

    // ── Recovery ─────────────────────────────────────────────────────────────────────────────

    /// A crash mid-handler persists an entry as running. On relaunch it is neither runnable, nor
    /// prunable, nor stranded — it wedges, and every entry behind it in its lane wedges with it.
    #[test]
    fn a_journal_loaded_from_disk_un_wedges_whatever_was_in_flight() {
        let loaded = recovered_on_load(&[
            entry("a").status(Status::Running).build(),
            entry("b").status(Status::Pending).build(),
            entry("c").status(Status::Completed).build(),
            entry("d").status(Status::FailedPermanent).build(),
        ]);
        let status_of = |id: &str| {
            loaded
                .iter()
                .find(|entry| entry.id == id)
                .expect("present")
                .status
        };
        assert_eq!(status_of("a"), Status::Pending);
        assert_eq!(status_of("b"), Status::Pending);
        assert_eq!(status_of("c"), Status::Completed);
        assert_eq!(status_of("d"), Status::FailedPermanent);
    }
}
