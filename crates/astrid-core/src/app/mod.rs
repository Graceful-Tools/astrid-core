//! The whole client behind one door.
//!
//! Everything above this — the FFI, and through it the WinUI shell — sees exactly one type
//! ([`App`]) and one vocabulary ([`Command`], [`Response`]). Nothing else is public to it: not the
//! services, not the store, not the API client.
//!
//! That is rule 8 of README.md made structural rather than aspirational. A shell that
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
//!
//! Note what that does and does not claim, because this said more than it should until task
//! `71ddacb8`: what crosses the FFI boundary is the window. What the core *reads* to decide which
//! rows those are is still every task the view could match, decoded from `json`, because the
//! filter and sort rules take whole `Task`s. The window bounds the boundary, not the cache read.

pub mod background;
mod command;
mod dispatch;
/// The wire as a JSON Schema. Only under `schema`, which no shipped client turns on — see the
/// module note and `crates/astrid-rules/Cargo.toml`.
#[cfg(feature = "schema")]
pub mod schema;

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
#[serde(try_from = "StartConfig")]
pub struct Config {
    /// Where the cache lives. A path the shell owns — the core has no opinion about where an app's
    /// data belongs on Windows, and guessing would be the first Windows dependency in a crate that
    /// has none.
    pub cache_path: String,
    /// The server (`baseUrl`): the shell's brand's, or a local astrid-web for a developer.
    ///
    /// Absent, it is Astrid's ([`crate::api::DEFAULT_BASE_URL`]) — unless the crate is built with
    /// the `require-base-url` feature, which compiles Astrid's server out of the binary and refuses
    /// a start that does not name one ([`CONFIG_BASE_URL_REQUIRED`]). A white-label binary must not
    /// carry Astrid's server, and must fail loudly rather than reach it (astrid-web AWTD-1627).
    /// Required is where every shell is headed; the default goes once they all pass one.
    pub base_url: String,
    /// Which client this is, stated on every request (`x-platform`). Windows when absent.
    pub platform: crate::api::Platform,
    /// The URL scheme the shell registered for the sign-in callback, without `://`
    /// (`urlScheme`). Astrid's `astrid` when absent; a white-label build names its own so it and
    /// Astrid can be installed side by side (AWTD2-95).
    pub url_scheme: String,
    /// The id the server's desktop hand-off registry knows this app by (`handoffClientId`). The
    /// platform's own (`windows`, `android`, …) when absent — see
    /// [`crate::api::Platform::handoff_client`]. A partner server that registers its app under a
    /// different id names it here.
    pub handoff_client_id: Option<String>,
}

/// The start config as the shell writes it: every key but `cachePath` may be left out (or sent
/// `null`), and [`Config`] fills it in or refuses it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartConfig {
    cache_path: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    platform: crate::api::Platform,
    #[serde(default)]
    url_scheme: Option<String>,
    #[serde(default)]
    handoff_client_id: Option<String>,
}

impl TryFrom<StartConfig> for Config {
    type Error = String;

    fn try_from(raw: StartConfig) -> Result<Self, Self::Error> {
        Ok(Config {
            cache_path: raw.cache_path,
            base_url: match raw.base_url {
                Some(base_url) => base_url,
                None => absent_base_url()?,
            },
            platform: raw.platform,
            url_scheme: raw
                .url_scheme
                .unwrap_or_else(|| DEFAULT_URL_SCHEME.to_string()),
            handoff_client_id: raw.handoff_client_id,
        })
    }
}

/// The code a start fails with when the config names no server and the build has no default
/// (`require-base-url`). It leads the error's text, which is what every binding hands its shell.
pub const CONFIG_BASE_URL_REQUIRED: &str = "CONFIG_BASE_URL_REQUIRED";

/// The code a Google command is refused with in a build without `sync-google` — task `b92569a8`.
///
/// The command is still in [`command_kinds`]: a vocabulary that changes shape with a feature is a
/// vocabulary every shell's bindings gate has to read a matrix of (astrid-windows'
/// `bindings_contract.rs`, and the Apple and Android equivalents). So a partner build that does not
/// mirror Google answers rather than forgets, and a shell can tell this refusal from a server one
/// by the code.
pub const SYNC_GOOGLE_UNAVAILABLE: &str = "SYNC_GOOGLE_UNAVAILABLE";

