//! Keeping a departing account's data out of the next person's cache.
//!
//! Signing out clears the cache. But a sync pass or a delivery drain that is already in flight is
//! holding the departing account's answers, and writes them when they arrive — into the cache that
//! was just cleared, for the next person who signs in to see. The live stream does the same with
//! any frame it has already read.
//!
//! So work that writes the account's data *enters* the session for as long as it runs, and
//! sign-out *closes* it: nothing new may start, what is running is waited for (up to a bound —
//! a request stuck on a dead network must not hold sign-out hostage), and the epoch moves on so
//! the stream can tell a frame read before the sign-out from one read after.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Default)]
pub struct SessionGate {
    closing: AtomicBool,
    busy: AtomicUsize,
    idle: tokio::sync::Notify,
    epoch: AtomicU64,
}

/// Held while a pass writes the account's data. Dropping it leaves the session.
pub struct Pass<'a> {
    gate: &'a SessionGate,
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        if self.gate.busy.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.gate.idle.notify_waiters();
        }
    }
}

impl SessionGate {
    /// Start a pass, or `None` while the session is closing — the pass should not run at all.
    pub fn enter(&self) -> Option<Pass<'_>> {
        if self.closing.load(Ordering::SeqCst) {
            return None;
        }
        self.busy.fetch_add(1, Ordering::SeqCst);
        // Closed between the look and the count: back out.
        if self.closing.load(Ordering::SeqCst) {
            drop(Pass { gate: self });
            return None;
        }
        Some(Pass { gate: self })
    }

    /// Stop new passes, move the epoch on, and wait — up to `wait` — for the running ones to finish.
    /// Answers whether they all did.
    pub async fn close(&self, wait: Duration) -> bool {
        self.closing.store(true, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let settled = async {
            loop {
                let notified = self.idle.notified();
                if self.busy.load(Ordering::SeqCst) == 0 {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(wait, settled).await.is_ok()
    }

    /// Let passes run again — once the cache has been cleared and the credentials are gone, so a
    /// new pass finds nobody signed in rather than the departing account.
    pub fn reopen(&self) {
        self.closing.store(false, Ordering::SeqCst);
    }

    /// Which session this is. A stream frame read under one epoch is not applied under another.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn closing_waits_for_the_pass_in_flight_and_refuses_new_ones() {
        let gate = std::sync::Arc::new(SessionGate::default());
        let pass = gate.enter().expect("open");
        let epoch = gate.epoch();

        let closing = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.close(Duration::from_secs(5)).await })
        };
        tokio::task::yield_now().await;
        assert!(gate.enter().is_none(), "nothing new starts while closing");
        assert_ne!(gate.epoch(), epoch);
        assert!(!closing.is_finished(), "waits for the pass in flight");

        drop(pass);
        assert!(closing.await.expect("joins"), "and settles once it is done");
        gate.reopen();
        assert!(gate.enter().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_pass_stuck_on_the_network_does_not_hold_sign_out_hostage() {
        let gate = SessionGate::default();
        let _stuck = gate.enter().expect("open");
        assert!(!gate.close(Duration::from_secs(10)).await);
    }
}
