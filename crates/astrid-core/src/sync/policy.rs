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

/// How long a pass may be left standing while the live stream is healthy.
///
/// Ten minutes. Task 5751fcb8 (finding C3 of the November platform assessment): the minute tick
/// re-fetched lists, tasks, projects — a full refetch, no delta — the feature flags and the
/// notification inbox, *while the stream was connected and would have said within a second if any
/// of them had moved*. Five sequential round trips a minute, for the whole time an app is open, to
/// learn nothing.
///
/// Ten rather than never, because the stream is an improvement and the timer is the guarantee:
/// [`super::super::app::background`] explains why neither is enough alone, and a stream that is
/// connected but silently delivering nothing is a real failure mode that only a pass can find.
pub const LIVE_SYNC_INTERVAL_SECS: u64 = 600;

/// What the live stream looked like when a tick came round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamHealth {
    /// Connected right now.
    pub live: bool,
    /// How many times it has connected since the app started.
    ///
    /// The flag alone is not enough to skip a pass on, and this is the whole reason the type has
    /// two fields: a stream that dropped and reconnected *between* two ticks reads `live: true` at
    /// both of them, having delivered nothing in the gap.
    pub connections: u64,
}

/// The pass this loop last actually ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastPass {
    /// [`StreamHealth::connections`] as it was when that pass ran.
    pub connections: u64,
    pub age_secs: u64,
}

/// Whether this tick has to fetch, or whether the stream has already covered it.
///
/// Four reasons to fetch, and the order is the argument:
///
/// 1. **Nothing has been fetched yet.** The stream only ever reports what happens from now on, so
///    a cache it alone had filled would be missing everything from before the app opened.
/// 2. **The stream is down.** Then the timer is the only floor there is, and nothing about the
///    sleeping laptops and captive portals it was chosen for has changed.
/// 3. **The stream reconnected since that pass.** Whatever happened during the gap arrived on
///    neither mechanism; this is the catch-up pass, and it is why a reconnect is counted.
/// 4. **It has been [`LIVE_SYNC_INTERVAL_SECS`].** The guarantee, kept.
pub fn tick_fetches(stream: StreamHealth, last_pass: Option<LastPass>) -> bool {
    let Some(last_pass) = last_pass else {
        return true;
    };
    !stream.live
        || stream.connections != last_pass.connections
        || last_pass.age_secs >= LIVE_SYNC_INTERVAL_SECS
}

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

    fn live(connections: u64) -> StreamHealth {
        StreamHealth {
            live: true,
            connections,
        }
    }

    /// Task 5751fcb8: while the stream is delivering, the timer is re-asking for everything the
    /// stream already said. Five sequential GETs a minute, for an account nothing had happened to.
    #[test]
    fn task_5751fcb8_a_healthy_stream_stretches_the_tick() {
        // Nothing has been fetched yet. The floor runs once whatever the stream is doing — a cache
        // the stream has only ever appended to is missing everything from before the app opened.
        assert!(tick_fetches(live(1), None));
        assert!(!tick_fetches(
            live(1),
            Some(LastPass {
                connections: 1,
                age_secs: 60,
            })
        ));
        assert!(!tick_fetches(
            live(1),
            Some(LastPass {
                connections: 1,
                age_secs: LIVE_SYNC_INTERVAL_SECS - 1,
            })
        ));
        // And it still runs eventually. The stream is the improvement; the timer is the guarantee.
        assert!(tick_fetches(
            live(1),
            Some(LastPass {
                connections: 1,
                age_secs: LIVE_SYNC_INTERVAL_SECS,
            })
        ));
    }

    /// The connection COUNT, not `is_live`, is what makes the stretch safe. A stream that dropped
    /// and reconnected between two ticks is live at both of them and delivered nothing in the gap;
    /// trusting the flag alone would lose exactly those changes, for up to ten minutes, on the
    /// networks where a drop is hardest to notice.
    #[test]
    fn task_5751fcb8_a_reconnect_since_the_last_pass_forces_one() {
        assert!(tick_fetches(
            live(2),
            Some(LastPass {
                connections: 1,
                age_secs: 1,
            })
        ));
    }

    /// With the stream down the timer is the only floor there is, so nothing changes for the
    /// sleeping laptops and captive portals this interval was chosen for.
    #[test]
    fn task_5751fcb8_a_stream_that_is_down_keeps_its_minute() {
        assert!(tick_fetches(
            StreamHealth {
                live: false,
                connections: 1,
            },
            Some(LastPass {
                connections: 1,
                age_secs: 1,
            })
        ));
    }
}
