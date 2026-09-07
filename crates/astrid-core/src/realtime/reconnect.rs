//! When the live stream should try again, and how long it waits.
//!
//! Ported from `astrid-ios/Astrid App/Core/RealTime/SSEReconnectPolicy.swift`, including the bug
//! that shaped it.
//!
//! The stream gives up after a bounded number of attempts, which is right for a transient failure
//! and wrong for a machine that has been **asleep**: every attempt burns while it is offline, and
//! once they are gone nothing ever revives the connection — live updates stay dead until the app is
//! relaunched. So waking, or regaining the network, **resets** the count rather than continuing
//! the old one. The previous failures describe a world that no longer exists.
//!
//! On Windows this matters more than it did on a Mac, not less: a laptop lid closes several times
//! a day, and modern standby wakes and sleeps the network without waking the user.

use std::time::Duration;

/// How many times in a row the stream may fail before it stops trying.
pub const MAX_ATTEMPTS: u32 = 5;

/// The longest wait between attempts. Capped so a long outage does not push the next try minutes
/// away, by which time the user has usually given up and relaunched.
pub const MAX_DELAY_SECS: u64 = 60;

/// How long to wait before attempt number `attempt` (1-based).
pub fn delay(attempt: u32) -> Duration {
    let exponent = attempt.clamp(1, 16);
    Duration::from_secs((1u64 << exponent).min(MAX_DELAY_SECS))
}

/// Whether to try again after `attempts` consecutive failures.
pub fn should_retry(attempts: u32) -> bool {
    attempts < MAX_ATTEMPTS
}

/// Whether a given HTTP status is worth retrying.
///
/// A 401 means the session is gone; reconnecting cannot help, and hammering an endpoint that will
/// keep saying no is how a client gets rate-limited on top of being signed out.
pub fn should_retry_after_status(status: u16) -> bool {
    status != 401
}

/// The attempt count to resume from after a wake or a network change.
pub fn attempts_after_recovery() -> u32 {
    0
}

/// The policy as a small piece of state, for a caller that would otherwise track the count itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReconnectPolicy {
    attempts: u32,
}

impl ReconnectPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a failure and say how long to wait, or `None` to stop trying.
    pub fn failed(&mut self) -> Option<Duration> {
        self.attempts += 1;
        should_retry(self.attempts).then(|| delay(self.attempts))
    }

    /// Record a status-carrying failure. A 401 stops the stream whatever the count says.
    pub fn failed_with_status(&mut self, status: u16) -> Option<Duration> {
        if !should_retry_after_status(status) {
            self.attempts = MAX_ATTEMPTS;
            return None;
        }
        self.failed()
    }

    /// A connection succeeded.
    pub fn connected(&mut self) {
        self.attempts = attempts_after_recovery();
    }

    /// The machine woke, or the network came back. The old count describes a world that is gone.
    pub fn recovered(&mut self) {
        self.attempts = attempts_after_recovery();
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn has_given_up(&self) -> bool {
        !should_retry(self.attempts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_grows_and_is_capped() {
        assert!(delay(1) < delay(2));
        assert!(delay(2) < delay(3));
        assert_eq!(delay(99), Duration::from_secs(MAX_DELAY_SECS));
    }

    #[test]
    fn it_stops_after_its_attempts_are_gone() {
        let mut policy = ReconnectPolicy::new();
        for _ in 0..MAX_ATTEMPTS - 1 {
            assert!(policy.failed().is_some());
        }
        assert!(policy.failed().is_none());
        assert!(policy.has_given_up());
    }

    /// The bug this policy exists for: a machine asleep for an hour burns every attempt on a
    /// network that is not there, and without a reset the stream never comes back at all.
    #[test]
    fn waking_up_starts_the_count_over() {
        let mut policy = ReconnectPolicy::new();
        for _ in 0..MAX_ATTEMPTS {
            policy.failed();
        }
        assert!(policy.has_given_up());

        policy.recovered();
        assert!(!policy.has_given_up());
        assert_eq!(policy.attempts(), 0);
        assert!(policy.failed().is_some());
    }

    #[test]
    fn a_successful_connection_forgets_the_failures_before_it() {
        let mut policy = ReconnectPolicy::new();
        policy.failed();
        policy.failed();
        policy.connected();
        assert_eq!(policy.attempts(), 0);
    }

    /// Reconnecting cannot fix a session that is gone, and trying gets the client rate-limited on
    /// top of being signed out.
    #[test]
    fn an_expired_session_stops_the_stream_rather_than_retrying() {
        let mut policy = ReconnectPolicy::new();
        assert!(policy.failed_with_status(401).is_none());
        assert!(policy.has_given_up());

        let mut other = ReconnectPolicy::new();
        assert!(other.failed_with_status(503).is_some());
    }
}
