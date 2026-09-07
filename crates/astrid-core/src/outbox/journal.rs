//! The journal on disk: entries, in order, that survive a crash.
//!
//! Ported from `astrid-ios/Astrid App/Core/Outbox/OutboxStore.swift`, with one deliberate
//! difference: the Apple journal is a file rewritten as a whole, this one is a table. A file has
//! to be read, mutated and written back for every status change, which is where its "a concurrent
//! enqueue can persist the journal mid-handler" hazard comes from — the one
//! [`super::scheduler::recovered_on_load`] exists to clean up after. A table updates the row it
//! means to, and that hazard shrinks to the genuine one: a process that dies while a request is
//! in flight.
//!
//! Ordering is by `sequence`, assigned by the database on insert, with `created_at` as the
//! human-meaningful field. Two entries enqueued in the same millisecond still have the order the
//! person performed them in.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};

use super::entry::{Entry, Status};
use crate::model::date;
use crate::store::{Result, Store, StoreError};

/// Read every entry, oldest first, with anything left `running` by a dead process reset to
/// pending. This is the only way the runner loads the journal.
pub fn load(store: &Store) -> Result<Vec<Entry>> {
    let entries = store.transaction(|connection| {
        let entries = read_all(connection)?;
        let recovered = super::scheduler::recovered_on_load(&entries);
        // Write the recovery back, so a crash loop cannot resurrect the same wedge each launch.
        for entry in &recovered {
            connection.execute(
                "UPDATE outbox SET status = ?1 WHERE id = ?2 AND status = 'running'",
                rusqlite::params![entry.status.as_str(), entry.id],
            )?;
        }
        Ok(recovered)
    })?;
    Ok(entries)
}

/// Read the journal as it stands, without the recovery pass. For stats and tests.
pub fn all(store: &Store) -> Result<Vec<Entry>> {
    store.transaction(read_all)
}

pub fn enqueue(store: &Store, entry: &Entry) -> Result<Entry> {
    store.transaction(|connection| {
        connection.execute(
            "INSERT INTO outbox (
                id, kind, payload, client_request_id, depends_on, temp_id, status, attempts,
                next_attempt_at, last_error, result, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                entry.id,
                entry.kind,
                entry.payload.to_string(),
                entry.client_request_id,
                serde_json::to_string(&entry.depends_on).unwrap_or_else(|_| "[]".into()),
                entry.temp_id,
                entry.status.as_str(),
                entry.attempts,
                date::format(entry.next_attempt_at),
                entry.last_error,
                entry
                    .result
                    .as_ref()
                    .map(|result| serde_json::to_string(result).unwrap_or_default()),
                date::format(entry.created_at),
                date::format(entry.updated_at),
            ],
        )?;
        let mut stored = entry.clone();
        stored.sequence = connection.last_insert_rowid();
        Ok(stored)
    })
}

/// Claim an entry for a handler, refusing if something else got there first.
///
/// The compare-and-set on `status = 'pending'` is what makes two runners safe: the loser sees
/// `false` and moves on rather than sending the same write twice.
pub fn mark_running(store: &Store, id: &str, now: DateTime<Utc>) -> Result<bool> {
    store.transaction(|connection| {
        let changed = connection.execute(
            "UPDATE outbox SET status = 'running', updated_at = ?2
             WHERE id = ?1 AND status = 'pending'",
            rusqlite::params![id, date::format(now)],
        )?;
        Ok(changed == 1)
    })
}

/// Record success, along with whatever the handler produced for its dependents.
pub fn mark_completed(
    store: &Store,
    id: &str,
    result: Option<&std::collections::BTreeMap<String, String>>,
    now: DateTime<Utc>,
) -> Result<()> {
    store.transaction(|connection| {
        connection.execute(
            "UPDATE outbox SET status = 'completed', last_error = NULL, result = ?2,
                               updated_at = ?3
             WHERE id = ?1",
            rusqlite::params![
                id,
                result.map(|result| serde_json::to_string(result).unwrap_or_default()),
                date::format(now),
            ],
        )?;
        Ok(())
    })
}

