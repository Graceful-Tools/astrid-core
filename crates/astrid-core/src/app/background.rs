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
//!
//! **Which is also why the floor moves.** While the stream is connected the pass is re-asking for
//! what the stream already said: five sequential round trips a minute, forever, for an account
//! nothing had happened to. So a healthy stream stretches the tick to ten minutes and a reconnect
//! buys a catch-up pass — [`policy::tick_fetches`] has the rule and the reasoning. The heartbeat
//! itself stays at a minute, because it is also how fast the loop notices the stream is gone.

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
///
/// And never runs just because a minute passed, either — see [`policy::tick_fetches`]. The tick is
/// the heartbeat; whether it costs anything depends on whether the stream has the account covered.
pub async fn sync_loop(
    app: Arc<App>,
    should_continue: impl Fn() -> bool + Send,
    interval: Duration,
) {
    // The pass this loop last ran, against the clock the loop sleeps on rather than
    // `app.clock` — a shell's clock can be a fixed one, and an interval measured against a clock
    // that never moves is an interval that never elapses.
    let mut last_pass: Option<(u64, tokio::time::Instant)> = None;
    while should_continue() {
        tokio::time::sleep(interval).await;
        if !should_continue() {
            return;
        }
        if !app.auth.is_signed_in().await {
            continue;
        }
        let stream = policy::StreamHealth {
            live: app.realtime.is_live(),
            connections: app.realtime.connections(),
        };
        if !policy::tick_fetches(
            stream,
            last_pass.map(|(connections, at)| policy::LastPass {
                connections,
                age_secs: at.elapsed().as_secs(),
            }),
        ) {
            continue;
        }
        last_pass = Some((stream.connections, tokio::time::Instant::now()));
        let mut report = app.sync.sync_in_background().await;
        tracing::debug!(
            fetched = report.fetched,
            skipped = report.skipped,
            tasks_added = report.tasks_added,
            tasks_updated = report.tasks_updated,
            tasks_deleted = report.tasks_deleted,
            "background sync"
        );
        if let Some(delivered) = report.delivered.take() {
            announce_delivery(&app, delivered);
        }
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
            // The flags too, quietly: what this person may see can change while the app is open.
            let _ = app.context.account().refresh_features().await;
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
                    announce_delivery(&app, report.delivered);
                }
            }
            Err(error) => tracing::debug!(%error, "outbox delivery failed"),
        }
    }
}

/// Tell the shell what a drain settled: the rows it touched, and the journal's counts with them
/// (AITD-454). Named rather than "something moved", so a shell re-reads one task after an edit
/// instead of every task, thread and channel it holds.
pub(crate) fn announce_delivery(app: &App, touched: crate::outbox::Delivery) {
    // An empty one still goes out — a settings write settled, which nothing on screen reads back
    // — because the counts moved, and a shell showing them has to hear so.
    match crate::outbox::journal::stats(&app.store) {
        Ok(outbox) => app
            .realtime()
            .publish(crate::realtime::Change::Delivered { touched, outbox }),
        // Counts it cannot give are not counts of zero: say what moved the old way, and the
        // shell asks for itself.
        Err(_) => app.realtime().publish(crate::realtime::Change::Delivered {
            touched: crate::outbox::Delivery {
                undescribed: true,
                ..touched
            },
            outbox: crate::outbox::Stats::default(),
        }),
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
/// difference between useful and annoying.
///
/// This is only cheap enough to do that often because the read is bounded: an index range over
/// `reminder_time` plus this device's snooze marks, which on an account with no reminder coming up
/// decodes nothing at all. Until task `71ddacb8` the comment here said it was "a SQLite read of
/// tasks already in memory" — it was neither. Nothing is held in memory between ticks, and it
/// decoded every task in the account, twice a minute, for as long as the app was open.
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
        let Ok(due) = super::dispatch::due_reminders(&app) else {
            continue;
        };
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
#[cfg(feature = "sync-google")]
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

/// The same loop in a build with no Google pass: there is nothing to poll, so it returns — task
/// `b92569a8`.
///
/// Kept as a function rather than removed because a shell spawns it by name. Returning at once is
/// also what makes the five-minute timer go away: no sleep, no wake, no battery.
#[cfg(not(feature = "sync-google"))]
pub async fn external_loop(
    _app: Arc<App>,
    _should_continue: impl Fn() -> bool + Send,
    _interval: Duration,
) {
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
            tokio::select! {
                _ = tokio::time::sleep(SIGNED_OUT_RETRY) => {}
                // A sign-in says so, rather than waiting out the retry.
                _ = app.realtime.woken() => {}
            }
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
            // until the next attempt succeeds, or until somebody says the world changed.
            realtime::Stopped::OutOfAttempts => {
                tokio::select! {
                    _ = tokio::time::sleep(default_sync_interval()) => {}
                    _ = app.realtime.woken() => {}
                }
            }
        }
    }
}

