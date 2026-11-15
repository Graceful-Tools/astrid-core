//! The seam between "what request to send" and "how bytes reach the network".
//!
//! Everything above this file — the client, the services, the Outbox — builds an [`HttpRequest`]
//! and reads an [`HttpResponse`]. Only [`ReqwestTransport`] touches a socket.
//!
//! The seam exists for two reasons, in this order:
//!
//! 1. **The tests.** The Apple apps stub `URLProtocol` to test their client; without an equivalent
//!    here, every test of a service would need a live server, and the ones that matter — a 401
//!    mid-sync, a 500 on the third retry, a body that decodes to nothing — are exactly the ones a
//!    live server will not produce on demand. [`StubTransport`] makes them ordinary unit tests.
//! 2. **The platform boundary.** A transport is a thing the shell may one day need to supply (a
//!    proxy, a certificate pinned by policy, a UI-test build that must never reach the network).
//!    Having the trait costs nothing now and avoids a rewrite then.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{Stream, StreamExt};

/// How long a single request may take before it is abandoned. Matches the Apple client's
/// `Constants.API.timeout`.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    /// Absolute URL, already built and already checked by [`super::path`].
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as text, for error messages. Lossy on purpose: an error path must not fail again
    /// on the encoding of the thing it is reporting.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A request that never reached a server, or never came back.
///
/// Distinguished from an HTTP error status because the two are handled completely differently:
/// this one means "we are offline or the network broke", which the Outbox retries forever and the
/// UI reports as offline; a 4xx means the server considered the request and refused it.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TransportError {
    #[error("the request timed out")]
    Timeout,
    #[error("could not reach the server: {0}")]
    Unreachable(String),
    #[error("the request could not be built: {0}")]
    Invalid(String),
    /// The server answered and refused. Only a stream reports this way: an ordinary request
    /// carries its status in [`HttpResponse`], but a stream that is refused never becomes one, and
    /// a 401 on the live stream has to be told apart from a network that is not there.
    #[error("the server refused the stream with {0}")]
    Refused(u16),
    /// Android only: the app has not yet handed the platform certificate verifier its JVM
    /// ([`super::tls::android::init`]), so nothing can be verified and nothing was sent. Retryable:
    /// the init is a startup step, and a write queued before it should wait for it, not be dropped
    /// (AWTD-1673).
    #[error(
        "the platform certificate verifier has not been initialised; the app must call the TLS init before the first request"
    )]
    TlsNotInitialized,
}

impl TransportError {
    /// Whether waiting and trying again is worth anything. Every transport failure is retryable —
    /// including a malformed request, because the malformed part is usually a URL built from data
    /// that a later sync corrects.
    pub fn is_retryable(&self) -> bool {
        match self {
            TransportError::Invalid(_) => false,
            TransportError::Refused(status) => *status >= 500,
            _ => true,
        }
    }
}

/// A long-lived stream of server-sent-event frames, already split on the blank line between them.
pub type FrameStream = Pin<Box<dyn Stream<Item = Result<String, TransportError>> + Send>>;

#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;

    /// Open a long-lived event stream.
    ///
    /// Separate from [`HttpTransport::send`] because the two have nothing in common at the socket:
    /// one reads a body to the end, the other must never do that. It lives on this trait anyway,
    /// rather than in a module of its own, so the live stream is built by the same code that
    /// attaches the session cookie and the platform header — the Apple client built its SSE
    /// request by hand and that stream identified itself as nothing for a year.
    ///
    /// The default refuses: a transport that cannot stream should say so rather than silently
    /// never delivering an event.
    async fn open_stream(&self, request: HttpRequest) -> Result<FrameStream, TransportError> {
        let _ = request;
        Err(TransportError::Invalid(
            "this transport does not open streams".to_string(),
        ))
    }
}

/// The real one, over the platform's TLS stack. See the workspace `Cargo.toml` for why it is the
/// platform's and not a bundled one, and `crates/astrid-core/Cargo.toml` for Android's.
pub struct ReqwestTransport {
    /// Built on the first request that passes the gate rather than up front, because on Android the
    /// verifier inside it cannot be relied on before the app's TLS init. Everywhere else
    /// [`ReqwestTransport::new`] builds it at once, exactly as before.
    client: OnceLock<reqwest::Client>,
    /// Asked before every request: whether this process's TLS can verify a certificate yet
    /// ([`super::tls::platform_tls_ready`] on Android, always yes elsewhere).
    ready: fn() -> bool,
}

impl ReqwestTransport {
    #[cfg(not(target_os = "android"))]
    pub fn new() -> Self {
        Self::with_client(default_client().expect("the default HTTP client always builds"))
    }

    /// On Android the client waits for the app's TLS init ([`super::tls::android::init`]); a
    /// request before it is [`TransportError::TlsNotInitialized`], not a panic in the verifier.
    #[cfg(target_os = "android")]
    pub fn new() -> Self {
        ReqwestTransport {
            client: OnceLock::new(),
            ready: super::tls::platform_tls_ready,
        }
    }