/// Record a failure that is worth trying again, and when to try.
pub fn mark_retry(
    store: &Store,
    id: &str,
    attempts: i64,
    next_attempt_at: DateTime<Utc>,
    error: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    store.transaction(|connection| {
        connection.execute(
            "UPDATE outbox SET status = 'pending', attempts = ?2, next_attempt_at = ?3,
                               last_error = ?4, updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![
                id,
                attempts,
                date::format(next_attempt_at),
                error,
                date::format(now)
            ],
        )?;
        Ok(())
    })
}

/// Dead-letter an entry. It stays in the journal: a write that was refused is evidence the user
/// may need, and deleting it is how "it just disappeared" happens.
pub fn mark_dead(store: &Store, id: &str, error: &str, now: DateTime<Utc>) -> Result<()> {
    store.transaction(|connection| {
        connection.execute(
            "UPDATE outbox SET status = 'failedPermanent', last_error = ?2, updated_at = ?3
             WHERE id = ?1",
            rusqlite::params![id, error, date::format(now)],
        )?;
        Ok(())
    })
}

/// Rewrite an entry's payload — used when a temporary id it names is resolved to a real one.
pub fn update_payload(
    store: &Store,
    id: &str,
    payload: &serde_json::Value,
    now: DateTime<Utc>,
) -> Result<()> {
    store.transaction(|connection| {
        connection.execute(
            "UPDATE outbox SET payload = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, payload.to_string(), date::format(now)],
        )?;
        Ok(())
    })
}

/// Drop the completed entries nothing needs any more.
pub fn prune(store: &Store, now: DateTime<Utc>) -> Result<usize> {
    store.transaction(|connection| {
        let entries = read_all(connection)?;
        let keep: std::collections::HashSet<String> = super::scheduler::pruned(&entries, now)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        let mut removed = 0;
        for entry in entries {
            if !keep.contains(&entry.id) {
                connection.execute("DELETE FROM outbox WHERE id = ?1", [&entry.id])?;
                removed += 1;
            }
        }
        Ok(removed)
    })
}

pub fn entry(store: &Store, id: &str) -> Result<Option<Entry>> {
    store.transaction(|connection| {
        let mut statement =
            connection.prepare(&format!("SELECT {COLUMNS} FROM outbox WHERE id = ?1"))?;
        let found = statement.query_row([id], read_entry).optional()?;
        found.transpose()
    })
}

/// Remove an entry outright. Only sign-out and the tests do this; ordinary life dead-letters.
pub fn remove(store: &Store, id: &str) -> Result<()> {
    store.transaction(|connection| {
        connection.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// How the queue is doing, for the status the shell shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
}

impl Stats {
    /// Whether there is unsent work. What "not synced yet" means in the UI.
    pub fn has_unsent_work(&self) -> bool {
        self.pending > 0 || self.running > 0
    }
}

pub fn stats(store: &Store) -> Result<Stats> {
    let entries = all(store)?;
    let mut stats = Stats::default();
    for entry in entries {
        match entry.status {
            Status::Pending => stats.pending += 1,
            Status::Running => stats.running += 1,
            Status::Completed => stats.completed += 1,
            Status::FailedPermanent => stats.failed += 1,
        }
    }
    Ok(stats)
}

const COLUMNS: &str = "id, kind, payload, client_request_id, depends_on, temp_id, status, \
     attempts, next_attempt_at, last_error, result, created_at, updated_at, sequence";

fn read_all(connection: &Connection) -> Result<Vec<Entry>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {COLUMNS} FROM outbox ORDER BY sequence ASC"
    ))?;
    let rows = statement.query_map([], read_entry)?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row??);
    }
    Ok(entries)
}

