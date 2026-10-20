//! The whole client behind one door.
//!
//! Everything above this — the FFI, and through it the WinUI shell — sees exactly one type
//! ([`App`]) and one vocabulary ([`Command`], [`Response`]). Nothing else is public to it: not the
//! services, not the store, not the API client.
//!
//! That is rule 9 of `docs/ASTRID.md` §0 made structural rather than aspirational. A shell that
//! could reach `TaskService` could also reach `update_task(completed: true)`, and the rule about
//! completing tasks would be a comment instead of a wall. Here the only way to complete a task is
//! [`Command::CompleteTask`], and that goes where it has to go.
//!
//! ## Why commands are data
//!
//! A command is a value, so:
//!
//! - every screen's behaviour is a test that builds a `Command` and asserts on a `Response`, with
//!   no window and no FFI;
//! - the boundary is one function rather than sixty exported symbols, so adding a feature does not
//!   mean touching a header, a P/Invoke declaration and a marshalling rule;
//! - what the shell asked for can be logged, replayed and diffed.
//!
//! The cost is a JSON encode per call. That is real, and it is why the read commands the list view
//! uses return only the rows in view — see [`Command::RowsForList`] — rather than a whole account.

pub mod background;
mod command;
mod dispatch;

pub use command::{kinds as command_kinds, Command, Failure, FailureKind, Response};

use std::sync::Arc;

use crate::api::{ApiClient, HttpTransport, ReqwestTransport};
use crate::outbox::Runner;
use crate::platform::{Clock, SecureStore, SystemClock};
use crate::realtime::RealtimeSink;
use crate::services::{AuthService, Context};
use crate::store::Store;
use crate::sync::SyncManager;

/// What the shell has to tell the core before anything else happens.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Where the cache lives. A path the shell owns — the core has no opinion about where an app's
    /// data belongs on Windows, and guessing would be the first Windows dependency in a crate that
    /// has none.
    pub cache_path: String,
    /// The server. Overridable so a developer can point at a local astrid-web, which is what M0's
    /// exit criterion needs.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Which client this is, stated on every request (`x-platform`). Windows when absent.
    #[serde(default)]
    pub platform: crate::api::Platform,
    /// The URL scheme the shell registered for the sign-in callback, without `://`
    /// (`urlScheme`). Astrid's `astrid` when absent; a white-label build names its own so it and
    /// Astrid can be installed side by side (AWTD2-95).
    #[serde(default = "default_url_scheme")]
    pub url_scheme: String,
    /// The id the server's desktop hand-off registry knows this app by (`handoffClientId`). The
    /// platform's own (`windows`, `android`, …) when absent — see
    /// [`crate::api::Platform::handoff_client`]. A partner server that registers its app under a
    /// different id names it here.
    #[serde(default)]
    pub handoff_client_id: Option<String>,
}

/// Astrid's sign-in callback scheme — what [`Config::url_scheme`] is when a shell leaves it out.
pub const DEFAULT_URL_SCHEME: &str = crate::services::auth::URL_SCHEME;

fn default_base_url() -> String {
    crate::api::DEFAULT_BASE_URL.to_string()
}

fn default_url_scheme() -> String {
    DEFAULT_URL_SCHEME.to_string()
}

/// The running client.
pub struct App {
    pub(crate) context: Context,
    pub(crate) runner: Arc<Runner>,
    pub(crate) sync: Arc<SyncManager>,
    pub(crate) realtime: Arc<RealtimeSink>,
    pub(crate) store: Arc<Store>,
    pub(crate) client: Arc<ApiClient>,
    pub(crate) clock: Arc<dyn Clock>,
    /// Held rather than made per call: it carries the flow between opening the browser and the
    /// callback coming back, and a fresh one would have nothing to check the callback against.
    pub(crate) auth: Arc<AuthService>,
    /// Where downloaded attachments are kept: `attachments/` beside the database, so one directory
    /// holds everything this installation stores and deleting it is a complete reset.
    pub(crate) attachment_cache: std::path::PathBuf,
    /// Rung after a command leaves something in the Outbox, so the delivery loop
    /// ([`background::outbox_loop`]) sends it now rather than on the next timer tick.
    ///
    /// Before this the journal drained only at the top of the sixty-second sync pass, so a task
    /// added here took up to a minute to exist anywhere else — an offline-first design behaving,
    /// while online, like an offline one.
    pub(crate) outbox_nudge: Arc<tokio::sync::Notify>,
    /// The one editing session (`PRODUCT_CONTRACT.md` §6): at most one editor open across the
    /// whole window, stepped by the `beginEditing` / `endEditing` / `cancelEditing` /
    /// `commitAllEditing` commands. Held here rather than in the shell so the rule is the core's
    /// — the shell only does what each answer names.
    pub(crate) editing: std::sync::Mutex<crate::editing::Session>,
}