/// Which of the background loops a shell starts.
///
/// **The policy is [`LoopSet::ALL`], and it lives here because it is not a shell's to decide.**
/// Apple started three of the five and Windows and Android started all five, so "is this account
/// current?" had a different answer on each platform and adding a loop meant editing three
/// repositories. A named subset exists for the one honest reason to want one — a harness that
/// wants an app with nothing running behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopSet {
    /// The sixty-second sync pass.
    pub sync: bool,
    /// The Outbox drain, woken by a write.
    pub outbox: bool,
    /// The half-minute look for a reminder that has come due.
    pub reminders: bool,
    /// The five-minute mirror of the externally-linked lists.
    pub external: bool,
    /// The live-update stream, kept connected.
    pub realtime: bool,
}

impl LoopSet {
    /// Every loop. What a shell starts, on every platform.
    pub const ALL: Self = Self {
        sync: true,
        outbox: true,
        reminders: true,
        external: true,
        realtime: true,
    };

    /// None of them, for a harness that wants an app that does nothing until it is asked.
    pub const NONE: Self = Self {
        sync: false,
        outbox: false,
        reminders: false,
        external: false,
        realtime: false,
    };

    /// How many loops this set starts.
    pub fn count(&self) -> usize {
        [
            self.sync,
            self.outbox,
            self.reminders,
            self.external,
            self.realtime,
        ]
        .into_iter()
        .filter(|started| *started)
        .count()
    }
}

impl Default for LoopSet {
    fn default() -> Self {
        Self::ALL
    }
}

/// The loops a shell started, and the one thing it has to hold: the flag that stops them.
///
/// Dropping the runtime would stop them anyway, but only at each loop's next await point — which
/// for a stream sitting on a socket is whenever the server next says something. Asking them to
/// stop first means closing the app takes a moment rather than however long a quiet connection
/// stays quiet.
pub struct BackgroundLoops {
    running: Arc<std::sync::atomic::AtomicBool>,
    started: usize,
}

impl BackgroundLoops {
    /// Ask every loop to finish at its next await point. Idempotent.
    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the loops have been asked to stop.
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many loops were spawned — [`LoopSet::count`] of the set that started them.
    pub fn started(&self) -> usize {
        self.started
    }
}

