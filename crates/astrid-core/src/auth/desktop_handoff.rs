//! Desktop browser hand-off sign-in — the client half.
//!
//! The server half is `astrid-web/lib/auth/desktop-handoff.ts` plus the two
//! routes it serves; this module is what the Windows app runs on either side of
//! the browser trip:
//!
//! 1. generate a PKCE verifier and keep it in memory,
//! 2. open the system browser at [`authorize_url`],
//! 3. receive `astrid://auth/callback?code=…&state=…` through protocol
//!    activation and check it with [`parse_callback`],
//! 4. POST the code and the verifier to `/api/v1/auth/desktop/exchange`.
//!
//! **Any local program can register the same URL scheme**, so step 3 is not a
//! private channel. Two things follow, and they are the reason this module
//! exists rather than a few inline string operations in the shell:
//!
//! - The verifier never leaves the process. Only its SHA-256 is sent in step 2,
//!   so a code stolen at step 3 cannot be redeemed.
//! - The `state` is compared before the code is used. A callback that arrives
//!   without a flow having started — or from a different flow — is another
//!   program talking to us, and is dropped.
//!
//! Nothing here talks to the network; the API call belongs to `api::client`.
//! That keeps every decision in this file testable without a server.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

/// The only PKCE method the server accepts. `plain` binds nothing.
pub const CODE_CHALLENGE_METHOD: &str = "S256";

/// Which app this is, in the server's client registry.
pub const CLIENT_ID: &str = "windows";

/// Path the browser opens to start the hand-off.
pub const AUTHORIZE_PATH: &str = "/auth/desktop";

/// Host and path of the callback, after the app's URL scheme.
pub const CALLBACK_HOST: &str = "auth";
pub const CALLBACK_PATH: &str = "/callback";

/// Bytes of entropy behind the verifier and the state.
///
/// 32 bytes is 256 bits, and base64url of 32 bytes is 43 characters — exactly
/// the minimum RFC 7636 §4.1 allows for a verifier, and the length an S256
/// challenge always has.
const ENTROPY_BYTES: usize = 32;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HandoffError {
    #[error("callback URL could not be parsed")]
    MalformedCallback,
    /// The callback arrived on a scheme this app did not register — someone
    /// else's deep link, or an attempt to feed us one.
    #[error("callback was not on the expected URL scheme")]
    WrongScheme,
    #[error("callback was not the sign-in callback")]
    WrongCallback,
    #[error("callback carried no code")]
    MissingCode,
    /// The decisive check. A callback whose state does not match the flow this
    /// process started is not ours, whoever sent it.
    #[error("callback state did not match the flow that was started")]
    StateMismatch,
    #[error("could not read from the system random number generator")]
    Entropy,
}

/// A PKCE pair. The verifier is a secret and must never be logged or persisted;
/// it lives only until the exchange completes.
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

/// Everything one hand-off attempt needs to remember while the browser is open.
#[derive(Debug, Clone)]
pub struct HandoffFlow {
    pub state: String,
    pub pkce: PkcePair,
}

fn random_token() -> Result<String, HandoffError> {
    let mut bytes = [0u8; ENTROPY_BYTES];
    getrandom::getrandom(&mut bytes).map_err(|_| HandoffError::Entropy)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 of the verifier, base64url without padding — RFC 7636 §4.2.
///
/// The hash is over the ASCII bytes of the verifier itself, not over decoded
/// entropy. Hashing the decoded bytes instead is the classic PKCE mistake: it
/// produces a challenge the server will never reproduce, and the failure only
/// shows up against a real server.
pub fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn generate_pkce() -> Result<PkcePair, HandoffError> {
    let verifier = random_token()?;
    let challenge = challenge_for(&verifier);
    Ok(PkcePair {
        verifier,
        challenge,
    })
}

/// Start a flow: fresh state, fresh PKCE pair.
pub fn begin_flow() -> Result<HandoffFlow, HandoffError> {
    Ok(HandoffFlow {
        state: random_token()?,
        pkce: generate_pkce()?,
    })
}

/// The URL to open in the system browser.
///
/// `base_url` is the server origin, so a Debug build pointed at
/// `http://localhost:3000` hands off to the local dev server exactly as a
/// release build hands off to production.
pub fn authorize_url(base_url: &str, flow: &HandoffFlow) -> Result<String, HandoffError> {
    let mut url = Url::parse(base_url).map_err(|_| HandoffError::MalformedCallback)?;

    // set_path rather than string concatenation, so a base URL with or without a
    // trailing slash — or with a path of its own — produces the same result.
    url.set_path(AUTHORIZE_PATH);
    url.query_pairs_mut()
        .clear()
        .append_pair("client", CLIENT_ID)
        .append_pair("state", &flow.state)
        .append_pair("code_challenge", &flow.pkce.challenge)
        .append_pair("code_challenge_method", CODE_CHALLENGE_METHOD);

    Ok(url.into())
}

/// The code carried back from the browser, once it has been proven to belong to
/// `flow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callback {
    pub code: String,
}

