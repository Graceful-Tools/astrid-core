//! The seams the shell fills in.
//!
//! `astrid-core` has no Windows dependency. Everything that needs one — the credential vault,
//! notifications, network reachability, the file system — arrives here as a trait the shell
//! implements. That keeps the crate testable on any machine and, more usefully, keeps the platform
//! boundary *visible*: if a rule ends up needing something from this module, it is a rule the
//! shell can no longer be asked to decide.
//!
//! Every trait ships with an in-memory implementation. Those are not test scaffolding to be
//! deleted later — the shell's UI-test build uses them so a UI test can never reach the real
//! credential vault or the real network, which is a hardening the Apple repo had to add after the
//! fact.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// The key the session `Cookie` header is stored under. One name, used by the client and by
/// sign-in, so a rename cannot sign everyone out.
pub const SESSION_COOKIE_KEY: &str = "astrid.session-cookie";

/// The signed-in user's id, cached beside the credential so the app knows who it is before its
/// first request comes back.
pub const CURRENT_USER_ID_KEY: &str = "astrid.current-user-id";

#[derive(Debug, Clone, thiserror::Error)]
pub enum PlatformError {
    #[error("secure storage is unavailable: {0}")]
    Unavailable(String),
    #[error("secure storage refused the write: {0}")]
    Denied(String),
}

/// Credentials at rest. On Windows this is the Credential Locker; in tests and in the UI-test
/// build it is [`MemorySecureStore`].
///
/// Async because the Windows implementation may be called from a background thread and
/// `PasswordVault` is not free to touch — see the M0 spike in `docs/M0_NOTES.md`.
#[async_trait]
pub trait SecureStore: Send + Sync {
    async fn get(&self, key: &str) -> Option<String>;
    async fn set(&self, key: &str, value: &str) -> Result<(), PlatformError>;
    async fn delete(&self, key: &str) -> Result<(), PlatformError>;
}

/// A store that forgets everything when the process ends.
#[derive(Debug, Default)]
pub struct MemorySecureStore {
    entries: Mutex<HashMap<String, String>>,
}

impl MemorySecureStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-seed a value, for a test that starts signed in.
    pub fn with(key: &str, value: &str) -> Self {
        let store = Self::new();
        store
            .entries
            .lock()
            .expect("store lock")
            .insert(key.to_string(), value.to_string());
        store
    }
}

#[async_trait]
impl SecureStore for MemorySecureStore {
    async fn get(&self, key: &str) -> Option<String> {
        self.entries.lock().expect("store lock").get(key).cloned()
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), PlatformError> {
        self.entries
            .lock()
            .expect("store lock")
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), PlatformError> {
        self.entries.lock().expect("store lock").remove(key);
        Ok(())
    }
}

/// The current time, as a seam.
///
/// Every rule that compares against "now" — retry backoff, the recently-completed window, whether
/// a task is overdue — reads it from here. A test that has to sleep to observe a scheduler is a
/// test that will be flaky on a loaded CI machine, and one that cannot reach a leap day at all.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock a test moves by hand.
#[derive(Debug)]
pub struct FixedClock {
    now: Mutex<DateTime<Utc>>,
}

impl FixedClock {
    pub fn at(now: DateTime<Utc>) -> Self {
        FixedClock {
            now: Mutex::new(now),
        }
    }

    /// Parse-or-panic, for the many tests that want a readable literal.
    pub fn parsed(instant: &str) -> Self {
        Self::at(crate::model::date::parse(instant).expect("a valid instant in a test"))
    }

    pub fn advance(&self, by: chrono::Duration) {
        let mut now = self.now.lock().expect("clock lock");
        *now += by;
    }

    pub fn set(&self, to: DateTime<Utc>) {
        *self.now.lock().expect("clock lock") = to;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("clock lock")
    }
}

/// Whether the machine believes it can reach the network.
///
/// Advisory only, and treated as such everywhere: the Outbox attempts a send when this says
/// online and *also* recovers when it is wrong, because a captive portal, a VPN coming up and a
/// sleeping laptop all report "connected" while nothing gets through.
pub trait Reachability: Send + Sync {
    fn is_online(&self) -> bool;
}

/// Reachability for the paths that have no opinion — tests, and the periods before the shell has
/// wired the real monitor up.
#[derive(Debug)]
pub struct AssumeOnline(pub bool);

impl Default for AssumeOnline {
    fn default() -> Self {
        AssumeOnline(true)
    }
}

impl Reachability for AssumeOnline {
    fn is_online(&self) -> bool {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_memory_store_round_trips_and_forgets() {
        let store = MemorySecureStore::new();
        assert_eq!(store.get(SESSION_COOKIE_KEY).await, None);
        store
            .set(SESSION_COOKIE_KEY, "next-auth.session-token=abc")
            .await
            .expect("stores");
        assert_eq!(
            store.get(SESSION_COOKIE_KEY).await.as_deref(),
            Some("next-auth.session-token=abc")
        );
        store.delete(SESSION_COOKIE_KEY).await.expect("deletes");
        assert_eq!(store.get(SESSION_COOKIE_KEY).await, None);
    }

    #[test]
    fn a_test_clock_moves_only_when_the_test_says_so() {
        let clock = FixedClock::parsed("2026-09-07T12:00:00Z");
        let before = clock.now();
        assert_eq!(clock.now(), before);
        clock.advance(chrono::Duration::hours(2));
        assert_eq!(
            crate::model::date::format(clock.now()),
            "2026-09-07T14:00:00Z"
        );
    }
}
