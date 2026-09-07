//! The only thing in this crate that speaks to the Astrid backend.
//!
//! Ported from `astrid-ios/Astrid App/Core/Networking/AstridAPIClient.swift`. Services call it;
//! the shell never sees it, and neither does a timer, a notification handler or a sync worker —
//! rule 1 in `docs/ASTRID.md` §0, which exists because every write that skipped the service layer
//! on another platform skipped something else with it.
//!
//! Four things are true of every request this builds, and they are true *because* they are built
//! in one place:
//!
//! - the path is checked for traversal ([`super::path`]) before a URL is ever composed;
//! - the platform header is set ([`super::platform`]), so the request is counted as this app
//!   rather than as UNKNOWN;
//! - the session `Cookie` header is attached from secure storage;
//! - a 401 comes back as [`ApiError::Unauthorized`] rather than as an anonymous 4xx, because that
//!   one status is the difference between "retry later" and "sign the user out".
//!
//! The Apple apps set the platform header correctly in most places and still left the real-time
//! stream, the passkey calls, attachments and the OAuth token identifying nothing — each of those
//! built its own request. That is the argument for the seam, not tidiness.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::path;
use super::platform as platform_header;
use super::transport::{HttpRequest, HttpResponse, HttpTransport, Method, TransportError};
use crate::model::{self, Lenient};
use crate::platform::{SecureStore, SESSION_COOKIE_KEY};

/// Where the API lives. Overridable so a developer can point at a local astrid-web — which is not
/// a convenience: M0's exit criterion is signing in against one.
pub const DEFAULT_BASE_URL: &str = "https://astrid.cc";

/// Every path this client may reach starts here. Rule 5 in `docs/ASTRID.md` §0 stated as a
/// constant, and checked by [`ApiClient::request`], so a stray legacy path fails locally instead
/// of in production.
pub const API_PREFIX: &str = "/api/v1/";

/// The paths outside `/api/v1/` this client is allowed to reach.
///
/// Exactly two, both from before the versioned API existed and neither with a v1 equivalent yet.
/// An allow-list rather than a loosened rule: a new legacy path has to be added here on purpose.
const LEGACY_PATH_ALLOWLIST: [&str; 2] = ["/api/auth/desktop/grant", "/api/mcp/user-tokens"];

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    /// The session is gone or was never valid. Distinguished from every other 4xx because it is
    /// the only one that means "sign out", and treating it as a generic failure leaves the app
    /// retrying forever against a server that will never say yes.
    #[error("the session is not valid")]
    Unauthorized,
    #[error("the server refused the request: {status} {message}")]
    Http { status: u16, message: String },
    #[error("the response could not be read: {0}")]
    Decode(String),
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The request was refused before it was sent — a traversal in the path, or a path outside
    /// `/api/v1/`.
    #[error("the request was refused before it was sent: {0}")]
    Refused(String),
}

impl ApiError {
    /// Whether the Outbox should keep this entry and try again.
    ///
    /// A 5xx and a transport failure are the server's problem or the network's, and both pass.
    /// A 4xx is this request's problem and will never succeed unchanged — retrying it forever is
    /// how a queue turns into a permanent backlog that blocks everything behind it. 401 is the
    /// exception in the other direction: it is retryable in principle (a refreshed session fixes
    /// it) but never by simply sending the same bytes again, so the Outbox parks the entry and
    /// waits for sign-in rather than burning attempts.
    pub fn is_retryable(&self) -> bool {
        match self {
            ApiError::Transport(error) => error.is_retryable(),
            ApiError::Http { status, .. } => *status >= 500 || *status == 429 || *status == 408,
            ApiError::Unauthorized | ApiError::Decode(_) | ApiError::Refused(_) => false,
        }
    }

    /// Whether the entry should wait for a new session rather than be discarded.
    pub fn needs_authentication(&self) -> bool {
        matches!(self, ApiError::Unauthorized)
    }

    /// The status, when there was one.
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Unauthorized => Some(401),
            ApiError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// A request under construction. Built by [`ApiClient::get`] and friends.
#[derive(Debug, Clone)]
pub struct Request {
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    body: Option<serde_json::Value>,
}

impl Request {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Request {
            method,
            path: path.into(),
            query: Vec::new(),
            body: None,
        }
    }

    /// Add a query parameter. Absent values are skipped rather than sent empty — `?assignee=` and
    /// no `assignee` at all mean different things to the server.
    pub fn query(mut self, name: &str, value: impl Into<Option<String>>) -> Self {
        if let Some(value) = value.into() {
            self.query.push((name.to_string(), value));
        }
        self
    }