    /// A caller's own client, used as given: whatever TLS it was built with is the caller's to
    /// have initialised.
    pub fn with_client(client: reqwest::Client) -> Self {
        ReqwestTransport {
            client: OnceLock::from(client),
            ready: || true,
        }
    }

    /// The default client behind a readiness gate of the test's choosing, so the pre-init path is
    /// testable on a target whose real gate is always open.
    #[cfg(test)]
    fn gated_by(ready: fn() -> bool) -> Self {
        ReqwestTransport {
            client: OnceLock::new(),
            ready,
        }
    }

    fn client(&self) -> Result<&reqwest::Client, TransportError> {
        if !(self.ready)() {
            return Err(TransportError::TlsNotInitialized);
        }
        if let Some(client) = self.client.get() {
            return Ok(client);
        }
        let built = default_client()?;
        Ok(self.client.get_or_init(|| built))
    }
}

/// The client every target ships: the platform's TLS (native-tls, from the workspace manifest).
#[cfg(not(target_os = "android"))]
fn default_client() -> Result<reqwest::Client, TransportError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| TransportError::Invalid(error.to_string()))
}

/// Android's: rustls, with certificates verified by Android's own trust manager through
/// `rustls-platform-verifier` rather than a root bundle compiled into the library (AWTD-1673).
#[cfg(target_os = "android")]
fn default_client() -> Result<reqwest::Client, TransportError> {
    use rustls_platform_verifier::BuilderVerifierExt;

    let invalid = |error: rustls::Error| TransportError::Invalid(error.to_string());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(invalid)?
        .with_platform_verifier()
        .map_err(invalid)?
        .with_no_client_auth();
    // What reqwest's own rustls config offers; a preconfigured one is used exactly as given, and
    // without this every connection would settle for HTTP/1.1.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .use_preconfigured_tls(config)
        .build()
        .map_err(|error| TransportError::Invalid(error.to_string()))
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_str().as_bytes())
            .map_err(|error| TransportError::Invalid(error.to_string()))?;
        let mut builder = self.client()?.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }

        let response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                TransportError::Timeout
            } else if error.is_builder() || error.is_request() && error.url().is_none() {
                TransportError::Invalid(error.to_string())
            } else {
                TransportError::Unreachable(error.to_string())
            }
        })?;

        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|error| TransportError::Unreachable(error.to_string()))?
            .to_vec();

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    async fn open_stream(&self, request: HttpRequest) -> Result<FrameStream, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_str().as_bytes())
            .map_err(|error| TransportError::Invalid(error.to_string()))?;
        let mut builder = self
            .client()?
            .request(method, &request.url)
            // No timeout: the whole point of this connection is to stay open with nothing on it.
            // The request timeout that protects an ordinary call would close a healthy stream
            // every thirty seconds, which reads as a server that keeps dropping the connection.
            .timeout(Duration::from_secs(u32::MAX as u64));
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }

        let response = builder
            .send()
            .await
            .map_err(|error| TransportError::Unreachable(error.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(TransportError::Refused(status));
        }

        // Frames are separated by a blank line and can arrive across any number of reads — and a
        // read can end inside a character. `FrameBuffer` holds bytes until an event is whole.
        let mut buffer = super::frames::FrameBuffer::default();
        let stream = response.bytes_stream().flat_map(move |chunk| {
            let frames = match chunk {
                Ok(bytes) => buffer.push(&bytes).into_iter().map(Ok).collect(),
                Err(error) => vec![Err(TransportError::Unreachable(error.to_string()))],
            };
            futures_util::stream::iter(frames)
        });
        Ok(Box::pin(stream))
    }
}

/// A transport that answers from a script instead of a network.
///
/// Available outside `cfg(test)` because the shell's UI-test build needs it too: a UI test that
/// can reach the network can sign in as the real user, which is how the Apple repo learned to
/// harden its test session.
pub struct StubTransport {
    responses: Mutex<HashMap<String, Vec<Result<HttpResponse, TransportError>>>>,
    fallback: Mutex<Option<Result<HttpResponse, TransportError>>>,
    /// Every request that was sent, in order, for a test to assert on.
    pub recorded: Arc<Mutex<Vec<HttpRequest>>>,
}