/// Reading a row is fallible twice over: the database can fail, and so can the JSON inside it.
/// The two are kept apart so a single unreadable payload is reported as corruption rather than as
/// a database error nobody can act on.
fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Entry>> {
    let payload: String = row.get(2)?;
    let depends_on: String = row.get(4)?;
    let status: String = row.get(6)?;
    let next_attempt_at: String = row.get(8)?;
    let result: Option<String> = row.get(10)?;
    let created_at: String = row.get(11)?;
    let updated_at: String = row.get(12)?;

    let parsed = (|| -> Result<Entry> {
        Ok(Entry {
            id: row_text(row, 0)?,
            kind: row_text(row, 1)?,
            payload: serde_json::from_str(&payload)
                .map_err(|error| StoreError::Corrupt(error.to_string()))?,
            client_request_id: row_text(row, 3)?,
            depends_on: serde_json::from_str(&depends_on).unwrap_or_default(),
            temp_id: row_optional_text(row, 5)?,
            status: Status::from_wire(&status),
            attempts: row_int(row, 7)?,
            next_attempt_at: date::parse(&next_attempt_at).ok_or_else(|| {
                StoreError::Corrupt(format!("unreadable next_attempt_at: {next_attempt_at}"))
            })?,
            last_error: row_optional_text(row, 9)?,
            result: result.and_then(|result| serde_json::from_str(&result).ok()),
            created_at: date::parse(&created_at).ok_or_else(|| {
                StoreError::Corrupt(format!("unreadable created_at: {created_at}"))
            })?,
            updated_at: date::parse(&updated_at).unwrap_or_else(Utc::now),
            sequence: row_int(row, 13)?,
        })
    })();
    Ok(parsed)
}

fn row_text(row: &rusqlite::Row<'_>, index: usize) -> Result<String> {
    Ok(row.get(index)?)
}

fn row_optional_text(row: &rusqlite::Row<'_>, index: usize) -> Result<Option<String>> {
    Ok(row.get(index)?)
}

