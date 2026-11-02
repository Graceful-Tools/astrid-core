//! The only thing in this crate that speaks to the Astrid backend.
//!
//! Ported from `astrid-ios/Astrid App/Core/Networking/AstridAPIClient.swift`. Services call it;
//! the shell never sees it, and neither does a timer, a notification handler or a sync worker —
//! rule 1 in README.md, which exists because every write that skipped the service layer
//! on another platform skipped something else with it.
//!
//! Four things are true of every request this builds, and they are true *because* they are built
//! in one place:
//!
//! - the path is checked for traversal ([`super::path`]) before a URL is ever composed;
//! - the platform header is set ([`super::platform`]), so the request is counted as this app
//!   rather than as UNKNOWN;
//! - the session `Cookie` header is attached, from secure storage the first time and from this
//!   client's own copy after that ([`ApiClient::session_cookie`]);
//! - a 401 comes back as [`ApiError::Unauthorized`] rather than as an anonymous 4xx, because that
//!   one status is the difference between "retry later" and "sign the user out".
//!
//! The Apple apps set the platform header correctly in most places and still left the real-time
//! stream, the passkey calls, attachments and the OAuth token identifying nothing — each of those
//! built its own request. That is the argument for the seam, not tidiness.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::endpoints;
use super::path;
use super::platform as platform_header;
use super::transport::{HttpRequest, HttpResponse, HttpTransport, Method, TransportError};
use crate::model::{self, Lenient};
use crate::platform::{SecureStore, SESSION_COOKIE_KEY};

/// Where the API lives. Overridable so a developer can point at a local astrid-web — which is not
/// a convenience: M0's exit criterion is signing in against one.
pub const DEFAULT_BASE_URL: &str = "https://astrid.cc";

/// Every path this client may reach starts here. Rule 4 in README.md stated as a
/// constant, and checked by [`ApiClient::request`], so a stray legacy path fails locally instead
/// of in production.
pub const API_PREFIX: &str = "/api/v1/";

/// The header that names which running client made a request (astrid-web AWTD-1497,
/// `lib/realtime/client-id.ts`). The live stream names the same client as `?clientId=`.
///
/// The server tags the event a task update causes with it and drops that event on the one
/// connection that names the same client. So the actor's *other* devices hear their own edit live,
/// and the device that made it never gets an echo that could spring a checkbox back mid-run of
/// quick toggles (the 2026-09-08 completion storm). A write with no id leaves the actor out
/// entirely — which is why an edit on the iPhone reached the open Mac list only on the 60-second
/// sync (AITD-574).
pub const CLIENT_ID_HEADER: &str = "x-astrid-client-id";

/// The paths outside `/api/v1/` this client is allowed to reach.
///
/// None has a v1 equivalent. An allow-list rather than a loosened rule: a new legacy path has to
/// be added here on purpose.
///
/// The two WebAuthn registration paths joined them for task 4c285039. Registering a passkey needs
/// a challenge issued and verified by the server, and `/api/v1/users/me/passkeys` is read-only —
/// so without these a native client cannot add a passkey at all, which is what sent Android and
/// Windows to the web page for it.
const LEGACY_PATH_ALLOWLIST: [&str; 4] = [
    "/api/auth/desktop/grant",
    "/api/mcp/user-tokens",
    endpoints::WEBAUTHN_REGISTER_OPTIONS,
    endpoints::WEBAUTHN_REGISTER_VERIFY,
];

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
    /// A body that is not JSON: its content type, and its bytes. Used by the one thing that sends
    /// a file.
    raw: Option<(String, Vec<u8>)>,
    /// Headers this one call adds to the ones every call carries.
    headers: Vec<(String, String)>,
}

