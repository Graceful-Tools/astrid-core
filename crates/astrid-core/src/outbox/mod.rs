//! The write journal — the only path a change takes to the server.
//!
//! Ported from `astrid-ios/Astrid App/Core/Outbox/`. Rule 5 of `docs/ASTRID.md` §0 says nothing
//! bypasses it, and the reason is the shape of the app rather than a preference: a task typed on a
//! train has to exist, be editable, survive a relaunch, and arrive — in the order it was made —
//! whenever the network comes back. A service that wrote straight to the API would have to invent
//! that story again per operation, which is what the Apple apps did before this existed: four
//! per-service pending queues, four sets of retry rules, and four different ways to lose a write.
//!
//! The pieces, in the order they matter:
//!
//! - [`entry`] — one self-contained unit of work, with its idempotency key and its dependencies.
//! - [`scheduler`] — pure policy: backoff, giving up, ordering, lanes, stranding. No I/O, so the
//!   highest-risk logic in the crate is also the most testable.
//! - [`journal`] — the entries on disk, in order, surviving a crash.
//! - [`handlers`] — what each kind does when its turn comes.
//! - [`runner`] — the drain loop that puts those together.

pub mod entry;
pub mod handlers;
pub mod journal;
pub mod runner;
pub mod scheduler;

pub use entry::{kind, Entry, Status};
pub use journal::Stats;
pub use runner::{DrainReport, Runner};

use chrono::{DateTime, Utc};

/// Mint an id for something that exists only on this device so far.
///
/// The `temp_` prefix is load-bearing: [`crate::model::is_temp_id`] reads it, the runner rewrites
/// payloads on it, and the shell shows a row carrying one as not yet synced. It doubles as the
/// idempotency key, so the id a task is created under is the key that stops a retry making a
/// second one.
pub fn new_temp_id() -> String {
    format!("{}{}", crate::model::TEMP_ID_PREFIX, uuid::Uuid::new_v4())
}

/// A fresh entry id. Not a temp id: it names the journal row, not a task.
pub fn new_entry_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Enqueue one entry, filling in the ids and timestamps that are the same every time.
pub fn build(
    kind: &str,
    payload: serde_json::Value,
    client_request_id: &str,
    now: DateTime<Utc>,
) -> Entry {
    Entry::new(new_entry_id(), kind, payload, client_request_id, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prefix is read by the model, the runner and the shell. A temp id without it is a row
    /// that renders as synced and an edit that is never rewritten.
    #[test]
    fn a_temp_id_is_recognisable_as_one() {
        let id = new_temp_id();
        assert!(crate::model::is_temp_id(&id));
        assert_ne!(id, new_temp_id());
    }

    #[test]
    fn an_entry_id_is_not_a_temp_id() {
        assert!(!crate::model::is_temp_id(&new_entry_id()));
    }
}
