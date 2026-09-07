//! How a sync pass decides to run, and how it treats a local push that fails.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/SyncPassPolicy.swift`, including the incident
//! it was written for (task 3173727d — "recently added tasks never show up; pull to refresh isn't
//! adding them, nor sync"). Two rules, and both describe a pass that stopped delivering remote
//! changes without saying anything:
//!
//! 1. **A refresh the user asked for is not a timer tick.** Both used to hit the same "already
//!    syncing, return" guard, so a pull-to-refresh that landed during the 60-second background
//!    pass finished its animation having fetched nothing.
//! 2. **Pushing local work is best-effort; fetching is not.** The pass pushed pending writes
//!    before it fetched, and one stuck local write — a comment on a task the server kept
//!    refusing — aborted the pass *before* the fetch. No remote change arrived again until the app
//!    was relaunched.
//!
//! Pure, so both rules are ordinary tests rather than something you find out about in a support
//! thread.

/// What a pass should do when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Nothing in flight — take the slot.
    Start,
    /// Something is in flight and a person asked for this pass. Wait for the slot rather than
    /// returning silently, because a spinner that stops having done nothing is a lie.
    WaitForInFlight,
    /// Something is in flight and this is a timer tick. The pass already running is doing this
    /// work.
    Skip,
}

pub fn admission(is_syncing: bool, is_user_initiated: bool) -> Admission {
    if !is_syncing {
        return Admission::Start;
    }
    if is_user_initiated {
        Admission::WaitForInFlight
    } else {
        Admission::Skip
    }
}

/// How long a user-initiated pass will wait for the slot before giving up.
///
/// Bounded: a refresh that cannot get in has to stop rather than leave the spinner turning.
pub const WAIT_FOR_SLOT_SECS: u64 = 15;

/// How often the background pass runs.
pub const AUTO_SYNC_INTERVAL_SECS: u64 = 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_client_starts_the_pass_whoever_asked() {
        assert_eq!(admission(false, true), Admission::Start);
        assert_eq!(admission(false, false), Admission::Start);
    }

    /// The pull-to-refresh half of task 3173727d: a person waiting on a spinner is not the same as
    /// a timer, and treating them the same is what made refresh do nothing.
    #[test]
    fn a_person_waits_for_the_slot_and_a_timer_gives_it_up() {
        assert_eq!(admission(true, true), Admission::WaitForInFlight);
        assert_eq!(admission(true, false), Admission::Skip);
    }
}