/// Astrid's sign-in callback scheme — what [`Config::url_scheme`] is when a shell leaves it out.
pub const DEFAULT_URL_SCHEME: &str = crate::services::auth::URL_SCHEME;

/// What `baseUrl` is when the shell leaves it out: Astrid's server, or — built with
/// `require-base-url` — a refusal, so the binary holds no server but the one its shell names.
#[cfg(not(feature = "require-base-url"))]
fn absent_base_url() -> Result<String, String> {
    Ok(crate::api::DEFAULT_BASE_URL.to_string())
}

#[cfg(feature = "require-base-url")]
fn absent_base_url() -> Result<String, String> {
    Err(format!(
        "{CONFIG_BASE_URL_REQUIRED}: the start config names no baseUrl, and this build has no \
         default server; pass the brand's own"
    ))
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
    /// The one editing session (`astrid-web/docs/PRODUCT_CONTRACT.md` §6): at most one editor open
    /// across the whole window, stepped by the `beginEditing` / `endEditing` / `cancelEditing` /
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

    /// Start the loops that keep the app up to date without being asked.
    ///
    /// One line per shell, and the same loops everywhere: which of them run is
    /// [`background::LoopSet`]'s to say, not a window's (astrid-windows task `ab249b76`). Hold
    /// the returned [`background::BackgroundLoops`] for as long as the app is open — dropping it
    /// stops them.
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use astrid_core::app::{background::LoopSet, App};
    /// # fn example(app: Arc<App>, runtime: &tokio::runtime::Runtime) {
    /// let loops = app.start_background(runtime.handle(), LoopSet::ALL);
    /// # }
    /// ```
    pub fn start_background(
        self: &Arc<Self>,
        handle: &tokio::runtime::Handle,
        loops: background::LoopSet,
    ) -> background::BackgroundLoops {
        background::start(self, handle, loops)
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

    /// astrid-web AWTD-1627. A shell that names its server gets it, in every build.
    #[test]
    fn awtd_1627_a_named_server_is_used_in_every_build() {
        let config: Config = serde_json::from_str(
            r#"{"cachePath":":memory:","baseUrl":"https://tasks.partner.example"}"#,
        )
        .expect("reads");
        assert_eq!(config.base_url, "https://tasks.partner.example");
    }

    /// astrid-web AWTD-1627. Without `require-base-url`, a shell that leaves the server out — or
    /// sends it `null` — still gets Astrid's, exactly as before.
    #[cfg(not(feature = "require-base-url"))]
    #[test]
    fn awtd_1627_an_absent_server_is_astrids_by_default() {
        for json in [
            r#"{"cachePath":":memory:"}"#,
            r#"{"cachePath":":memory:","baseUrl":null}"#,
        ] {
            let config: Config = serde_json::from_str(json).expect("reads");
            assert_eq!(config.base_url, crate::api::DEFAULT_BASE_URL, "{json}");
        }
    }

    /// astrid-web AWTD-1627. Built with `require-base-url`, a start that names no server fails
    /// with the code, rather than reaching a server the brand never chose.
    #[cfg(feature = "require-base-url")]
    #[test]
    fn awtd_1627_an_absent_server_is_refused_when_required() {
        for json in [
            r#"{"cachePath":":memory:"}"#,
            r#"{"cachePath":":memory:","baseUrl":null}"#,
        ] {
            let error = serde_json::from_str::<Config>(json).expect_err("refused");
            assert!(
                error.to_string().starts_with(CONFIG_BASE_URL_REQUIRED),
                "{json}: {error}"
            );
        }
    }

    /// AWTD2-95. A shell that says nothing about its identity is Astrid, exactly as before.
    #[test]
    fn awtd2_95_a_config_without_an_identity_is_astrids() {
        let config: Config =
            serde_json::from_str(r#"{"cachePath":":memory:","baseUrl":"https://astrid.cc"}"#)
                .expect("reads");
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
            r#"{"cachePath":":memory:","baseUrl":"https://astrid.cc","platform":"android-app","urlScheme":"partner","handoffClientId":"partner-android"}"#,
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
