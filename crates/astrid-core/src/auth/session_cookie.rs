//! The stored session credential, and how a renewed token folds into it.
//!
//! Ported from `astrid-ios/Astrid App/Core/Authentication/SessionCookie.swift` (task b8999ea3),
//! including its tests. The behaviour is subtle enough that restating it is worthwhile:
//!
//! Secure storage does not hold a bare token. It holds a whole `Cookie` request header —
//! `name=value; name2=value2` — which is set verbatim on every request. The server, meanwhile,
//! returns a bare JWT in the `mobile-session` body when it renews.
//!
//! Writing that token straight to storage would produce `Cookie: eyJhbGciOi…` with no cookie name,
//! the server would find no session token, and the user would be signed out on the very launch that
//! was meant to keep them signed in — looking exactly like the bug it was fixing.

/// Cookie names the server may have issued the session under.
///
/// Production uses the `__Secure-` prefix; development does not. The server accepts either, so the
/// rule is to keep whichever name is already stored rather than to impose one.
pub const SESSION_COOKIE_NAMES: [&str; 2] = [
    "__Secure-next-auth.session-token",
    "next-auth.session-token",
];

/// The name used when there is no stored cookie to learn one from.
pub const DEFAULT_SESSION_COOKIE_NAME: &str = "next-auth.session-token";

/// Swap the session token's VALUE inside a stored `Cookie` header, keeping its name and every
/// other cookie beside it.
///
/// Other cookies matter: the CSRF one travels here too, and dropping it breaks the next write
/// rather than the next read — a far more confusing failure than an outright sign-out.
pub fn replacing_token(stored: Option<&str>, token: &str) -> String {
    replacing_token_named(stored, DEFAULT_SESSION_COOKIE_NAME, token)
}

/// As [`replacing_token`], but naming the cookie to use when there is nothing stored yet.
///
/// First sign-in has no stored header to learn the name from, and the two names are not
/// interchangeable: production issues the `__Secure-` prefixed one and development does not.
/// The exchange response states which, so the very first authenticated request carries a name
/// the server will actually look for. Every later renewal keeps whatever is already stored,
/// which is why the name is only consulted when nothing matches.
pub fn replacing_token_named(stored: Option<&str>, name: &str, token: &str) -> String {
    let pairs: Vec<&str> = stored
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .collect();

    if pairs.is_empty() {
        return format!("{name}={token}");
    }

    let mut replaced = false;
    let mut out: Vec<String> = Vec::with_capacity(pairs.len() + 1);
    for pair in pairs {
        // Split on the FIRST `=` only: base64url padding can put `=` inside the value, and
        // splitting on every one would silently truncate the token.
        let pair_name = pair.split('=').next().unwrap_or(pair);
        if SESSION_COOKIE_NAMES.contains(&pair_name) {
            out.push(format!("{pair_name}={token}"));
            replaced = true;
        } else {
            out.push(pair.to_string());
        }
    }

    if !replaced {
        out.push(format!("{name}={token}"));
    }
    out.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEW_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.new";

    /// Production uses the `__Secure-` prefixed name. Keep whichever name is already there.
    #[test]
    fn replaces_the_value_and_keeps_the_secure_name() {
        assert_eq!(
            replacing_token(Some("__Secure-next-auth.session-token=OLD"), NEW_TOKEN),
            format!("__Secure-next-auth.session-token={NEW_TOKEN}")
        );
    }

    #[test]
    fn keeps_the_plain_name_when_that_is_what_is_stored() {
        assert_eq!(
            replacing_token(Some("next-auth.session-token=OLD"), NEW_TOKEN),
            format!("next-auth.session-token={NEW_TOKEN}")
        );
    }

    /// Other cookies travel with it — the CSRF cookie in particular. Dropping them would break the
    /// next write rather than the next read, which is a far more confusing failure.
    #[test]
    fn other_cookies_survive_in_order() {
        assert_eq!(
            replacing_token(
                Some("next-auth.csrf-token=abc; __Secure-next-auth.session-token=OLD; other=z"),
                NEW_TOKEN
            ),
            format!(
                "next-auth.csrf-token=abc; __Secure-next-auth.session-token={NEW_TOKEN}; other=z"
            )
        );
    }

    /// A JWT is dot-separated, but base64url padding can carry `=`. Splitting on every `=` instead
    /// of the first would truncate the token silently.
    #[test]
    fn a_token_containing_equals_is_not_truncated() {
        let padded = "eyJhbGciOiJIUzI1NiJ9.payload==";
        assert_eq!(
            replacing_token(Some("next-auth.session-token=OLD"), padded),
            format!("next-auth.session-token={padded}")
        );
    }

    #[test]
    fn whitespace_after_separators_is_tolerated() {
        let result = replacing_token(Some("a=1;   next-auth.session-token=OLD ;b=2"), NEW_TOKEN);
        assert!(result.contains(&format!("next-auth.session-token={NEW_TOKEN}")));
        assert!(result.contains("a=1"));
        assert!(result.contains("b=2"));
    }

    /// Nothing stored yet: still produce a usable header rather than a bare token. The server
    /// accepts either name, so the unprefixed one is the safe default.
    #[test]
    fn with_nothing_stored_it_still_produces_a_named_cookie() {
        for stored in [None, Some(""), Some("   ")] {
            assert_eq!(
                replacing_token(stored, NEW_TOKEN),
                format!("next-auth.session-token={NEW_TOKEN}"),
                "a bare token would be sent as a nameless Cookie header (stored={stored:?})"
            );
        }
    }

    /// Cookies stored but no session token among them — add one, keep the rest.
    #[test]
    fn a_session_cookie_is_added_when_absent() {
        let result = replacing_token(Some("next-auth.csrf-token=abc"), NEW_TOKEN);
        assert!(result.contains("next-auth.csrf-token=abc"));
        assert!(result.contains(&format!("next-auth.session-token={NEW_TOKEN}")));
    }

    /// The result must never be just the token — that is the whole failure this guards.
    #[test]
    fn the_result_is_never_a_bare_token() {
        for stored in [
            None,
            Some(""),
            Some("a=1"),
            Some("next-auth.session-token=OLD"),
        ] {
            let result = replacing_token(stored, NEW_TOKEN);
            assert_ne!(result, NEW_TOKEN);
            assert!(
                result.contains("session-token="),
                "every result must name the session cookie (stored={stored:?})"
            );
        }
    }
}