/// Dropping the handle stops the loops.
///
/// Without this, a shell that closes its window and drops everything leaves five tasks polling a
/// server until the runtime itself goes. Stopping is the only correct meaning of "nobody holds
/// these any more".
impl Drop for BackgroundLoops {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the background loops for `app` on `handle`, and hand back the flag that stops them.
///
/// Prefer [`App::start_background`], which is this with the app already in hand.
pub fn start(app: &Arc<App>, handle: &tokio::runtime::Handle, loops: LoopSet) -> BackgroundLoops {
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));

    // One flag, read by every loop. `keep_going` is rebuilt per loop because each takes it by
    // value, and `realtime_loop` needs one that is `Copy` so its inner reconnect can check it too.
    macro_rules! keep_going {
        () => {{
            let running = running.clone();
            move || running.load(std::sync::atomic::Ordering::Relaxed)
        }};
    }

    if loops.sync {
        handle.spawn(sync_loop(
            app.clone(),
            keep_going!(),
            default_sync_interval(),
        ));
    }
    if loops.outbox {
        let nudge = app.outbox_nudge().clone();
        handle.spawn(outbox_loop(app.clone(), keep_going!(), nudge));
    }
    if loops.reminders {
        handle.spawn(reminder_loop(app.clone(), keep_going!(), REMINDER_INTERVAL));
    }
    if loops.external {
        handle.spawn(external_loop(app.clone(), keep_going!(), EXTERNAL_INTERVAL));
    }
    if loops.realtime {
        let app = app.clone();
        let keep_going = keep_going!();
        handle.spawn(async move {
            let keep_going = keep_going;
            realtime_loop(app, &keep_going).await;
        });
    }

    BackgroundLoops {
        running,
        started: loops.count(),
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
                    platform: Default::default(),
                    url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
                    handoff_client_id: None,
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
                    platform: Default::default(),
                    url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
                    handoff_client_id: None,
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
                    platform: Default::default(),
                    url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
                    handoff_client_id: None,
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
                if matches!(change, crate::realtime::Change::Delivered { .. }) {
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
            status_role: None,
            quick_add: false,
            locale: None,
            repeating: None,
            repeating_data: None,
            repeat_from: None,
            is_private: None,
            apply_list_defaults: true,
            wait_on_task_id: None,
            keep_assignee: false,
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

    /// AITD-454: a delivered edit names the task it wrote, and nothing else. Announced as an
    /// empty `Synced` — "something moved, cannot say what" — every write made the Apple shell read
    /// every task in the cache, every open comment thread and every open chat channel.
    #[tokio::test(start_paused = true)]
    async fn aitd_454_a_delivered_edit_names_its_task_rather_than_everything() {
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = StubTransport::new()
            .push_json(
                "/api/v1/tasks/t1",
                200,
                serde_json::json!({ "task": { "id": "t1", "title": "Buy oat milk" } }),
            )
            .fallback(Err(TransportError::Unreachable("done".into())));
        let app = app_with(transport, secure);
        app.store
            .upsert_task(&crate::model::Task::new("t1", "Buy milk"))
            .expect("stores");
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let heard = heard.clone();
            app.realtime().on_change(move |change| {
                heard.lock().expect("lock").push(change.clone());
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
        app.context
            .tasks()
            .update(
                "t1",
                &crate::services::TaskChanges {
                    title: Some("Buy oat milk".into()),
                    ..Default::default()
                },
            )
            .expect("journals");
        app.outbox_nudge().notify_one();
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        running.store(false, Ordering::SeqCst);
        app.outbox_nudge().notify_one();
        let _ = looping.await;

        let heard = heard.lock().expect("lock");
        assert_eq!(
            heard.as_slice(),
            &[crate::realtime::Change::Delivered {
                touched: crate::outbox::Delivery {
                    task_ids: vec!["t1".into()],
                    ..Default::default()
                },
                outbox: crate::outbox::Stats {
                    completed: 1,
                    ..Default::default()
                },
            }],
            "one change, naming the one task"
        );
    }

    /// AITD-454: a create names its temporary id and the server's, so the optimistic row can be
    /// swapped for the real one without reading the rest.
    #[tokio::test(start_paused = true)]
    async fn aitd_454_a_delivered_create_names_its_temporary_and_real_ids() {
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = StubTransport::new()
            .push_json(
                "/api/v1/tasks",
                200,
                serde_json::json!({ "task": { "id": "t-real", "title": "Buy milk" } }),
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
        let created = app
            .context
            .tasks()
            .create(&crate::services::TaskDraft {
                title: "Buy milk".into(),
                ..Default::default()
            })
            .expect("journals");

        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let looping = {
            let (app, running) = (app.clone(), running.clone());
            tokio::spawn(outbox_loop(
                app.clone(),
                move || running.load(Ordering::SeqCst),
                app.outbox_nudge().clone(),
            ))
        };
        app.outbox_nudge().notify_one();
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        running.store(false, Ordering::SeqCst);
        app.outbox_nudge().notify_one();
        let _ = looping.await;

        let heard = heard.lock().expect("lock");
        let [crate::realtime::Change::Delivered { touched, .. }] = heard.as_slice() else {
            panic!("one delivery, heard {heard:?}");
        };
        assert_eq!(touched.task_ids, vec![created.id.clone(), "t-real".into()]);
        assert!(touched.list_ids.is_empty() && touched.channel_ids.is_empty());
        assert!(!touched.undescribed);
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

    /// Task 5751fcb8, finding C3: an idle minute with the live stream healthy makes no requests at
    /// all. It used to make five on every tick — lists, tasks, projects (a full refetch), the
    /// flags and the notification inbox — for an account nothing had happened to, while the stream
    /// sat there ready to say so within a second if it had.
    ///
    /// One pass still runs: the cache has to be filled from somewhere, and the stream only ever
    /// reports what happens from now on.
    #[tokio::test(start_paused = true)]
    async fn task_5751fcb8_an_idle_minute_with_a_live_stream_makes_no_further_requests() {
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = StubTransport::new()
            .push_json("/api/v1/lists", 200, serde_json::json!({ "lists": [] }))
            .push_json("/api/v1/tasks", 200, serde_json::json!({ "tasks": [] }))
            .fallback(Err(TransportError::Unreachable("done".into())));
        let recorded = transport.recorded.clone();
        let app = app_with(transport, secure);
        app.realtime.set_live(true);

        let ticks = AtomicUsize::new(0);
        sync_loop(
            app,
            || ticks.fetch_add(1, Ordering::SeqCst) < 12,
            Duration::from_secs(60),
        )
        .await;

        let lists = recorded
            .lock()
            .expect("lock")
            .iter()
            .filter(|request| request.url.contains("/api/v1/lists"))
            .count();
        assert_eq!(
            lists, 1,
            "one pass to fill the cache, and then the stream carries it"
        );
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
                    platform: Default::default(),
                    url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
                    handoff_client_id: None,
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
                    platform: Default::default(),
                    url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
                    handoff_client_id: None,
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

    /// The policy is "every shell runs every loop", and it is written down once.
    ///
    /// Apple started three of the five and Windows and Android started all five, so an account
    /// that was current on one platform was stale on another and adding a loop meant editing
    /// three repositories (astrid-windows task `ab249b76`). [`LoopSet::ALL`] is now the answer,
    /// and a loop added below without being named here fails this.
    #[test]
    fn every_loop_is_in_the_shared_policy() {
        let all = LoopSet::ALL;
        assert!(all.sync && all.outbox && all.reminders && all.external && all.realtime);
        assert_eq!(all.count(), 5);
        assert_eq!(LoopSet::default().count(), 5);
        assert_eq!(LoopSet::NONE.count(), 0);
    }

    /// A shell starts the loops with one line and gets back the one thing it has to hold: the
    /// flag that stops them. Starting fewer is possible — a test harness wants none — but it is a
    /// deliberate, named subset, not five copies of a spawn.
    #[tokio::test(start_paused = true)]
    async fn start_background_spawns_the_set_and_stop_ends_them() {
        let app = app_with(StubTransport::new(), Arc::new(MemorySecureStore::new()));

        let loops = app.start_background(&tokio::runtime::Handle::current(), LoopSet::ALL);
        assert_eq!(loops.started(), 5);
        assert!(loops.is_running());

        loops.stop();
        assert!(!loops.is_running());
    }

    /// Dropping the handle stops them too. The alternative is a shell that closes its window,
    /// drops everything, and leaves five tasks polling a server until the runtime notices.
    #[tokio::test(start_paused = true)]
    async fn dropping_the_handle_stops_the_loops() {
        let app = app_with(StubTransport::new(), Arc::new(MemorySecureStore::new()));

        let still_running = {
            let loops = app.start_background(&tokio::runtime::Handle::current(), LoopSet::NONE);
            loops.running.clone()
        };

        assert!(!still_running.load(Ordering::SeqCst));
    }
}