impl StubTransport {
    pub fn new() -> Self {
        StubTransport {
            responses: Mutex::new(HashMap::new()),
            fallback: Mutex::new(None),
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Queue a response for the next request whose URL contains `url_fragment`. Queued responses
    /// are consumed in order, so a test can script "fails, fails, then succeeds".
    pub fn push(
        mut self,
        url_fragment: &str,
        response: Result<HttpResponse, TransportError>,
    ) -> Self {
        self.responses
            .get_mut()
            .expect("the stub is not shared while being built")
            .entry(url_fragment.to_string())
            .or_default()
            .push(response);
        self
    }

    pub fn push_json(self, url_fragment: &str, status: u16, body: serde_json::Value) -> Self {
        self.push(
            url_fragment,
            Ok(HttpResponse {
                status,
                headers: vec![("content-type".into(), "application/json".into())],
                body: body.to_string().into_bytes(),
            }),
        )
    }

    /// What to answer when nothing is queued for a URL. Without one, an unscripted request is a
    /// test failure rather than a silent empty response — which is the behaviour you want, since
    /// the alternative is a test that passes because the code under test never called anything.
    pub fn fallback(mut self, response: Result<HttpResponse, TransportError>) -> Self {
        *self
            .fallback
            .get_mut()
            .expect("the stub is not shared while being built") = Some(response);
        self
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.recorded.lock().expect("stub lock").clone()
    }
}

impl Default for StubTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpTransport for StubTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.recorded
            .lock()
            .expect("stub lock")
            .push(request.clone());

        let mut responses = self.responses.lock().expect("stub lock");
        let matching = responses.iter_mut().find(|(fragment, queued)| {
            request.url.contains(fragment.as_str()) && !queued.is_empty()
        });
        if let Some((_, queued)) = matching {
            return queued.remove(0);
        }
        drop(responses);

        match self.fallback.lock().expect("stub lock").clone() {
            Some(response) => response,
            None => Err(TransportError::Invalid(format!(
                "no stubbed response for {} {}",
                request.method.as_str(),
                request.url
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_stub_answers_in_the_order_it_was_scripted() {
        let transport = StubTransport::new()
            .push_json("/api/v1/tasks", 500, serde_json::json!({ "error": "boom" }))
            .push_json("/api/v1/tasks", 200, serde_json::json!([{ "id": "t1" }]));

        let request = HttpRequest {
            method: Method::Get,
            url: "https://astrid.cc/api/v1/tasks".into(),
            headers: Vec::new(),
            body: None,
        };
        assert_eq!(
            transport
                .send(request.clone())
                .await
                .expect("stubbed")
                .status,
            500
        );
        assert_eq!(transport.send(request).await.expect("stubbed").status, 200);
        assert_eq!(transport.requests().len(), 2);
    }

    /// A request nobody scripted is a bug in the test, and it has to read as one. Answering it
    /// with an empty 200 makes a test pass while proving nothing.
    #[tokio::test]
    async fn an_unscripted_request_fails_loudly() {
        let transport = StubTransport::new();
        let error = transport
            .send(HttpRequest {
                method: Method::Get,
                url: "https://astrid.cc/api/v1/lists".into(),
                headers: Vec::new(),
                body: None,
            })
            .await
            .expect_err("nothing was scripted");
        assert!(matches!(error, TransportError::Invalid(_)));
    }

    /// Offline is not a refusal. The Outbox keeps retrying a transport failure and gives up on a
    /// 4xx, so the two must never collapse into one error type.
    #[test]
    fn transport_failures_are_retryable_unless_the_request_itself_is_wrong() {
        assert!(TransportError::Timeout.is_retryable());
        assert!(TransportError::Unreachable("dns".into()).is_retryable());
        assert!(!TransportError::Invalid("bad url".into()).is_retryable());
    }

    fn get(url: &str) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// AWTD-1673: on Android the certificate verifier needs the app's JVM before it can verify
    /// anything, and it panics when asked first. A request made before the init must come back as
    /// a typed transport error — never a panic, and never a request on the wire. The gate is the
    /// same function on every target, so this runs on the desktop with a gate that says "not yet".
    #[tokio::test]
    async fn a_request_before_the_platform_tls_init_is_a_typed_error_awtd_1673() {
        let transport = ReqwestTransport::gated_by(|| false);
        // A port nothing listens on: had the gate let it through, the error would be Unreachable.
        let error = transport
            .send(get("https://127.0.0.1:9/api/v1/tasks"))
            .await
            .expect_err("the verifier is not ready");
        assert!(
            matches!(error, TransportError::TlsNotInitialized),
            "{error:?}"
        );

        let error = match transport
            .open_stream(get("https://127.0.0.1:9/api/v1/events"))
            .await
        {
            Ok(_) => panic!("the verifier is not ready"),
            Err(error) => error,
        };
        assert!(
            matches!(error, TransportError::TlsNotInitialized),
            "{error:?}"
        );
    }

    /// Retryable, so a write queued before the init waits in the Outbox instead of being dropped.
    #[test]
    fn a_missing_tls_init_is_retryable_awtd_1673() {
        assert!(TransportError::TlsNotInitialized.is_retryable());
    }

    /// Once the gate opens, the request goes out as before.
    #[tokio::test]
    async fn a_ready_gate_lets_the_request_through_awtd_1673() {
        let transport = ReqwestTransport::gated_by(|| true);
        let error = transport
            .send(get("http://127.0.0.1:9/api/v1/tasks"))
            .await
            .expect_err("nothing listens on the discard port");
        assert!(
            !matches!(error, TransportError::TlsNotInitialized),
            "{error:?}"
        );
    }

    #[test]
    fn headers_are_read_without_regard_to_case() {
        let response = HttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Vec::new(),
        };
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert!(response.is_success());
    }
}
