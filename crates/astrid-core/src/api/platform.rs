//! Which app this build is, stated on every request.
//!
//! Ported from `astrid-ios/Astrid App/Core/Networking/AnalyticsPlatformHeader.swift` (task
//! 119cabc3 / AITD-301), and it exists for the same reason.
//!
//! The Apple apps used to send nothing that identified them, so the server fell back to sniffing
//! the user agent URLSession composed for it — and the pattern it matched was not the one being
//! sent, so iOS traffic landed in UNKNOWN and Mac had no bucket at all. Stating the platform
//! outright removes the guessing: the server checks this header FIRST, before any user-agent
//! matching, so it wins regardless of what the HTTP stack puts in the agent string.
//!
//! A Windows client has no user-agent alternative worth wanting. Its agent is a Windows HTTP stack
//! string with nothing Astrid about it, and the browser agents it would have to be told apart from
//! all say Mozilla. The header is the whole identification.
//!
//! **Cross-repo contract.** The value is matched by equality in `astrid-web`'s
//! `lib/analytics-events.ts` (`AnalyticsPlatform.WINDOWS_APP`). A typo here is indistinguishable
//! from sending nothing at all — the dashboard simply keeps under-counting — which is why the exact
//! string is pinned by a test rather than trusted to review.

/// The header name. Lower-case because that is how it is written on the other clients and in the
/// server's lookup; HTTP header names are case-insensitive, but matching the others makes a grep
/// across the three repos find all of them.
pub const HEADER_NAME: &str = "x-platform";

/// What this build claims to be. Matches `AnalyticsPlatform.WINDOWS_APP` on the server.
pub const PLATFORM: &str = "windows-app";

/// Astrid-bound requests only.
///
/// Blob uploads and third-party token endpoints are not ours to label; our analytics header is not
/// theirs to receive. The API client applies this to every request it builds, which is the reason
/// it is the only place allowed to speak HTTP — the Apple apps set the header correctly in most
/// places and still left the real-time stream, the passkey calls, attachments and the OAuth token
/// identifying nothing, because each of those built its own request.
pub fn header() -> (&'static str, &'static str) {
    (HEADER_NAME, PLATFORM)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server matches this by equality. Pinned as a literal so a rename has to be deliberate
    /// and cross-repo, rather than quietly turning every Windows request into UNKNOWN.
    #[test]
    fn the_platform_string_is_the_one_the_server_matches() {
        assert_eq!(PLATFORM, "windows-app");
        assert_eq!(HEADER_NAME, "x-platform");
    }

    /// Not `ios-app` or `mac-app`: this build is neither, and claiming to be one would mix the
    /// platforms together in a way no dashboard could unpick afterwards.
    #[test]
    fn it_does_not_claim_to_be_another_client() {
        assert_ne!(PLATFORM, "ios-app");
        assert_ne!(PLATFORM, "mac-app");
    }

    #[test]
    fn the_header_pair_is_what_a_request_carries() {
        assert_eq!(header(), ("x-platform", "windows-app"));
    }
}
