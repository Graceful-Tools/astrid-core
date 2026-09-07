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
pub const URL_SCHEME: &str = "astrid";

pub struct AuthService {
    context: Context,
    /// The flow between opening the browser and the callback arriving.
    ///
    /// One at a time on purpose: starting a second sign-in replaces the first, so a callback for
    /// an abandoned attempt is refused by the state check rather than quietly completing.
    flow: Mutex<Option<HandoffFlow>>,
}

impl AuthService {
    pub fn new(context: Context) -> Self {
        AuthService {
            context,
            flow: Mutex::new(None),
        }
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
        let flow = desktop_handoff::begin_flow()
            .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;
        let url = desktop_handoff::authorize_url(self.context.client.base_url(), &flow)
            .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;
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

        let callback = desktop_handoff::parse_callback(callback_url, &flow, URL_SCHEME)
            .map_err(|error| crate::api::ApiError::Refused(error.to_string()))?;

        let request = ExchangeRequest::new(&callback, &flow);
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
        let secure = Arc::new(MemorySecureStore::new());
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                Arc::new(transport),
                secure.clone(),
            )),
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