impl Request {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Request {
            method,
            path: path.into(),
            query: Vec::new(),
            body: None,
            raw: None,
            headers: Vec::new(),
        }
    }

    /// Add a header to this one call.
    ///
    /// For the things that are a property of the *request* rather than of the client: a
    /// conditional GET's `If-None-Match`, say. The headers every call carries — accept, platform,
    /// the session cookie — are [`ApiClient::build`]'s and are not set here.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// A body of bytes, with the content type to send them under.
    pub fn bytes(mut self, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        self.raw = Some((content_type.into(), body));
        self
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

/// What this client knows about the session cookie without asking the credential vault.
///
/// `Unknown` and `Known(None)` are different answers, and keeping them apart is half the point:
/// collapsing them into a plain `Option<String>` would leave a *signed-out* app calling into the
/// Keychain, the Keystore or the Credential Locker on every single request, which is the same cost
/// the cache exists to remove.
#[derive(Debug, Clone, Default)]
enum CachedCookie {
    /// Nobody has asked the store yet, or something invalidated what we had.
    #[default]
    Unknown,
    /// The store was asked, and this is what it said.
    Known(Option<String>),
}

pub struct ApiClient {
    base_url: String,
    transport: Arc<dyn HttpTransport>,
    secure_store: Arc<dyn SecureStore>,
    /// The session cookie, held in memory in front of [`ApiClient::secure_store`] (task ec54c0a5).
    ///
    /// `SecureStore::get` is a trip out of Rust and into the platform's credential vault — a
    /// UniFFI foreign callback into the Keychain on Apple, Keystore over JNA on Android, the
    /// Credential Locker here. [`ApiClient::build`] needs the cookie on every request, so paying
    /// for it per request means a sync walking twenty pages crosses that boundary twenty times for
    /// a value that changes twice in a session: once at sign-in and once at sign-out.
    ///
    /// The store stays the durable source of truth. This is a read-through cache in front of it,
    /// and it is correct only because this client is the one gateway to the credential: sign-in
    /// goes through [`ApiClient::set_session_cookie`], sign-out through
    /// [`ApiClient::clear_session_cookie`], and a 401 through
    /// [`ApiClient::forget_session_cookie`]. A writer this client cannot see would make it stale,
    /// which is why there is no longer a way to reach the key except through here.
    session_cookie: std::sync::RwLock<CachedCookie>,
    /// What this build states in `x-platform` — see [`platform_header`].
    platform: platform_header::Platform,
    /// Which running client this is — see [`CLIENT_ID_HEADER`]. New for every client, never
    /// persisted: two app instances that shared one would each miss the other's edits.
    client_id: String,
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
            session_cookie: std::sync::RwLock::new(CachedCookie::Unknown),
            platform: platform_header::Platform::default(),
            client_id: format!("core_{}", uuid::Uuid::new_v4().simple()),
        }
    }

    /// Which running client this is: sent on every request and on the live stream's URL.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// State a different platform on every request — the Apple apps' shells set theirs.
    pub fn with_platform(mut self, platform: platform_header::Platform) -> Self {
        self.platform = platform;
        self
    }

    /// Which client this build states it is — the platform header, and the sign-in client.
    pub fn platform(&self) -> platform_header::Platform {
        self.platform
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The credential store this client authenticates from.
    ///
    /// Exposed so sign-in and sign-out write the session to the same place every request reads it
    /// from. Two references to one store, rather than two stores that agree until they do not.
    ///
    /// **The session cookie is not read or written through here** — it goes through
    /// [`ApiClient::session_cookie`] and friends, which keep the in-memory copy honest. This is
    /// for the other things that sit beside the credential, such as
    /// [`crate::platform::CURRENT_USER_ID_KEY`].
    pub fn secure_store(&self) -> &Arc<dyn SecureStore> {
        &self.secure_store
    }

    /// The session cookie every request carries, from memory once it is known.
    ///
    /// The first caller pays for the credential-vault read and every caller after it does not,
    /// until something invalidates the cache. See [`ApiClient::session_cookie`]'s field for why
    /// that matters (task ec54c0a5).
    pub async fn session_cookie(&self) -> Option<String> {
        if let CachedCookie::Known(cookie) = self.cached_cookie() {
            return cookie;
        }
        // Deliberately not holding the lock across this await. Two requests racing the very first
        // read both ask the store and both write the same answer, which costs one extra vault call
        // once; holding a lock across a platform callback would serialise every request in the app
        // behind it, and on Apple that callback can show UI.
        let cookie = self.secure_store.get(SESSION_COOKIE_KEY).await;
        self.remember_cookie(CachedCookie::Known(cookie.clone()));
        cookie
    }

    /// Store a new session and start serving it. Sign-in's half.
    ///
    /// Caches what it was given rather than reading back what it wrote: the value is already in
    /// hand, so a confirming read would be a vault call that can only agree.
    pub async fn set_session_cookie(
        &self,
        cookie: &str,
    ) -> Result<(), crate::platform::PlatformError> {
        self.secure_store.set(SESSION_COOKIE_KEY, cookie).await?;
        self.remember_cookie(CachedCookie::Known(Some(cookie.to_string())));
        Ok(())
    }

    /// Delete the session, from the store and from memory. Sign-out's half.
    ///
    /// On a failed delete the cache drops to `Unknown` rather than to "no session": the store may
    /// still hold the credential, and the next request should find that out from the store instead
    /// of this client deciding it.
    pub async fn clear_session_cookie(&self) -> Result<(), crate::platform::PlatformError> {
        match self.secure_store.delete(SESSION_COOKIE_KEY).await {
            Ok(()) => {
                self.remember_cookie(CachedCookie::Known(None));
                Ok(())
            }
            Err(error) => {
                self.remember_cookie(CachedCookie::Unknown);
                Err(error)
            }
        }
    }

    /// Drop the cached copy without touching the store — what a 401 does.
    ///
    /// The server has refused the credential this client is holding. That does not mean the stored
    /// one is bad: a sign-in on another thread may already have replaced it, and the Outbox parks
    /// an unauthorised entry precisely so it can be retried once there is a new session. So the
    /// next request asks the store again rather than assuming either answer.
    pub fn forget_session_cookie(&self) {
        self.remember_cookie(CachedCookie::Unknown);
    }

    fn cached_cookie(&self) -> CachedCookie {
        self.session_cookie
            .read()
            .expect("session cookie lock")
            .clone()
    }

    fn remember_cookie(&self, cached: CachedCookie) {
        *self.session_cookie.write().expect("session cookie lock") = cached;
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
    ///
    /// Straight from the response bytes rather than through a `serde_json::Value` first (task
    /// ec54c0a5): building the whole tree only to walk it again allocates every string and every
    /// map in the body twice, which on a page of a hundred tasks is the bulk of what the call
    /// does. The empty-body case still goes through `Value::Null`, because a 204 on a DELETE has
    /// to decode rather than fail — see [`ApiClient::send`].
    pub async fn send_as<T: DeserializeOwned>(&self, request: Request) -> Result<T, ApiError> {
        let response = self.send_raw(request).await?;
        if response.body.is_empty() {
            return serde_json::from_value(serde_json::Value::Null)
                .map_err(|error| ApiError::Decode(error.to_string()));
        }
        serde_json::from_slice(&response.body).map_err(|error| ApiError::Decode(error.to_string()))
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
            // bare array. [`endpoints::envelope::unwrap`] is the one place that reconciles them.
            Some(key) => endpoints::envelope::unwrap(value, key),
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
            self.forget_session_cookie();
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

    /// Send a conditional GET: `None` **is** the 304.
    ///
    /// Task 5751fcb8. A 304 is not an error and it is not a success either — `is_success()` is
    /// `200..300`, so [`ApiClient::send_raw`] reports one as `ApiError::Http { status: 304 }`, and
    /// anything that went through [`ApiClient::send`] would decode its empty body to
    /// `Value::Null` and hand back "nothing" for "nothing changed". Those are opposite answers,
    /// and a caller that cached the second would wipe what it had on the very tick that proved it
    /// was still right.
    ///
    /// Passing `etag: None` is an ordinary unconditional GET, which is what the first read of
    /// anything is.
    pub async fn send_conditional(
        &self,
        request: Request,
        etag: Option<&str>,
    ) -> Result<Option<HttpResponse>, ApiError> {
        let request = match etag {
            Some(etag) => request.header("if-none-match", etag),
            None => request,
        };
        let http = self.build(request).await?;
        let response = self.transport.send(http).await?;

        if response.status == 401 {
            self.forget_session_cookie();
            return Err(ApiError::Unauthorized);
        }
        if response.status == 304 {
            return Ok(None);
        }
        if !response.is_success() {
            return Err(ApiError::Http {
                status: response.status,
                message: response.text(),
            });
        }
        Ok(Some(response))
    }

    /// Open a long-lived event stream, with the same headers every other request carries.
    ///
    /// Built here rather than by the real-time module so the stream states the platform and sends
    /// the session cookie. On Apple, SSE built its own request and identified itself as nothing —
    /// which is a shame, because a live connection is the clearest evidence the app is open.
    pub async fn open_stream(
        &self,
        request: Request,
    ) -> Result<super::transport::FrameStream, ApiError> {
        let mut http = self.build(request).await?;
        // Replace rather than add: two Accept headers is a request that asks for both and is
        // answered with whichever the server prefers, which is not a decision to leave open.
        http.headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("accept"));
        http.headers
            .push(("accept".to_string(), "text/event-stream".to_string()));
        http.headers
            .push(("cache-control".to_string(), "no-cache".to_string()));
        match self.transport.open_stream(http).await {
            Ok(frames) => Ok(frames),
            // A refused stream carries a status, and 401 is the one that means "stop", not "wait".
            Err(TransportError::Refused(401)) => {
                self.forget_session_cookie();
                Err(ApiError::Unauthorized)
            }
            Err(TransportError::Refused(status)) => Err(ApiError::Http {
                status,
                message: "the live stream was refused".to_string(),
            }),
            Err(error) => Err(ApiError::Transport(error)),
        }
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

        let mut headers = vec![
            ("accept".to_string(), "application/json".to_string()),
            (
                platform_header::HEADER_NAME.to_string(),
                self.platform.wire().to_string(),
            ),
            (CLIENT_ID_HEADER.to_string(), self.client_id.clone()),
        ];
        if let Some(cookie) = self.session_cookie().await {
            headers.push(("cookie".to_string(), cookie));
        }
        headers.extend(request.headers);

        let body = match (request.raw, request.body) {
            (Some((content_type, bytes)), _) => {
                headers.push(("content-type".to_string(), content_type));
                Some(bytes)
            }
            (None, Some(value)) => {
                headers.push(("content-type".to_string(), "application/json".to_string()));
                Some(serde_json::to_vec(&value).map_err(|e| ApiError::Decode(e.to_string()))?)
            }
            (None, None) => None,
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

    /// AITD-574: a task edited on the iPhone reached the open Mac list only on the 60-second sync.
    /// The server leaves the actor's own devices out of an update unless the write names the
    /// client that made it (astrid-web AWTD-1497, `lib/realtime/client-id.ts`). So every request
    /// names this running client, with one id for its lifetime, in the shape the server accepts;
    /// and another client — another app on the same account — has its own.
    #[tokio::test]
    async fn aitd_574_every_request_names_this_client_with_one_id() {
        let (client, transport) = client_with(
            StubTransport::new()
                .push_json("/api/v1/tasks", 200, serde_json::json!([]))
                .push_json("/api/v1/tasks", 200, serde_json::json!([])),
        );
        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");
        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");

        let sent = transport.requests();
        let first = sent[0]
            .header(CLIENT_ID_HEADER)
            .expect("names the client")
            .to_string();
        assert_eq!(sent[1].header(CLIENT_ID_HEADER), Some(first.as_str()));
        assert_eq!(first, client.client_id());
        assert!(
            (8..=64).contains(&first.len())
                && first
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "the server ignores an id outside [A-Za-z0-9_-]{{8,64}}: {first}"
        );

        let (other, _) = client_with(StubTransport::new());
        assert_ne!(other.client_id(), client.client_id());
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

    /// Rule 4 of README.md, enforced rather than remembered.
    #[tokio::test]
    async fn a_path_outside_v1_is_refused() {
        let (client, _) = client_with(StubTransport::new());
        assert!(matches!(
            client.send(client.get("/api/tasks")).await,
            Err(ApiError::Refused(_))
        ));
    }

    /// Four paths have no v1 equivalent. They are named, not waved through by loosening the rule —
    /// so a sibling under the very same `/api/auth/webauthn/` prefix is still refused (task
    /// 4c285039, which added the two registration paths).
    #[tokio::test]
    async fn the_legacy_paths_are_allowed_one_by_one_and_their_siblings_are_not() {
        for path in [
            "/api/auth/desktop/grant",
            "/api/mcp/user-tokens",
            endpoints::WEBAUTHN_REGISTER_OPTIONS,
            endpoints::WEBAUTHN_REGISTER_VERIFY,
        ] {
            let (client, _) =
                client_with(StubTransport::new().push_json(path, 200, serde_json::json!({})));
            assert!(
                client.send(client.get(path)).await.is_ok(),
                "{path} is on the allow-list"
            );
        }

        let (client, _) = client_with(StubTransport::new());
        assert!(
            matches!(
                client
                    .send(client.get("/api/auth/webauthn/authenticate/options"))
                    .await,
                Err(ApiError::Refused(_))
            ),
            "signing in with a passkey is the shell's browser hand-off, not this client's"
        );
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

    /// Task ec54c0a5. The session cookie is read from the credential vault once and then served
    /// from memory.
    ///
    /// On Apple that `get` is a UniFFI foreign callback into the Keychain and on Android it is
    /// Keystore over JNA, so "once per request" is a per-request trip out of Rust and into the
    /// platform — paid by every sync page and every Outbox drain, which are exactly the places
    /// that send requests in bulk.
    #[tokio::test]
    async fn the_session_cookie_is_read_from_the_store_once_across_many_requests() {
        let store = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let transport = Arc::new(StubTransport::new().fallback(Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"[]".to_vec(),
        })));
        let client = ApiClient::new("https://astrid.cc", transport.clone(), store.clone());

        for _ in 0..100 {
            client
                .send(client.get("/api/v1/tasks"))
                .await
                .expect("succeeds");
        }

        assert_eq!(transport.requests().len(), 100);
        assert_eq!(
            store.reads_of(SESSION_COOKIE_KEY),
            1,
            "100 requests must cost one credential-vault read, not 100"
        );
        for sent in transport.requests() {
            assert_eq!(sent.header("cookie"), Some("next-auth.session-token=abc"));
        }
    }

    /// Signed out, the cache has to hold "there is no session" as an answer in its own right —
    /// otherwise a signed-out app calls into the vault on every request, which is the same bug
    /// wearing the other hat.
    #[tokio::test]
    async fn with_no_session_the_store_is_still_only_asked_once() {
        let store = Arc::new(MemorySecureStore::new());
        let transport = Arc::new(StubTransport::new().fallback(Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"[]".to_vec(),
        })));
        let client = ApiClient::new("https://astrid.cc", transport.clone(), store.clone());

        for _ in 0..10 {
            client
                .send(client.get("/api/v1/tasks"))
                .await
                .expect("succeeds");
        }

        assert_eq!(store.reads_of(SESSION_COOKIE_KEY), 1);
        assert_eq!(transport.requests()[9].header("cookie"), None);
    }

    /// Task ec54c0a5. A 401 is the server saying the credential this client is holding is not
    /// good, so the cached copy is dropped — but *not* the stored one, which may already have been
    /// replaced by a sign-in. The next request asks the store rather than assuming either answer.
    #[tokio::test]
    async fn a_401_drops_the_cached_session_and_the_next_request_reads_the_store_again() {
        let store = Arc::new(MemorySecureStore::with(SESSION_COOKIE_KEY, "stale=1"));
        let transport = Arc::new(
            StubTransport::new()
                .push_json("/api/v1/tasks", 200, serde_json::json!([]))
                .push_json("/api/v1/tasks", 401, serde_json::json!({}))
                .push_json("/api/v1/tasks", 200, serde_json::json!([])),
        );
        let client = ApiClient::new("https://astrid.cc", transport.clone(), store.clone());

        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");
        assert_eq!(store.reads_of(SESSION_COOKIE_KEY), 1);

        assert!(matches!(
            client.send(client.get("/api/v1/tasks")).await,
            Err(ApiError::Unauthorized)
        ));

        // The session was renewed out from under the client while it was being refused.
        store
            .set(SESSION_COOKIE_KEY, "fresh=2")
            .await
            .expect("stores");

        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");
        assert_eq!(
            store.reads_of(SESSION_COOKIE_KEY),
            2,
            "the 401 must have invalidated the cache"
        );
        assert_eq!(transport.requests()[2].header("cookie"), Some("fresh=2"));
    }

    /// The write half: sign-in hands the cookie to the client, which stores it durably *and*
    /// starts serving it — so the first authenticated request after signing in needs no vault
    /// read at all.
    #[tokio::test]
    async fn setting_the_session_stores_it_and_primes_the_cache() {
        let store = Arc::new(MemorySecureStore::new());
        let transport = Arc::new(StubTransport::new().fallback(Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"[]".to_vec(),
        })));
        let client = ApiClient::new("https://astrid.cc", transport.clone(), store.clone());

        client
            .set_session_cookie("next-auth.session-token=new")
            .await
            .expect("stores");

        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");

        assert_eq!(
            transport.requests()[0].header("cookie"),
            Some("next-auth.session-token=new")
        );
        assert_eq!(
            store.reads_of(SESSION_COOKIE_KEY),
            0,
            "sign-in already knows the cookie; reading it back is a trip it has no reason to make"
        );
        // Durable, not only cached: a restart has to find it.
        assert_eq!(
            store.get(SESSION_COOKIE_KEY).await.as_deref(),
            Some("next-auth.session-token=new")
        );
    }

    /// And the clear half, which sign-out uses: gone from the store and gone from memory, in one
    /// call, so the two can never disagree.
    #[tokio::test]
    async fn clearing_the_session_empties_the_store_and_the_cache() {
        let store = Arc::new(MemorySecureStore::with(SESSION_COOKIE_KEY, "signed-in=1"));
        let transport = Arc::new(StubTransport::new().fallback(Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"[]".to_vec(),
        })));
        let client = ApiClient::new("https://astrid.cc", transport.clone(), store.clone());

        // Prime the cache the way a signed-in session would.
        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");
        assert_eq!(
            transport.requests()[0].header("cookie"),
            Some("signed-in=1")
        );

        client.clear_session_cookie().await.expect("clears");

        assert_eq!(client.session_cookie().await, None);
        assert_eq!(store.get(SESSION_COOKIE_KEY).await, None);
        client
            .send(client.get("/api/v1/tasks"))
            .await
            .expect("succeeds");
        assert_eq!(transport.requests()[1].header("cookie"), None);
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