    pub fn json<T: Serialize>(mut self, body: &T) -> Self {
        self.body = Some(serde_json::to_value(body).unwrap_or(serde_json::Value::Null));
        self
    }

    /// A body given as JSON directly, for the update payloads that must be able to send an
    /// explicit `null` to clear a field. A typed struct with `skip_serializing_if` cannot express
    /// the difference between "leave it alone" and "clear it", and both are real operations.
    pub fn value(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }
}

pub struct ApiClient {
    base_url: String,
    transport: Arc<dyn HttpTransport>,
    secure_store: Arc<dyn SecureStore>,
}

impl ApiClient {
    pub fn new(
        base_url: impl Into<String>,
        transport: Arc<dyn HttpTransport>,
        secure_store: Arc<dyn SecureStore>,
    ) -> Self {
        ApiClient {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            transport,
            secure_store,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn get(&self, path: impl Into<String>) -> Request {
        Request::new(Method::Get, path)
    }

    pub fn post(&self, path: impl Into<String>) -> Request {
        Request::new(Method::Post, path)
    }

    pub fn put(&self, path: impl Into<String>) -> Request {
        Request::new(Method::Put, path)
    }

    pub fn patch(&self, path: impl Into<String>) -> Request {
        Request::new(Method::Patch, path)
    }

    pub fn delete(&self, path: impl Into<String>) -> Request {
        Request::new(Method::Delete, path)
    }

    /// Send, and hand back the decoded JSON body.
    ///
    /// A 204 and an empty body both become `Value::Null` rather than a decode error: several
    /// endpoints answer a successful DELETE with nothing at all.
    pub async fn send(&self, request: Request) -> Result<serde_json::Value, ApiError> {
        let response = self.send_raw(request).await?;
        if response.body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&response.body).map_err(|error| ApiError::Decode(error.to_string()))
    }

    /// Send, and decode into `T`.
    pub async fn send_as<T: DeserializeOwned>(&self, request: Request) -> Result<T, ApiError> {
        let value = self.send(request).await?;
        serde_json::from_value(value).map_err(|error| ApiError::Decode(error.to_string()))
    }

    /// Send, and decode a collection row by row.
    ///
    /// This is the one to reach for on any endpoint that returns an array. See
    /// [`crate::model::lenient`] for why: one unreadable row must cost that row, not the response.
    /// The caller is expected to log [`Lenient::skipped`] — a row that vanishes silently is worse
    /// than a response that fails loudly.
    pub async fn send_collection<T: DeserializeOwned>(
        &self,
        request: Request,
        key: Option<&str>,
    ) -> Result<Lenient<T>, ApiError> {
        let value = self.send(request).await?;
        let array = match key {
            // Most v1 collection routes answer `{ "tasks": [...], "total": n }`; a few answer a
            // bare array. Asking for a key that is absent falls back to the whole body rather than
            // reporting nothing, because both shapes have been seen on the same route.
            Some(key) => value.get(key).cloned().unwrap_or(value),
            None => value,
        };
        Ok(model::lenient(array))
    }

    /// Send, and return the response as it arrived. For the callers that need a header or a
    /// non-JSON body.
    pub async fn send_raw(&self, request: Request) -> Result<HttpResponse, ApiError> {
        let http = self.build(request).await?;
        let response = self.transport.send(http).await?;

        if response.status == 401 {
            return Err(ApiError::Unauthorized);
        }
        if !response.is_success() {
            return Err(ApiError::Http {
                status: response.status,
                message: response.text(),
            });
        }
        Ok(response)
    }

    /// Compose the request. Separated from sending so the guards can be tested without a
    /// transport, and so an Outbox entry can be rebuilt identically on a later attempt.
    pub async fn build(&self, request: Request) -> Result<HttpRequest, ApiError> {
        if !path::is_safe_request_path(&request.path) {
            return Err(ApiError::Refused(format!(
                "path contains a traversal segment: {}",
                request.path
            )));
        }
        if !request.path.starts_with(API_PREFIX)
            && !LEGACY_PATH_ALLOWLIST.contains(&request.path.as_str())
        {
            return Err(ApiError::Refused(format!(
                "path is outside {API_PREFIX}: {}",
                request.path
            )));
        }

        let mut url = format!("{}{}", self.base_url, request.path);
        if !request.query.is_empty() {
            let encoded: Vec<String> = request
                .query
                .iter()
                .map(|(name, value)| {
                    format!(
                        "{}={}",
                        path::escaped_path_component(name),
                        path::escaped_path_component(value)
                    )
                })
                .collect();
            url.push('?');
            url.push_str(&encoded.join("&"));
        }

        let mut headers = vec![("accept".to_string(), "application/json".to_string()), {
            let (name, value) = platform_header::header();
            (name.to_string(), value.to_string())
        }];
        if let Some(cookie) = self.secure_store.get(SESSION_COOKIE_KEY).await {
            headers.push(("cookie".to_string(), cookie));
        }

        let body = match request.body {
            Some(value) => {
                headers.push(("content-type".to_string(), "application/json".to_string()));
                Some(serde_json::to_vec(&value).map_err(|e| ApiError::Decode(e.to_string()))?)
            }
            None => None,
        };

        Ok(HttpRequest {
            method: request.method,
            url,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::transport::StubTransport;
    use crate::model::Task;
    use crate::platform::MemorySecureStore;

    fn client_with(transport: StubTransport) -> (ApiClient, Arc<StubTransport>) {
        let transport = Arc::new(transport);
        let client = ApiClient::new(
            "https://astrid.cc",
            transport.clone(),
            Arc::new(MemorySecureStore::with(
                SESSION_COOKIE_KEY,
                "next-auth.session-token=abc",
            )),
        );
        (client, transport)
    }

    #[tokio::test]
    async fn every_request_states_the_platform_and_carries_the_session() {
        let (client, transport) = client_with(StubTransport::new().push_json(
            "/api/v1/tasks",
            200,
            serde_json::json!([]),
        ));
        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");

        let sent = &transport.requests()[0];
        assert_eq!(sent.header("x-platform"), Some("windows-app"));
        assert_eq!(sent.header("cookie"), Some("next-auth.session-token=abc"));
        assert_eq!(sent.header("accept"), Some("application/json"));
    }

    /// Signed out is not an error state for the client to invent a credential around — it simply
    /// sends no cookie and lets the server answer 401.
    #[tokio::test]
    async fn with_no_stored_session_no_cookie_is_sent() {
        let transport =
            Arc::new(StubTransport::new().push_json("/api/v1/tasks", 200, serde_json::json!([])));
        let client = ApiClient::new(
            "https://astrid.cc",
            transport.clone(),
            Arc::new(MemorySecureStore::new()),
        );
        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");
        assert_eq!(transport.requests()[0].header("cookie"), None);
    }

    /// The 2026-07-25 audit finding, at the layer that is impossible to bypass: whichever call
    /// site built the path, a traversal never leaves the device.
    #[tokio::test]
    async fn a_traversal_is_refused_before_anything_is_sent() {
        let (client, transport) = client_with(StubTransport::new());
        let error = client
            .send(client.get("/api/v1/tasks/abc%2F..%2Fadmin"))
            .await
            .expect_err("refused");
        assert!(matches!(error, ApiError::Refused(_)));
        assert!(
            transport.requests().is_empty(),
            "a refused request must not reach the transport"
        );
    }

    /// Rule 5 of `docs/ASTRID.md` §0, enforced rather than remembered.
    #[tokio::test]
    async fn a_path_outside_v1_is_refused() {
        let (client, _) = client_with(StubTransport::new());
        assert!(matches!(
            client.send(client.get("/api/tasks")).await,
            Err(ApiError::Refused(_))
        ));
    }

    /// Two paths predate the versioned API and have no v1 equivalent. They are named, not waved
    /// through by loosening the rule.
    #[tokio::test]
    async fn the_two_legacy_paths_are_allowed_by_name() {
        let (client, _) = client_with(StubTransport::new().push_json(
            "/api/mcp/user-tokens",
            200,
            serde_json::json!({}),
        ));
        assert!(client
            .send(client.get("/api/mcp/user-tokens"))
            .await
            .is_ok());
    }

    /// 401 is the only status that means "sign out", and it has to arrive as itself.
    #[tokio::test]
    async fn an_expired_session_is_its_own_error() {
        let (client, _) = client_with(StubTransport::new().push_json(
            "/api/v1/tasks",
            401,
            serde_json::json!({ "error": "Unauthorized" }),
        ));
        let error = client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect_err("401");
        assert!(matches!(error, ApiError::Unauthorized));
        assert!(error.needs_authentication());
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn a_server_fault_is_retryable_and_a_refusal_is_not() {
        let (client, _) = client_with(
            StubTransport::new()
                .push_json("/api/v1/tasks", 500, serde_json::json!({}))
                .push_json("/api/v1/tasks", 422, serde_json::json!({})),
        );
        let server_fault = client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect_err("500");
        assert!(server_fault.is_retryable());
        assert_eq!(server_fault.status(), Some(500));

        let refusal = client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect_err("422");
        assert!(!refusal.is_retryable());
    }

    /// A successful DELETE answers with nothing. Reading that as a decode failure would make
    /// every delete look like it failed and leave the Outbox retrying a completed operation.
    #[tokio::test]
    async fn an_empty_success_is_not_a_decode_failure() {
        let transport = Arc::new(StubTransport::new().push(
            "/api/v1/tasks/t1",
            Ok(HttpResponse {
                status: 204,
                headers: Vec::new(),
                body: Vec::new(),
            }),
        ));
        let client = ApiClient::new(
            "https://astrid.cc",
            transport,
            Arc::new(MemorySecureStore::new()),
        );
        let value = client
            .send(client.delete("/api/v1/tasks/t1"))
            .await
            .expect("succeeds");
        assert!(value.is_null());
    }

    #[tokio::test]
    async fn a_collection_is_read_from_its_wrapper_or_from_a_bare_array() {
        let (client, _) = client_with(
            StubTransport::new()
                .push_json(
                    "/api/v1/tasks",
                    200,
                    serde_json::json!({ "tasks": [{ "id": "t1" }], "total": 1 }),
                )
                .push_json("/api/v1/tasks", 200, serde_json::json!([{ "id": "t2" }])),
        );

        let wrapped: Lenient<Task> = client
            .send_collection(client.get("/api/v1/tasks"), Some("tasks"))
            .await
            .expect("succeeds");
        assert_eq!(wrapped.items[0].id, "t1");

        let bare: Lenient<Task> = client
            .send_collection(client.get("/api/v1/tasks"), Some("tasks"))
            .await
            .expect("succeeds");
        assert_eq!(bare.items[0].id, "t2");
    }

    #[tokio::test]
    async fn query_parameters_are_escaped_and_absent_ones_are_omitted() {
        let (client, transport) = client_with(StubTransport::new().push_json(
            "/api/v1/users/search",
            200,
            serde_json::json!([]),
        ));
        client
            .send(
                client
                    .get("/api/v1/users/search")
                    .query("q", Some("ada lovelace".to_string()))
                    .query("listId", None),
            )
            .await
            .expect("succeeds");

        let url = &transport.requests()[0].url;
        assert!(url.contains("q=ada%20lovelace"), "{url}");
        assert!(!url.contains("listId"), "{url}");
    }

    /// Offline is not a refusal: it comes back as a transport error the Outbox will retry.
    #[tokio::test]
    async fn being_offline_arrives_as_a_retryable_transport_error() {
        let (client, _) = client_with(StubTransport::new().push(
            "/api/v1/tasks",
            Err(TransportError::Unreachable("dns".into())),
        ));
        let error = client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect_err("offline");
        assert!(error.is_retryable());
        assert_eq!(error.status(), None);
    }

    #[tokio::test]
    async fn a_body_is_sent_as_json_with_its_content_type() {
        let (client, transport) = client_with(StubTransport::new().push_json(
            "/api/v1/tasks",
            200,
            serde_json::json!({ "id": "t1" }),
        ));
        client
            .send(
                client
                    .post("/api/v1/tasks")
                    .json(&Task::new("t1", "Buy milk")),
            )
            .await
            .expect("succeeds");

        let sent = &transport.requests()[0];
        assert_eq!(sent.header("content-type"), Some("application/json"));
        let body: serde_json::Value =
            serde_json::from_slice(sent.body.as_ref().expect("a body")).expect("valid JSON");
        assert_eq!(body["title"], "Buy milk");
    }

    /// Clearing a field means sending `null`, which a struct with `skip_serializing_if` cannot
    /// express — hence `Request::value`.
    #[tokio::test]
    async fn a_raw_body_can_send_an_explicit_null() {
        let (client, transport) = client_with(StubTransport::new().push_json(
            "/api/v1/lists/l1",
            200,
            serde_json::json!({}),
        ));
        client
            .send(
                client
                    .put("/api/v1/lists/l1")
                    .value(serde_json::json!({ "defaultAssigneeId": null })),
            )
            .await
            .expect("succeeds");

        let body = transport.requests()[0].body.clone().expect("a body");
        assert_eq!(
            String::from_utf8_lossy(&body),
            r#"{"defaultAssigneeId":null}"#
        );
    }
}