/// Validate a protocol activation and pull the code out of it.
///
/// `scheme` is the app's registered URL scheme without `://`. It is passed in
/// rather than hardcoded so a fork — which registers its own scheme, the way
/// `BRAND.appUrlScheme` allows on the server — needs no change here.
///
/// The state comparison is what makes this safe to call on any activation the
/// OS delivers: an activation that does not match the flow in progress is
/// rejected before its code is looked at.
pub fn parse_callback(
    raw: &str,
    flow: &HandoffFlow,
    scheme: &str,
) -> Result<Callback, HandoffError> {
    let url = Url::parse(raw).map_err(|_| HandoffError::MalformedCallback)?;

    if !url.scheme().eq_ignore_ascii_case(scheme) {
        return Err(HandoffError::WrongScheme);
    }

    // `astrid://auth/callback` parses as host `auth`, path `/callback`. Both are
    // checked so a different deep link on our own scheme — `astrid://task/123`,
    // say — cannot be mistaken for a sign-in.
    let host_matches = url
        .host_str()
        .is_some_and(|h| h.eq_ignore_ascii_case(CALLBACK_HOST));
    if !host_matches || url.path() != CALLBACK_PATH {
        return Err(HandoffError::WrongCallback);
    }

    let mut code = None;
    let mut state = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            _ => {}
        }
    }

    // Checked BEFORE the code is accepted, and checked even when absent: a
    // callback with no state at all is not one of ours either.
    if state.as_deref() != Some(flow.state.as_str()) {
        return Err(HandoffError::StateMismatch);
    }

    match code {
        Some(code) if !code.is_empty() => Ok(Callback { code }),
        _ => Err(HandoffError::MissingCode),
    }
}

/// Body of `POST /api/v1/auth/desktop/exchange`.
///
/// camelCase because that is what the route reads. The verifier appears here
/// and nowhere else in the crate.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeRequest<'a> {
    pub client: &'a str,
    pub code: &'a str,
    pub code_verifier: &'a str,
}

