//! The two loops that keep the app up to date without being asked.
//!
//! Ported in behaviour from `astrid-ios`'s `SyncManager.startAutoSync` and its SSE client. They
//! live here rather than in the FFI crate so they can be tested with a stub transport and a clock
//! a test moves by hand — a background loop that can only be observed by waiting is one nobody
//! writes a test for.
//!
//! ## Why there are two, and why neither is enough alone
//!
//! The **stream** is fast and unreliable: it delivers a change within a second of somebody else
//! making it, and it is dropped by sleeping laptops, corporate proxies and captive portals. The
//! **timer** is slow and dependable: sixty seconds, whatever the network has been doing.
//!
//! Running only the stream means an app that silently stops updating on the networks where that is
//! hardest to notice. Running only the timer means watching a colleague's edit take up to a minute
//! to appear. Everything the stream delivers also arrives on the next pass, so the timer is the
//! floor and the stream is the improvement — which is exactly why a dropped stream is not an error
//! worth showing anybody.

use std::sync::Arc;
use std::time::Duration;

use super::App;
use crate::realtime;
use crate::sync::policy;

/// How long to wait before looking again when there is no session yet.
///
/// Short enough that signing in starts the stream while the browser is still closing, long enough
/// that a signed-out app is not spinning.
const SIGNED_OUT_RETRY: Duration = Duration::from_secs(2);

/// The background sync pass.
///
/// Never runs while signed out: every request would 401, and an app sitting on a sign-in screen
/// making a request a minute is an app that looks broken in a network log.
pub async fn sync_loop(
    app: Arc<App>,
    should_continue: impl Fn() -> bool + Send,
    interval: Duration,
) {
    while should_continue() {
        tokio::time::sleep(interval).await;
        if !should_continue() {
            return;
        }
        if !app.auth.is_signed_in().await {
            continue;
        }
        let report = app.sync.sync().await;
        tracing::debug!(
            fetched = report.fetched,
            tasks_added = report.tasks_added,
            tasks_updated = report.tasks_updated,
            "background sync"
        );
    }
}

/// The default interval: sixty seconds, matching the other clients.
pub fn default_sync_interval() -> Duration {
    Duration::from_secs(policy::AUTO_SYNC_INTERVAL_SECS)
}

/// The live-update stream, kept connected.
///
/// [`realtime::stream::run`] handles reconnection within a session; this outer loop handles the
/// two things it deliberately gives up on — a missing session and a run of failures — by waiting
/// and starting over. That is the same shape as the wake-up rule in [`realtime::reconnect`]: the
/// failures that made it stop describe a world that may no longer exist.
pub async fn realtime_loop(app: Arc<App>, should_continue: impl Fn() -> bool + Send + Copy) {
    while should_continue() {
        if !app.auth.is_signed_in().await {
            tokio::time::sleep(SIGNED_OUT_RETRY).await;
            continue;
        }

        let stopped =
            realtime::stream::run(app.client.clone(), app.realtime.clone(), should_continue).await;

        match stopped {
            realtime::Stopped::Cancelled => return,
            // The session went while we were connected. Wait for a new one rather than hammering
            // an endpoint that will keep saying no.
            realtime::Stopped::Unauthorized => {
                tokio::time::sleep(SIGNED_OUT_RETRY).await;
            }
            // It gave up. The timer is still running, so the app is not stale — it is just slower
            // until the next attempt succeeds.
            realtime::Stopped::OutOfAttempts => {
                tokio::time::sleep(default_sync_interval()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{StubTransport, TransportError};
    use crate::app::Config;
    use crate::platform::{FixedClock, MemorySecureStore, SESSION_COOKIE_KEY};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn app_with(transport: StubTransport, secure: Arc<MemorySecureStore>) -> Arc<App> {
        Arc::new(
            App::with_parts(
                &Config {
                    cache_path: ":memory:".into(),
                    base_url: "https://astrid.cc".into(),
                },
                secure,
                Arc::new(transport),
                Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
            )
            .expect("starts"),
        )
    }

    /// An app on a sign-in screen making a request a minute is an app that looks broken in a
    /// network log, and every one of those requests would 401.
    #[tokio::test(start_paused = true)]
    async fn the_timer_does_nothing_while_signed_out() {
        let transport = Arc::new(StubTransport::new());
        let app = Arc::new(
            App::with_parts(
                &Config {
                    cache_path: ":memory:".into(),
                    base_url: "https://astrid.cc".into(),
                },
                Arc::new(MemorySecureStore::new()),
                transport.clone(),
                Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
            )
            .expect("starts"),
        );

        let ticks = AtomicUsize::new(0);
        sync_loop(
            app,
            || ticks.fetch_add(1, Ordering::SeqCst) < 4,
            Duration::from_secs(60),
        )
        .await;

        assert!(transport.requests().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_timer_syncs_once_there_is_a_session() {
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = Arc::new(
            StubTransport::new().fallback(Err(TransportError::Unreachable("offline".into()))),
        );
        let app = Arc::new(
            App::with_parts(
                &Config {
                    cache_path: ":memory:".into(),
                    base_url: "https://astrid.cc".into(),
                },
                secure,
                transport.clone(),
                Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
            )
            .expect("starts"),
        );

        let ticks = AtomicUsize::new(0);
        sync_loop(
            app,
            || ticks.fetch_add(1, Ordering::SeqCst) < 2,
            Duration::from_secs(60),
        )
        .await;

        assert!(
            !transport.requests().is_empty(),
            "a signed-in timer tick has to try"
        );
    }

    /// Stopping is checked before the work as well as after the sleep, so a shutdown that lands
    /// during the wait does not cost one more pass.
    #[tokio::test(start_paused = true)]
    async fn the_timer_stops_when_it_is_told_to() {
        let app = app_with(
            StubTransport::new().fallback(Err(TransportError::Timeout)),
            Arc::new(MemorySecureStore::with(SESSION_COOKIE_KEY, "cookie=abc")),
        );

        sync_loop(app, || false, Duration::from_secs(60)).await;
        // Returning at all is the assertion: a loop that ignored the flag would hang the test.
    }

    #[tokio::test(start_paused = true)]
    async fn the_stream_waits_for_a_session_rather_than_connecting_without_one() {
        let transport = Arc::new(StubTransport::new());
        let app = Arc::new(
            App::with_parts(
                &Config {
                    cache_path: ":memory:".into(),
                    base_url: "https://astrid.cc".into(),
                },
                Arc::new(MemorySecureStore::new()),
                transport.clone(),
                Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
            )
            .expect("starts"),
        );

        let ticks = std::sync::atomic::AtomicUsize::new(0);
        let keep_going = || ticks.fetch_add(1, Ordering::SeqCst) < 3;
        realtime_loop(app, &keep_going).await;

        assert!(transport.requests().is_empty());
    }
}