impl App {
    /// Open the cache and wire everything to it.
    pub fn start(
        config: &Config,
        secure_store: Arc<dyn SecureStore>,
    ) -> Result<Self, crate::store::StoreError> {
        Self::with_parts(
            config,
            secure_store,
            Arc::new(ReqwestTransport::new()),
            Arc::new(SystemClock),
        )
    }

    /// The same, with the transport and the clock supplied. Every test uses this; so does the
    /// shell's UI-test build, which must never reach the network.
    pub fn with_parts(
        config: &Config,
        secure_store: Arc<dyn SecureStore>,
        transport: Arc<dyn HttpTransport>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, crate::store::StoreError> {
        let store = Arc::new(if config.cache_path == ":memory:" {
            Store::in_memory()?
        } else {
            Store::open(&config.cache_path)?
        });
        let client = Arc::new(
            ApiClient::new(&config.base_url, transport, secure_store)
                .with_platform(config.platform),
        );
        let runner = Arc::new(Runner::new(client.clone(), store.clone(), clock.clone()));
        let sync = Arc::new(SyncManager::new(
            client.clone(),
            store.clone(),
            clock.clone(),
            runner.clone(),
        ));
        let realtime = Arc::new(RealtimeSink::new(store.clone()));

        let context = Context::new(client.clone(), store.clone(), clock.clone());
        // Beside the database, whatever the shell chose as its data directory — including a test's
        // temporary one, which is what keeps a test run out of somebody's real cache.
        //
        // An in-memory cache has no directory to be beside, and the empty parent it yields lands
        // in whatever the working directory happens to be — for a test run, the repository. So it
        // gets one of its own, which nothing is expected to outlive the process.
        let attachment_cache = match config.cache_path.as_str() {
            ":memory:" => std::env::temp_dir().join(format!(
                "astrid-attachments-{}",
                crate::outbox::new_temp_id()
            )),
            path => std::path::Path::new(path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("attachments"),
        };
        Ok(App {
            auth: Arc::new(
                context
                    .auth()
                    .with_url_scheme(&config.url_scheme)
                    .with_handoff_client(config.handoff_client_id.as_deref()),
            ),
            context,
            runner,
            sync,
            realtime,
            store,
            client,
            clock,
            attachment_cache,
            outbox_nudge: Arc::new(tokio::sync::Notify::new()),
            editing: std::sync::Mutex::new(crate::editing::Session::IDLE),
        })
    }

    /// The bell the delivery loop listens for.
    pub fn outbox_nudge(&self) -> &Arc<tokio::sync::Notify> {
        &self.outbox_nudge
    }

    /// The cache, for tests that need to seed one.
    ///
    /// Public because the performance tests build an account of ten thousand tasks and going
    /// through `createTask` ten thousand times would measure the write path instead. Nothing
    /// outside this crate's own tests uses it: the FFI exposes commands and nothing else, so the
    /// shell could not reach the store if it wanted to.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Where downloaded attachments are kept.
    pub(crate) fn attachment_cache(&self) -> &std::path::Path {
        &self.attachment_cache
    }

    /// Run one command.
    ///
    /// Async throughout, even for the reads that never touch the network, so the boundary has one
    /// shape rather than two. A cache read completes without yielding, so the cost is a future that
    /// is already ready.
    pub async fn run(&self, command: Command) -> Response {
        let response = dispatch::run(self, command).await;
        // Whatever the command was, if the journal has something waiting the delivery loop
        // should hear about it now. One indexed lookup on a table that is pruned hourly; cheaper
        // than teaching every write arm to ring the bell, and impossible to forget.
        if crate::outbox::journal::has_pending(&self.store).unwrap_or(false) {
            self.outbox_nudge.notify_one();
        }
        response
    }

    /// Run a command given as JSON, and answer as JSON. What the FFI calls.
    pub async fn run_json(&self, request: &str) -> String {
        let command: Command = match serde_json::from_str(request) {
            Ok(command) => command,
            Err(error) => {
                // A command the core cannot read is a bug in the shell, and it has to say so
                // rather than fail silently — a button that does nothing is the hardest kind of
                // bug to report.
                return Response::failed(Failure::bad_request(error.to_string())).to_json();
            }
        };
        self.run(command).await.to_json()
    }

    /// The live-update sink, for the shell to subscribe to.
    pub fn realtime(&self) -> &Arc<RealtimeSink> {
        &self.realtime
    }

    pub fn client(&self) -> &Arc<ApiClient> {
        &self.client
    }

    /// Sign-in, which the shell needs directly: it opens the browser and receives the activation.
    pub fn auth(&self) -> &Arc<AuthService> {
        &self.auth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::StubTransport;
    use crate::platform::{FixedClock, MemorySecureStore};

    pub(crate) fn app_with(transport: StubTransport) -> App {
        App::with_parts(
            &Config {
                cache_path: ":memory:".into(),
                base_url: "https://astrid.cc".into(),
                platform: Default::default(),
                url_scheme: crate::app::DEFAULT_URL_SCHEME.into(),
                handoff_client_id: None,
            },
            Arc::new(MemorySecureStore::new()),
            Arc::new(transport),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        )
        .expect("starts")
    }

    /// AWTD2-95. A shell that says nothing about its identity is Astrid, exactly as before.
    #[test]
    fn awtd2_95_a_config_without_an_identity_is_astrids() {
        let config: Config = serde_json::from_str(r#"{"cachePath":":memory:"}"#).expect("reads");
        assert_eq!(config.url_scheme, "astrid");
        assert_eq!(config.url_scheme, DEFAULT_URL_SCHEME);
        assert_eq!(config.handoff_client_id, None);
        let app = App::with_parts(
            &config,
            Arc::new(MemorySecureStore::new()),
            Arc::new(StubTransport::new()),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        )
        .expect("starts");
        assert_eq!(app.auth().url_scheme(), "astrid");
        assert_eq!(app.auth().handoff_client(), "windows");
    }

    /// AWTD2-95. The white-label identity rides in the same JSON config the Windows C ABI and the
    /// Android UniFFI binding already hand over, and reaches the sign-in flow.
    #[tokio::test]
    async fn awtd2_95_a_shell_names_its_scheme_and_client_in_the_config() {
        let config: Config = serde_json::from_str(
            r#"{"cachePath":":memory:","platform":"android-app","urlScheme":"partner","handoffClientId":"partner-android"}"#,
        )
        .expect("reads");
        assert_eq!(config.url_scheme, "partner");
        assert_eq!(config.handoff_client_id.as_deref(), Some("partner-android"));
        let transport = StubTransport::new().push_json(
            "/api/v1/auth/desktop/exchange",
            200,
            serde_json::json!({
                "sessionToken": "t",
                "expiresAt": "2026-10-07T12:00:00Z",
                "sessionCookieName": "next-auth.session-token",
                "user": { "id": "u1", "email": "ada@example.com" }
            }),
        );
        let app = App::with_parts(
            &config,
            Arc::new(MemorySecureStore::new()),
            Arc::new(transport),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        )
        .expect("starts");
        let url = app.auth().begin().expect("begins");
        assert!(url.contains("client=partner-android"), "{url}");
        let state = url::Url::parse(&url)
            .expect("a URL")
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("a state");
        let user = app
            .auth()
            .complete(&format!("partner://auth/callback?code=abc&state={state}"))
            .await
            .expect("completes");
        assert_eq!(user.id, "u1");
    }

    #[tokio::test]
    async fn a_command_the_core_cannot_read_says_so() {
        let app = app_with(StubTransport::new());
        let answer = app.run_json("{\"kind\":\"somethingLater\"}").await;
        assert!(answer.contains("\"ok\":false"), "{answer}");
        assert!(answer.contains("badRequest"), "{answer}");
    }

    #[tokio::test]
    async fn a_cache_read_answers_without_a_network() {
        let app = app_with(StubTransport::new());
        let answer = app.run_json("{\"kind\":\"lists\"}").await;
        assert!(answer.contains("\"ok\":true"), "{answer}");
    }
}
