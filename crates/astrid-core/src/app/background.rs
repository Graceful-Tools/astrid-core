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
        let report = app.sync.sync_in_background().await;
        tracing::debug!(
            fetched = report.fetched,
            skipped = report.skipped,
            tasks_added = report.tasks_added,
            tasks_updated = report.tasks_updated,
            tasks_deleted = report.tasks_deleted,
            "background sync"
        );
        // The cache moved, so the screen has to. Without this the timer was a floor for the
        // cache and not for the person looking at it: a change that arrived while the stream was
        // down sat in SQLite until they happened to click something.
        if report.changed_anything() {
            app.realtime().publish(crate::realtime::Change::Synced {
                task_ids: report.changed_task_ids,
                list_ids: report.changed_list_ids,
            });
        }
        // The inbox rides on the same tick: the web sends no live event for it. Best effort —
        // a deployment without the route, or a pass that could not reach the server, leaves the
        // cached inbox standing — and announced only when it differs from what was cached.
        if report.fetched {
            let notifications = app.context.notifications();
            let before = notifications.inbox().unwrap_or_default();
            if let Ok(after) = notifications.refresh().await {
                if after != before {
                    app.realtime()
                        .publish(crate::realtime::Change::Notifications);
                }
            }
        }
    }
}

/// Deliver what the Outbox holds: as soon as something is journalled, and again when a retry
/// comes due.
///
/// The sync pass drains the journal too, at the top of every pass — and until this loop existed
/// that was the *only* time it drained, so a task added here took up to sixty seconds to exist
/// anywhere else. Now [`super::App::run`] rings `nudge` after any command that leaves something
/// pending, and this wakes, sends it, and tells the shell what moved: a temporary id becoming a
/// real one is a change the rows have to see.
///
/// A drain that could not deliver — no network — leaves the entries scheduled with a backoff,
/// and the next wait is until the earliest of them; nothing here spins.
pub async fn outbox_loop(
    app: Arc<App>,
    should_continue: impl Fn() -> bool + Send,
    nudge: Arc<tokio::sync::Notify>,
) {
    while should_continue() {
        let wait = next_delivery_wait(&app);
        tokio::select! {
            _ = nudge.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
        if !should_continue() {
            return;
        }
        if !app.auth.is_signed_in().await {
            continue;
        }
        match app.runner.drain().await {
            Ok(report) => {
                tracing::debug!(
                    completed = report.completed,
                    retried = report.retried,
                    dead_lettered = report.dead_lettered,
                    "outbox delivery"
                );
                // Only when a row's fate is settled. A retry that failed again changed nothing on
                // screen, and announcing it would redraw the window on every backoff tick.
                if report.completed > 0 || report.dead_lettered > 0 {
                    app.realtime().publish(crate::realtime::Change::Synced {
                        task_ids: Vec::new(),
                        list_ids: Vec::new(),
                    });
                }
            }
            Err(error) => tracing::debug!(%error, "outbox delivery failed"),
        }
    }
}

/// How long the delivery loop may sleep before it must look at the journal again.
///
/// Until the earliest scheduled retry; at once if something is runnable now (a nudge may have
/// arrived while a drain was running, which `Notify` folds into one); and otherwise the sync
/// interval, as a floor that costs one lookup a minute.
fn next_delivery_wait(app: &App) -> Duration {
    match app.runner.next_wakeup() {
        Ok(Some(at)) => (at - app.clock.now()).to_std().unwrap_or(Duration::ZERO),
        Ok(None) => match crate::outbox::journal::has_pending(&app.store) {
            Ok(true) => Duration::ZERO,
            _ => default_sync_interval(),
        },
        Err(_) => default_sync_interval(),
    }
}

/// How often to look for a reminder that has come due.
///
/// Half a minute. A reminder is a promise about a time, and a minute's slack on "9:00" is the
/// difference between useful and annoying; polling the cache is a SQLite read of tasks already in
/// memory, so this costs nothing to do often.
pub const REMINDER_INTERVAL: Duration = Duration::from_secs(30);

/// Watch for reminders coming due while the app runs.
///
/// The server owns push and email — it knows about quiet hours, digests and every device somebody
/// owns. This is only the thing a running client can do that the server cannot: notice that a
/// reminder has arrived for the app that is open in front of them. It announces; showing a banner
/// and deciding what a banner even is belongs to the shell.
pub async fn reminder_loop(
    app: Arc<App>,
    should_continue: impl Fn() -> bool + Send,
    interval: Duration,
) {
    while should_continue() {
        tokio::time::sleep(interval).await;
        if !should_continue() {
            return;
        }
        let Ok(tasks) = app.store.tasks() else {
            continue;
        };
        let now = app.clock.now();
        let due = crate::reminders::due_now(&tasks, now, |id| {
            let Some(task) = tasks.iter().find(|task| task.id == id) else {
                return false;
            };
            let Some(at) = task.reminder_time else {
                return false;
            };
            app.store
                .metadata(&format!("reminder.shown.{id}"))
                .ok()
                .flatten()
                .is_some_and(|stamp| stamp == at.to_rfc3339())
        });
        if !due.is_empty() {
            app.realtime()
                .publish(crate::realtime::Change::RemindersDue);
        }
    }
}

/// How often to mirror the Google-linked lists.
///
/// Five minutes. Google Tasks has no webhooks, so a client polls — "on foreground/nudge", as the
/// route puts it — and this is the nudge. A minute would be four extra round trips an hour for a
/// system nobody edits from two places in the same minute.
pub const EXTERNAL_INTERVAL: Duration = Duration::from_secs(300);

/// Mirror the Google-linked lists, while the app runs.
///
/// Only Google: GitHub is a cron on the server, so a list linked to a repository syncs whether or
/// not this app is open. Never while signed out, for the same reason the sync pass is not.
pub async fn external_loop(
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
        let external = app.context.external();
        // Whether anything on this machine is different afterwards. The passes report counts,
        // not ids, so what is announced is "something moved" and the shell refreshes what it
        // has on screen.
        let mut cache_moved = false;
        // Before the passes: a list made on either side since the last round gets its counterpart
        // and is then synced in the same round. A no-op in manual mode, which is the default.
        let auto_linked = external.auto_link_google().await;
        match &auto_linked {
            Ok(report) if report.linked > 0 || report.lists_created > 0 => {
                cache_moved = true;
                tracing::debug!(linked = report.linked, "auto-linked")
            }
            Ok(_) => {}
            Err(error) => tracing::debug!(%error, "auto-link failed"),
        }
        // My Tasks against Google's default list, when the mode asks for it.
        if let Ok(Some(container)) = auto_linked.map(|report| report.my_tasks_container) {
            match external.sync_my_tasks(&container).await {
                Ok(report) => cache_moved |= report.applied > 0 || report.deleted_locally > 0,
                Err(error) => tracing::debug!(%error, "My Tasks pass failed"),
            }
        }
        let Ok(links) = external.links(crate::services::Provider::GoogleTasks).await else {
            continue;
        };
        for link in &links {
            match external.sync_google_link(link).await {
                Ok(report) => {
                    cache_moved |= report.applied > 0 || report.deleted_locally > 0;
                    tracing::debug!(
                        link = %link.id,
                        applied = report.applied,
                        pushed = report.pushed,
                        "external pass"
                    )
                }
                // One list failing is not the others failing.
                Err(error) => tracing::debug!(link = %link.id, %error, "external pass failed"),
            }
        }
        if cache_moved {
            app.realtime().publish(crate::realtime::Change::Synced {
                task_ids: Vec::new(),
                list_ids: Vec::new(),
            });
        }
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

    /// A reminder coming due announces itself once, through the same subscription a colleague's
    /// edit arrives on. It does not mark itself shown — the shell does that when a banner is
    /// actually on screen, because a banner that failed to appear must still be owed.
    #[tokio::test(start_paused = true)]
    async fn a_reminder_coming_due_is_announced() {
        let app = app_with(StubTransport::new(), Arc::new(MemorySecureStore::new()));
        let mut task = crate::model::Task::new("t1", "Call the vet");
        task.reminder_time = crate::model::date::parse("2026-09-07T11:59:00Z");
        app.store.upsert_task(&task).expect("stores");

        let heard = Arc::new(AtomicUsize::new(0));
        {
            let heard = heard.clone();
            app.realtime().on_change(move |change| {
                if matches!(change, crate::realtime::Change::RemindersDue) {
                    heard.fetch_add(1, Ordering::SeqCst);
                }
            });
        }

        let ticks = AtomicUsize::new(0);
        reminder_loop(
            app,
            || ticks.fetch_add(1, Ordering::SeqCst) < 2,
            Duration::from_secs(30),
        )
        .await;

        assert!(heard.load(Ordering::SeqCst) >= 1);
    }

    /// Nothing due, nothing said. A loop that announced every tick would wake the shell twice a
    /// minute for the rest of the day.
    #[tokio::test(start_paused = true)]
    async fn nothing_due_says_nothing() {
        let app = app_with(StubTransport::new(), Arc::new(MemorySecureStore::new()));
        app.store
            .upsert_task(&crate::model::Task::new("t1", "Call the vet"))
            .expect("stores");

        let heard = Arc::new(AtomicUsize::new(0));
        {
            let heard = heard.clone();
            app.realtime().on_change(move |_| {
                heard.fetch_add(1, Ordering::SeqCst);
            });
        }

        let ticks = AtomicUsize::new(0);
        reminder_loop(
            app,
            || ticks.fetch_add(1, Ordering::SeqCst) < 3,
            Duration::from_secs(30),
        )
        .await;

        assert_eq!(heard.load(Ordering::SeqCst), 0);
    }

    /// A write goes out when it is made, not on the next timer tick. The command rings the
    /// bell, the loop wakes without any time passing, and the shell hears that the rows moved.
    #[tokio::test(start_paused = true)]
    async fn a_journalled_write_goes_out_at_once_rather_than_on_the_next_tick() {
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = Arc::new(
            StubTransport::new()
                .push_json(
                    "/api/v1/tasks",
                    200,
                    serde_json::json!({ "task": { "id": "t-real", "title": "Buy milk" } }),
                )
                .fallback(Err(TransportError::Unreachable("done".into()))),
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
        let heard = Arc::new(AtomicUsize::new(0));
        {
            let heard = heard.clone();
            app.realtime().on_change(move |change| {
                if matches!(change, crate::realtime::Change::Synced { .. }) {
                    heard.fetch_add(1, Ordering::SeqCst);
                }
            });
        }

        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let looping = {
            let (app, running) = (app.clone(), running.clone());
            tokio::spawn(outbox_loop(
                app.clone(),
                move || running.load(Ordering::SeqCst),
                app.outbox_nudge().clone(),
            ))
        };

        let started = tokio::time::Instant::now();
        app.run(crate::app::Command::CreateTask {
            title: "Buy milk".into(),
            description: None,
            list_ids: Vec::new(),
            priority: None,
            due_date_time: None,
            is_all_day: None,
            assignee_id: None,
            parent_task_id: None,
            quick_add: false,
        })
        .await;
        // Let the loop take its turn. No time passes: the wake-up is the bell, not a timer.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert!(
            transport
                .requests()
                .iter()
                .any(|r| r.method.as_str() == "POST" && r.url.ends_with("/api/v1/tasks")),
            "the create went out on the nudge"
        );
        assert_eq!(started.elapsed(), Duration::ZERO, "and not on a timer");
        assert_eq!(
            heard.load(Ordering::SeqCst),
            1,
            "the shell hears the rows moved"
        );

        running.store(false, Ordering::SeqCst);
        app.outbox_nudge().notify_one();
        let _ = looping.await;
    }

    /// The timer is the floor for the screen, not only for the cache. A pass that brought
    /// something in says so on the same subscription a colleague's edit arrives on, naming what
    /// moved.
    #[tokio::test(start_paused = true)]
    async fn a_background_pass_that_brought_something_in_is_announced() {
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = StubTransport::new()
            .push_json(
                "/api/v1/lists",
                200,
                serde_json::json!({ "lists": [{ "id": "l1", "name": "Home" }] }),
            )
            .push_json(
                "/api/v1/tasks",
                200,
                serde_json::json!({ "tasks": [{ "id": "t1", "title": "From the web" }] }),
            )
            .fallback(Err(TransportError::Unreachable("done".into())));
        let app = app_with(transport, secure);

        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let heard = heard.clone();
            app.realtime().on_change(move |change| {
                heard.lock().expect("lock").push(change.clone());
            });
        }

        let ticks = AtomicUsize::new(0);
        sync_loop(
            app,
            || ticks.fetch_add(1, Ordering::SeqCst) < 2,
            Duration::from_secs(60),
        )
        .await;

        let heard = heard.lock().expect("lock");
        assert_eq!(
            heard.as_slice(),
            &[crate::realtime::Change::Synced {
                task_ids: vec!["t1".into()],
                list_ids: vec!["l1".into()],
            }]
        );
    }

    /// And a pass that found nothing says nothing. Announcing every tick would redraw the
    /// window once a minute for the rest of the day.
    #[tokio::test(start_paused = true)]
    async fn a_background_pass_that_found_nothing_says_nothing() {
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = StubTransport::new()
            .push_json("/api/v1/lists", 200, serde_json::json!({ "lists": [] }))
            .push_json("/api/v1/tasks", 200, serde_json::json!({ "tasks": [] }))
            .fallback(Err(TransportError::Unreachable("done".into())));
        let app = app_with(transport, secure);

        let heard = Arc::new(AtomicUsize::new(0));
        {
            let heard = heard.clone();
            app.realtime().on_change(move |_| {
                heard.fetch_add(1, Ordering::SeqCst);
            });
        }

        let ticks = AtomicUsize::new(0);
        sync_loop(
            app,
            || ticks.fetch_add(1, Ordering::SeqCst) < 2,
            Duration::from_secs(60),
        )
        .await;

        assert_eq!(heard.load(Ordering::SeqCst), 0);
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
