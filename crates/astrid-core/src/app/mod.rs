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

mod command;
mod dispatch;

pub use command::{Command, Failure, FailureKind, Response};

use std::sync::Arc;

use crate::api::{ApiClient, HttpTransport, ReqwestTransport};
use crate::outbox::Runner;
use crate::platform::{Clock, SecureStore, SystemClock};
use crate::realtime::RealtimeSink;
use crate::services::Context;
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
}

fn default_base_url() -> String {
    crate::api::DEFAULT_BASE_URL.to_string()
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
        let client = Arc::new(ApiClient::new(&config.base_url, transport, secure_store));
        let runner = Arc::new(Runner::new(client.clone(), store.clone(), clock.clone()));
        let sync = Arc::new(SyncManager::new(
            client.clone(),
            store.clone(),
            clock.clone(),
            runner.clone(),
        ));
        let realtime = Arc::new(RealtimeSink::new(store.clone()));

        Ok(App {
            context: Context::new(client.clone(), store.clone(), clock.clone()),
            runner,
            sync,
            realtime,
            store,
            client,
            clock,
        })
    }

    /// Run one command.
    ///
    /// Async throughout, even for the reads that never touch the network, so the boundary has one
    /// shape rather than two. A cache read completes without yielding, so the cost is a future that
    /// is already ready.
    pub async fn run(&self, command: Command) -> Response {
        dispatch::run(self, command).await
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
            },
            Arc::new(MemorySecureStore::new()),
            Arc::new(transport),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        )
        .expect("starts")
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
