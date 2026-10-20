//! Signing in, and staying signed in.
//!
//! The client half of the browser hand-off, whose rules are in [`crate::auth::desktop_handoff`]
//! and whose server half lives in `astrid-web`. This service is the part that has state: it holds
//! the flow in progress between opening the browser and the callback coming back.
//!
//! ## Why the browser at all
//!
//! Astrid's sign-in is passkeys, Google, and a magic link. Reimplementing three of those in a
//! desktop app means three more places to get an OAuth redirect wrong, and it means a passkey
//! prompt inside an app window that cannot use the platform authenticator the way a browser can.
//! Handing off to the browser the user is already signed into is both less code and a better
//! experience — and it is what the Mac app does.
//!
//! ## The flow, and where it can be attacked
//!
//! 1. [`AuthService::begin`] mints a PKCE verifier and a random state, and returns the URL to open.
//! 2. The browser signs the person in and redirects to `astrid://auth/callback?code=…&state=…`.
//! 3. Windows activates the app with that URL. [`AuthService::complete`] checks the state against
//!    the flow in progress **before** it looks at the code, then exchanges the code with the
//!    verifier.
//!
//! Any web page can open `astrid://auth/callback?code=…`. The state check is what makes that
//! harmless: an activation that does not match the flow this app started is refused before its
//! code is used. There being no flow in progress is refused too — that is the case where somebody
//! sent a link to an app that was not signing in.

use std::sync::Mutex;

use super::{Context, Result};
use crate::auth::desktop_handoff::{self, ExchangeRequest, ExchangeResponse, HandoffFlow};
use crate::model::User;
use crate::platform::SESSION_COOKIE_KEY;

/// Where the exchange happens. Outside `/api/v1` is not allowed by the client's path rule, so this
/// one is on the versioned prefix — see `astrid-web`'s route of the same name.
const EXCHANGE_PATH: &str = "/api/v1/auth/desktop/exchange";

/// The scheme Windows activates the app on. Registered by the installer; see `docs/PARITY.md`.
///
/// Astrid's, and the default. A white-label shell registers its own and names it in
/// [`crate::app::Config::url_scheme`] (AWTD2-95), so two brands installed side by side do not fight
/// over each other's sign-in callbacks.
pub const URL_SCHEME: &str = "astrid";

pub struct AuthService {
    context: Context,
    /// The flow between opening the browser and the callback arriving.
    ///
    /// One at a time on purpose: starting a second sign-in replaces the first, so a callback for
    /// an abandoned attempt is refused by the state check rather than quietly completing.
    flow: Mutex<Option<HandoffFlow>>,
    /// The scheme a callback must arrive on, without `://`. [`URL_SCHEME`] unless the shell said
    /// otherwise.
    url_scheme: String,
    /// The id the server's hand-off registry knows this app by, when it is not the platform's own
    /// ([`crate::api::Platform::handoff_client`]) — a partner build registered under its own id.
    handoff_client: Option<String>,
}

impl AuthService {
    pub fn new(context: Context) -> Self {
        AuthService {
            context,
            flow: Mutex::new(None),
            url_scheme: URL_SCHEME.to_string(),
            handoff_client: None,
        }
    }

    /// Accept callbacks on `scheme` instead of `astrid` (AWTD2-95). Blank keeps Astrid's.
    pub fn with_url_scheme(mut self, scheme: &str) -> Self {
        let scheme = scheme.trim();
        self.url_scheme = if scheme.is_empty() {
            URL_SCHEME
        } else {
            scheme
        }
        .to_string();
        self
    }

    /// Sign in as `client` instead of the platform's registered id (AWTD2-95). `None` or blank
    /// keeps the platform's.
    pub fn with_handoff_client(mut self, client: Option<&str>) -> Self {
        self.handoff_client = client
            .map(str::trim)
            .filter(|client| !client.is_empty())
            .map(str::to_string);
        self
    }

    /// The scheme a sign-in callback must arrive on.
    pub fn url_scheme(&self) -> &str {
        &self.url_scheme
    }

    /// The id sent as `client` in both halves of the hand-off.
    pub fn handoff_client(&self) -> &str {
        self.handoff_client
            .as_deref()
            .unwrap_or_else(|| self.context.client.platform().handoff_client())
    }

    /// Whether there is a stored session. Not whether it is still valid — only the server knows
    /// that, and it says so with a 401.
    pub async fn is_signed_in(&self) -> bool {
        self.context
            .client
            .secure_store()
            .get(SESSION_COOKIE_KEY)
            .await
            .is_some_and(|cookie| !cookie.trim().is_empty())
    }