impl<'a> ExchangeRequest<'a> {
    pub fn new(callback: &'a Callback, flow: &'a HandoffFlow) -> Self {
        Self {
            client: CLIENT_ID,
            code: &callback.code,
            code_verifier: &flow.pkce.verifier,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeUser {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub image: Option<String>,
}

/// Response of the exchange.
///
/// `session_cookie_name` is not decoration. Secure storage holds a whole
/// `Cookie` header (see [`super::session_cookie`]), and production issues
/// `__Secure-next-auth.session-token` while development issues
/// `next-auth.session-token`. Guessing means signing in successfully and then
/// being treated as signed out on the very next request.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeResponse {
    pub session_token: String,
    pub expires_at: String,
    pub session_cookie_name: String,
    pub user: ExchangeUser,
}

impl ExchangeResponse {
    /// The `Cookie` header to store, built from the name the server named.
    pub fn cookie_header(&self) -> String {
        super::session_cookie::replacing_token_named(
            None,
            &self.session_cookie_name,
            &self.session_token,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEME: &str = "astrid";

    fn flow_with_state(state: &str) -> HandoffFlow {
        HandoffFlow {
            state: state.to_string(),
            pkce: PkcePair {
                verifier: "v".repeat(64),
                challenge: challenge_for(&"v".repeat(64)),
            },
        }
    }

    /// RFC 7636 Appendix B. Locking the published vector means a refactor that
    /// hashes the wrong thing fails here rather than against a live server.
    #[test]
    fn challenge_matches_the_rfc_test_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_verifiers_are_within_the_rfc_bounds_and_base64url() {
        let pair = generate_pkce().expect("entropy");
        assert!((43..=128).contains(&pair.verifier.len()));
        assert!(pair
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(pair.challenge, challenge_for(&pair.verifier));
    }

    #[test]
    fn every_flow_gets_its_own_secrets() {
        let a = begin_flow().expect("entropy");
        let b = begin_flow().expect("entropy");
        assert_ne!(a.state, b.state);
        assert_ne!(a.pkce.verifier, b.pkce.verifier);
    }

    #[test]
    fn authorize_url_carries_what_the_server_reads() {
        let flow = flow_with_state("state-123");
        let url = Url::parse(&authorize_url("https://astrid.cc", &flow).unwrap()).unwrap();

        assert_eq!(url.path(), AUTHORIZE_PATH);
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(pairs.contains(&("client".into(), CLIENT_ID.into())));
        assert!(pairs.contains(&("state".into(), "state-123".into())));
        assert!(pairs.contains(&("code_challenge".into(), flow.pkce.challenge.clone())));
        assert!(pairs.contains(&("code_challenge_method".into(), CODE_CHALLENGE_METHOD.into())));
    }

    #[test]
    fn authorize_url_honours_a_local_dev_server() {
        // The Debug build points at localhost; the flow must be identical.
        let flow = flow_with_state("s");
        let url = authorize_url("http://localhost:3000", &flow).unwrap();
        assert!(url.starts_with("http://localhost:3000/auth/desktop?"));
    }

    #[test]
    fn authorize_url_never_sends_the_verifier() {
        let flow = flow_with_state("s");
        let url = authorize_url("https://astrid.cc", &flow).unwrap();
        assert!(!url.contains(&flow.pkce.verifier));
    }

    #[test]
    fn callback_yields_the_code_when_the_state_matches() {
        let flow = flow_with_state("state-123");
        let cb = parse_callback(
            "astrid://auth/callback?code=abc123&state=state-123",
            &flow,
            SCHEME,
        )
        .unwrap();
        assert_eq!(cb.code, "abc123");
    }

    #[test]
    fn callback_from_another_flow_is_refused() {
        // The decisive check: any local program can send us an activation.
        let flow = flow_with_state("state-123");
        assert_eq!(
            parse_callback(
                "astrid://auth/callback?code=abc123&state=someone-elses",
                &flow,
                SCHEME
            ),
            Err(HandoffError::StateMismatch)
        );
    }

    #[test]
    fn callback_without_a_state_is_refused() {
        let flow = flow_with_state("state-123");
        assert_eq!(
            parse_callback("astrid://auth/callback?code=abc123", &flow, SCHEME),
            Err(HandoffError::StateMismatch)
        );
    }

    #[test]
    fn callback_on_another_scheme_is_refused() {
        let flow = flow_with_state("state-123");
        assert_eq!(
            parse_callback(
                "notastrid://auth/callback?code=abc&state=state-123",
                &flow,
                SCHEME
            ),
            Err(HandoffError::WrongScheme)
        );
    }

    #[test]
    fn another_deep_link_on_our_own_scheme_is_not_a_sign_in() {
        // astrid://task/<id> is a real activation this app receives; it must not
        // be read as a callback just because it is ours.
        let flow = flow_with_state("state-123");
        assert_eq!(
            parse_callback("astrid://task/123?state=state-123", &flow, SCHEME),
            Err(HandoffError::WrongCallback)
        );
    }

    #[test]
    fn callback_without_a_code_is_refused_even_when_the_state_matches() {
        let flow = flow_with_state("state-123");
        assert_eq!(
            parse_callback("astrid://auth/callback?state=state-123", &flow, SCHEME),
            Err(HandoffError::MissingCode)
        );
        assert_eq!(
            parse_callback(
                "astrid://auth/callback?code=&state=state-123",
                &flow,
                SCHEME
            ),
            Err(HandoffError::MissingCode)
        );
    }

    #[test]
    fn a_state_that_would_inject_a_second_code_round_trips_intact() {
        // Pairs with the server test that builds this URL: the server
        // percent-encodes the state, so the real code must still win here.
        let flow = flow_with_state("x&code=attacker-code&y= #frag");
        let mut url = Url::parse("astrid://auth/callback").unwrap();
        url.query_pairs_mut()
            .append_pair("code", "real-code")
            .append_pair("state", &flow.state);

        let cb = parse_callback(url.as_str(), &flow, SCHEME).unwrap();
        assert_eq!(cb.code, "real-code");
    }

    #[test]
    fn the_scheme_comparison_is_case_insensitive() {
        // Windows hands protocol activations back in whatever case the caller
        // used; URL schemes are case-insensitive by RFC 3986.
        let flow = flow_with_state("state-123");
        assert!(parse_callback(
            "ASTRID://auth/callback?code=abc&state=state-123",
            &flow,
            SCHEME
        )
        .is_ok());
    }

    #[test]
    fn exchange_request_uses_the_keys_the_route_reads() {
        let flow = flow_with_state("s");
        let callback = Callback {
            code: "the-code".into(),
        };
        let json = serde_json::to_value(ExchangeRequest::new(&callback, &flow)).unwrap();

        assert_eq!(json["client"], CLIENT_ID);
        assert_eq!(json["code"], "the-code");
        assert_eq!(json["codeVerifier"], flow.pkce.verifier);
    }

    #[test]
    fn exchange_response_decodes_what_the_route_returns() {
        // Mirrors V1DesktopExchangeResponse in
        // astrid-web/lib/api-contracts/v1-ios-shapes.ts.
        let body = r#"{
            "sessionToken": "jwt.value.here",
            "expiresAt": "2026-10-07T12:00:00.000Z",
            "sessionCookieName": "__Secure-next-auth.session-token",
            "user": { "id": "u1", "email": "a@example.test", "name": "A", "image": null },
            "meta": { "apiVersion": "v1", "authSource": "desktop-handoff" }
        }"#;

        let parsed: ExchangeResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.session_token, "jwt.value.here");
        assert_eq!(
            parsed.session_cookie_name,
            "__Secure-next-auth.session-token"
        );
        assert_eq!(parsed.user.id, "u1");
        assert_eq!(parsed.user.image, None);
    }

    #[test]
    fn the_stored_cookie_uses_the_name_the_server_named() {
        // The whole point of the field: a dev server issues the unprefixed name,
        // and storing the production one would sign the user straight back out.
        let body = r#"{
            "sessionToken": "tok",
            "expiresAt": "2026-10-07T12:00:00.000Z",
            "sessionCookieName": "next-auth.session-token",
            "user": { "id": "u1", "email": null, "name": null, "image": null }
        }"#;
        let parsed: ExchangeResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.cookie_header(), "next-auth.session-token=tok");
    }
}