fn row_int(row: &rusqlite::Row<'_>, index: usize) -> Result<i64> {
    Ok(row.get(index)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::entry::kind;
    use chrono::Duration;

    fn t0() -> DateTime<Utc> {
        date::parse("2026-09-07T12:00:00Z").expect("an instant")
    }

    fn new_entry(id: &str) -> Entry {
        Entry::new(
            id,
            kind::CREATE_TASK,
            serde_json::json!({ "title": "Buy milk" }),
            format!("temp_{id}"),
            t0(),
        )
    }

    #[test]
    fn an_entry_round_trips_through_the_journal() {
        let store = Store::in_memory().expect("opens");
        let stored = enqueue(&store, &new_entry("e1")).expect("enqueues");
        assert!(stored.sequence > 0);

        let read = entry(&store, "e1").expect("reads").expect("present");
        assert_eq!(read.kind, kind::CREATE_TASK);
        assert_eq!(read.payload["title"], "Buy milk");
        assert_eq!(read.client_request_id, "temp_e1");
        assert_eq!(read.status, Status::Pending);
        assert_eq!(read.next_attempt_at, t0());
    }

    /// The order the person performed the writes in, even inside one millisecond.
    #[test]
    fn entries_come_back_in_the_order_they_were_enqueued() {
        let store = Store::in_memory().expect("opens");
        for id in ["a", "b", "c"] {
            enqueue(&store, &new_entry(id)).expect("enqueues");
        }
        let ids: Vec<String> = all(&store)
            .expect("reads")
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    /// Two runners, one entry. The loser has to see that it lost, or the write is sent twice.
    #[test]
    fn only_one_claim_on_an_entry_succeeds() {
        let store = Store::in_memory().expect("opens");
        enqueue(&store, &new_entry("e1")).expect("enqueues");
        assert!(mark_running(&store, "e1", t0()).expect("claims"));
        assert!(!mark_running(&store, "e1", t0()).expect("claims"));
    }

    #[test]
    fn a_retry_records_when_and_why() {
        let store = Store::in_memory().expect("opens");
        enqueue(&store, &new_entry("e1")).expect("enqueues");
        mark_running(&store, "e1", t0()).expect("claims");
        mark_retry(
            &store,
            "e1",
            1,
            t0() + Duration::seconds(2),
            "500 the server fell over",
            t0(),
        )
        .expect("records");

        let read = entry(&store, "e1").expect("reads").expect("present");
        assert_eq!(read.status, Status::Pending);
        assert_eq!(read.attempts, 1);
        assert_eq!(read.next_attempt_at, t0() + Duration::seconds(2));
        assert!(read.last_error.expect("an error").contains("500"));
    }

    /// A refused write is evidence. Deleting it is how "it just disappeared" happens.
    #[test]
    fn a_dead_letter_stays_in_the_journal_with_its_reason() {
        let store = Store::in_memory().expect("opens");
        enqueue(&store, &new_entry("e1")).expect("enqueues");
        mark_dead(&store, "e1", "403 not yours", t0()).expect("records");

        let read = entry(&store, "e1").expect("reads").expect("present");
        assert_eq!(read.status, Status::FailedPermanent);
        assert_eq!(read.last_error.as_deref(), Some("403 not yours"));
    }

    #[test]
    fn a_completion_carries_what_dependents_need() {
        let store = Store::in_memory().expect("opens");
        enqueue(&store, &new_entry("e1")).expect("enqueues");
        let result =
            std::collections::BTreeMap::from([("taskId".to_string(), "cm3real".to_string())]);
        mark_completed(&store, "e1", Some(&result), t0()).expect("records");

        let read = entry(&store, "e1").expect("reads").expect("present");
        assert_eq!(read.status, Status::Completed);
        assert_eq!(read.result.expect("a result")["taskId"], "cm3real");
    }

    /// The crash case: a process that died mid-request left an entry claimed. On the next launch
    /// it has to become runnable again, and the fix has to be written back so a crash loop cannot
    /// rebuild the same wedge every time.
    #[test]
    fn loading_the_journal_frees_whatever_was_in_flight() {
        let store = Store::in_memory().expect("opens");
        enqueue(&store, &new_entry("e1")).expect("enqueues");
        mark_running(&store, "e1", t0()).expect("claims");

        let loaded = load(&store).expect("loads");
        assert_eq!(loaded[0].status, Status::Pending);
        assert_eq!(
            all(&store).expect("reads")[0].status,
            Status::Pending,
            "the recovery has to be persisted, not just returned"
        );
    }

    #[test]
    fn pruning_keeps_what_is_still_referenced_and_drops_what_is_not() {
        let store = Store::in_memory().expect("opens");
        enqueue(&store, &new_entry("old")).expect("enqueues");
        enqueue(&store, &new_entry("referenced")).expect("enqueues");
        let mut waiting = new_entry("waiting");
        waiting.depends_on = vec!["referenced".to_string()];
        enqueue(&store, &waiting).expect("enqueues");

        mark_completed(&store, "old", None, t0()).expect("records");
        mark_completed(&store, "referenced", None, t0()).expect("records");

        let later = t0() + Duration::seconds(super::super::scheduler::COMPLETED_RETENTION_SECS * 2);
        assert_eq!(prune(&store, later).expect("prunes"), 1);
        assert!(entry(&store, "old").expect("reads").is_none());
        assert!(entry(&store, "referenced").expect("reads").is_some());
    }

    #[test]
    fn the_stats_say_whether_anything_is_still_unsent() {
        let store = Store::in_memory().expect("opens");
        assert!(!stats(&store).expect("reads").has_unsent_work());

        enqueue(&store, &new_entry("e1")).expect("enqueues");
        enqueue(&store, &new_entry("e2")).expect("enqueues");
        mark_dead(&store, "e2", "403", t0()).expect("records");

        let stats = stats(&store).expect("reads");
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.failed, 1);
        assert!(stats.has_unsent_work());
    }
}