    /// Start signing in. Returns the URL for the shell to open in the browser.
    pub fn begin(&self) -> Result<String> {
        self.begin_with(None)
    }

    /// As [`AuthService::begin`], asking the hand-off page to start `provider` directly.
    pub fn begin_with(&self, provider: Option<&str>) -> Result<String> {
        let flow = desktop_handoff::begin_flow()
            .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;
        let url = desktop_handoff::authorize_url(
            self.context.client.base_url(),
            self.handoff_client(),
            &flow,
        )
        .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;
        let url = match provider.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => {
                let mut url = url::Url::parse(&url)
                    .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;
                url.query_pairs_mut().append_pair("provider", name);
                url.into()
            }
            None => url,
        };
        *self.flow.lock().expect("auth lock") = Some(flow);
        Ok(url)
    }

    /// Whether a sign-in is waiting for its callback.
    pub fn is_waiting(&self) -> bool {
        self.flow.lock().expect("auth lock").is_some()
    }

    /// Abandon the flow in progress — the user closed the sign-in prompt.
    pub fn cancel(&self) {
        *self.flow.lock().expect("auth lock") = None;
    }

    /// Finish signing in from the URL Windows activated the app with.
    ///
    /// The flow is taken out of the slot before anything is checked, so a callback can only be
    /// used once: a replayed activation finds nothing in progress and is refused.
    pub async fn complete(&self, callback_url: &str) -> Result<User> {
        let flow = self
            .flow
            .lock()
            .expect("auth lock")
            .take()
            .ok_or_else(|| crate::api::ApiError::Refused("no sign-in is in progress".into()))?;

        let callback = desktop_handoff::parse_callback(callback_url, &flow, &self.url_scheme)
            .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;

        let request = ExchangeRequest::new(self.handoff_client(), &callback, &flow);
        let value = self
            .context
            .client
            .send(self.context.client.post(EXCHANGE_PATH).json(&request))
            .await?;
        let exchange: ExchangeResponse = serde_json::from_value(value)
            .map_err(|error| crate::api::ApiError::Decode(error.to_string()))?;

        // The cookie goes in first. Everything after this point is an authenticated request, and
        // the very next one — fetching the user — would 401 if the credential were stored after.
        self.context
            .client
            .secure_store()
            .set(SESSION_COOKIE_KEY, &exchange.cookie_header())
            .await
            .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;

        let user = serde_json::from_value::<User>(serde_json::json!({
            "id": exchange.user.id,
            "email": exchange.user.email,
            "name": exchange.user.name,
            "image": exchange.user.image,
        }))
        .map_err(|error| crate::api::ApiError::Decode(error.to_string()))?;
        self.context.account().set_current_user(&user).await?;
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport};
    use crate::platform::{FixedClock, MemorySecureStore, SecureStore};
    use crate::store::Store;
    use std::sync::Arc;

    struct Fixture {
        auth: AuthService,
        secure: Arc<MemorySecureStore>,
    }

    fn fixture(transport: StubTransport) -> Fixture {
        fixture_on(transport, crate::api::Platform::default())
    }

    fn fixture_on(transport: StubTransport, platform: crate::api::Platform) -> Fixture {
        let secure = Arc::new(MemorySecureStore::new());
        let context = Context::new(
            Arc::new(
                ApiClient::new("https://astrid.cc", Arc::new(transport), secure.clone())
                    .with_platform(platform),
            ),
            Arc::new(Store::in_memory().expect("opens")),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        );
        Fixture {
            auth: AuthService::new(context),
            secure,
        }
    }

    fn exchange_body() -> serde_json::Value {
        serde_json::json!({
            "sessionToken": "eyJhbGciOi.token",
            "expiresAt": "2026-10-07T12:00:00Z",
            // Development issues the unprefixed name; production issues the `__Secure-` one. The
            // server says which, because a client cannot know before it has seen a cookie.
            "sessionCookieName": "next-auth.session-token",
            "user": { "id": "u1", "email": "ada@example.com", "name": "Ada" }
        })
    }

    /// AATD-3. The hand-off page names the app it is signing in ("Astrid for Android"), and the
    /// server's registry refuses a client it does not know — so the client is the shell's own,
    /// in both halves of the flow, not the "windows" every caller used to send.
    #[tokio::test]
    async fn aatd_3_a_shell_signs_in_as_its_own_client() {
        let transport =
            StubTransport::new().push_json("/api/v1/auth/desktop/exchange", 200, exchange_body());
        let recorded = transport.recorded.clone();
        let fixture = fixture_on(transport, crate::api::Platform::Android);
        let url = fixture.auth.begin().expect("begins");
        assert!(url.contains("client=android"), "{url}");

        let state = state_from(&url);
        fixture
            .auth
            .complete(&format!("astrid://auth/callback?code=abc&state={state}"))
            .await
            .expect("completes");
        let sent = recorded.lock().expect("recorded").clone();
        let body: serde_json::Value =
            serde_json::from_slice(sent.last().expect("sent").body.as_deref().expect("a body"))
                .expect("json");
        assert_eq!(body["client"], "android");
    }

    /// AWTD2-95. A white-label app installed beside Astrid registers its own scheme, so a sign-in
    /// it started comes back on `partner://`, and its server's registry knows it by its own id. An
    /// `astrid://` callback is Astrid's — refused here, whatever state it carries.
    #[tokio::test]
    async fn awtd2_95_a_white_label_shell_signs_in_on_its_own_scheme_and_client() {
        let transport =
            StubTransport::new().push_json("/api/v1/auth/desktop/exchange", 200, exchange_body());
        let recorded = transport.recorded.clone();
        let fixture = fixture_on(transport, crate::api::Platform::Windows);
        let auth = fixture
            .auth
            .with_url_scheme("partner")
            .with_handoff_client(Some("partner-windows"));
        assert_eq!(auth.url_scheme(), "partner");
        assert_eq!(auth.handoff_client(), "partner-windows");

        let url = auth.begin().expect("begins");
        assert!(url.contains("client=partner-windows"), "{url}");
        let error = auth
            .complete(&format!(
                "astrid://auth/callback?code=abc&state={}",
                state_from(&url)
            ))
            .await
            .expect_err("Astrid's scheme is not this app's");
        assert!(error.to_string().contains("scheme"), "{error}");

        let url = auth.begin().expect("begins again");
        auth.complete(&format!(
            "partner://auth/callback?code=abc&state={}",
            state_from(&url)
        ))
        .await
        .expect("completes on its own scheme");
        let sent = recorded.lock().expect("recorded").clone();
        let body: serde_json::Value =
            serde_json::from_slice(sent.last().expect("sent").body.as_deref().expect("a body"))
                .expect("json");
        assert_eq!(body["client"], "partner-windows");
    }

    /// AWTD2-95. Hosted Astrid is unchanged: with nothing configured — or something blank — the
    /// scheme is `astrid` and the client is the platform's own registered id.
    #[test]
    fn awtd2_95_unset_or_blank_identity_keeps_astrids_defaults() {
        let fixture = fixture_on(StubTransport::new(), crate::api::Platform::Android);
        assert_eq!(fixture.auth.url_scheme(), "astrid");
        assert_eq!(fixture.auth.handoff_client(), "android");
        let auth = fixture
            .auth
            .with_url_scheme("  ")
            .with_handoff_client(Some(" "));
        assert_eq!(auth.url_scheme(), URL_SCHEME);
        assert_eq!(auth.handoff_client(), "android");
        let auth = auth.with_url_scheme(" Partner ").with_handoff_client(None);
        assert_eq!(auth.url_scheme(), "Partner");
        assert_eq!(auth.handoff_client(), "android");
    }

    /// AATD-3. The iOS sign-in screen asks which provider first; the page should start it.
    #[test]
    fn aatd_3_a_chosen_provider_rides_on_the_hand_off_url() {
        let fixture = fixture(StubTransport::new());
        let url = fixture.auth.begin_with(Some("google")).expect("begins");
        assert!(url.contains("provider=google"), "{url}");
        assert!(url.contains("state="), "the flow is still bound: {url}");
        let plain = fixture.auth.begin_with(None).expect("begins");
        assert!(!plain.contains("provider="), "{plain}");
        let blank = fixture.auth.begin_with(Some("  ")).expect("begins");
        assert!(!blank.contains("provider="), "{blank}");
    }

    #[test]
    fn aatd_3_every_platform_has_the_client_the_server_registers() {
        use crate::api::Platform;
        assert_eq!(Platform::Windows.handoff_client(), "windows");
        assert_eq!(Platform::Ios.handoff_client(), "ios");
        assert_eq!(Platform::Mac.handoff_client(), "mac");
        assert_eq!(Platform::Android.handoff_client(), "android");
    }

    /// The state in the URL is what the callback is later checked against.
    #[test]
    fn beginning_a_sign_in_produces_a_url_carrying_the_flows_state() {
        let fixture = fixture(StubTransport::new());
        let url = fixture.auth.begin().expect("begins");

        assert!(url.starts_with("https://astrid.cc/auth/desktop"), "{url}");
        assert!(url.contains("code_challenge="), "{url}");
        assert!(url.contains("state="), "{url}");
        assert!(fixture.auth.is_waiting());
    }

    #[tokio::test]
    async fn a_completed_sign_in_stores_the_credential_and_the_user() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/auth/desktop/exchange",
            200,
            exchange_body(),
        ));
        let url = fixture.auth.begin().expect("begins");
        let state = state_from(&url);

        let user = fixture
            .auth
            .complete(&format!("astrid://auth/callback?code=abc&state={state}"))
            .await
            .expect("completes");

        assert_eq!(user.id, "u1");
        assert_eq!(
            fixture.secure.get(SESSION_COOKIE_KEY).await.as_deref(),
            Some("next-auth.session-token=eyJhbGciOi.token"),
            "the whole Cookie header is stored, not the bare token"
        );
        assert!(fixture.auth.is_signed_in().await);
        assert!(!fixture.auth.is_waiting(), "the flow is spent");
    }

    /// Any web page can open `astrid://auth/callback?code=…`. The state check is what makes that
    /// harmless.
    #[tokio::test]
    async fn a_callback_with_the_wrong_state_is_refused() {
        let fixture = fixture(StubTransport::new());
        fixture.auth.begin().expect("begins");

        let error = fixture
            .auth
            .complete("astrid://auth/callback?code=abc&state=not-ours")
            .await
            .expect_err("refused");
        assert!(matches!(
            error,
            crate::services::ServiceError::Api(crate::api::ApiError::Refused(_))
        ));
        assert!(!fixture.auth.is_signed_in().await);
    }

    /// A link sent to an app that was not signing in. There is nothing to check it against, so it
    /// is refused rather than trusted.
    #[tokio::test]
    async fn a_callback_with_no_sign_in_in_progress_is_refused() {
        let fixture = fixture(StubTransport::new());
        assert!(fixture
            .auth
            .complete("astrid://auth/callback?code=abc&state=whatever")
            .await
            .is_err());
    }

    /// A replayed activation — the same URL delivered twice — finds nothing in progress the second
    /// time.
    #[tokio::test]
    async fn a_callback_can_only_be_used_once() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/auth/desktop/exchange",
            200,
            exchange_body(),
        ));
        let url = fixture.auth.begin().expect("begins");
        let callback = format!("astrid://auth/callback?code=abc&state={}", state_from(&url));

        assert!(fixture.auth.complete(&callback).await.is_ok());
        assert!(fixture.auth.complete(&callback).await.is_err());
    }

    /// A deep link on our own scheme that is not a sign-in must not be mistaken for one.
    #[tokio::test]
    async fn another_deep_link_on_the_same_scheme_is_not_a_callback() {
        let fixture = fixture(StubTransport::new());
        let url = fixture.auth.begin().expect("begins");
        let state = state_from(&url);

        assert!(fixture
            .auth
            .complete(&format!("astrid://tasks/t1?state={state}"))
            .await
            .is_err());
    }

    /// The server refusing the exchange leaves the app signed out rather than half signed in.
    #[tokio::test]
    async fn a_refused_exchange_stores_nothing() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/auth/desktop/exchange",
            400,
            serde_json::json!({ "error": "the code has expired" }),
        ));
        let url = fixture.auth.begin().expect("begins");
        let state = state_from(&url);

        assert!(fixture
            .auth
            .complete(&format!("astrid://auth/callback?code=abc&state={state}"))
            .await
            .is_err());
        assert!(!fixture.auth.is_signed_in().await);
    }

    #[test]
    fn cancelling_leaves_nothing_for_a_late_callback_to_complete() {
        let fixture = fixture(StubTransport::new());
        fixture.auth.begin().expect("begins");
        fixture.auth.cancel();
        assert!(!fixture.auth.is_waiting());
    }

    fn state_from(url: &str) -> String {
        url::Url::parse(url)
            .expect("a URL")
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("a state")
    }
}
