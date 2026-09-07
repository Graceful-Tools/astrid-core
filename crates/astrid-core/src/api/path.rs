//! Keeping attacker-influenced identifiers from steering a request.
//!
//! Ported from `astrid-ios/Astrid App/Core/Networking/APIPathSafety.swift`, which came out of the
//! 2026-07-25 audit. The attack it stops is worth restating, because the code looks paranoid until
//! you have seen it:
//!
//! Task and list ids arrive from places the user does not control — above all deep links
//! (`astrid://tasks/<id>`, `https://astrid.cc/tasks/<id>`), which any web page or message can hand
//! the app. Those ids are interpolated straight into request paths (`/api/v1/tasks/{id}`), and
//! nothing in a URL type normalises the result: a percent-encoded slash survives as a real
//! separator, so `astrid://tasks/abc%2F..%2Fadmin` built `/api/v1/tasks/abc/../admin` — which
//! servers and CDNs resolve to `/api/v1/admin`, issued with the signed-in user's session.
//!
//! Two independent guards, so neither is load-bearing alone:
//!
//! 1. [`is_valid_identifier`] — protocol activation and deep links accept only id-shaped strings,
//!    before anything is opened.
//! 2. [`escaped_path_component`] — anything interpolated into a path is percent-encoded, so a
//!    stray separator cannot change a request's shape even if the first guard is bypassed.
//!
//! And [`is_safe_request_path`] is the backstop at the single point where every request URL is
//! built, so no call site can opt out of it.

/// The longest identifier that will be routed on. Bounded so a pathological link cannot drive an
/// enormous URL.
const MAX_IDENTIFIER_LEN: usize = 128;

/// Whether `id` is safe to route on and to place in a request path.
///
/// The allowed set is deliberately narrower than "URL-safe": no `/`, no `.`, no `%`, no
/// whitespace — the ingredients of traversal.
pub fn is_valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_IDENTIFIER_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether a fully built request path is free of traversal segments.
///
/// Checked on the decoded form as well as the raw one, because `%2E%2E%2F` decodes to `../` at the
/// server — a path that looks clean here and traverses there is the whole trick.
pub fn is_safe_request_path(path: &str) -> bool {
    let decoded = percent_decode(path);
    [path, decoded.as_str()].iter().all(|candidate| {
        !candidate
            .split('/')
            .filter(|segment| !segment.is_empty())
            .any(|segment| segment == ".." || segment == ".")
    })
}

/// Percent-encode a value for use as a single path component.
///
/// `/` is not in the unreserved set, so an embedded separator becomes `%2F` and stays inert.
pub fn escaped_path_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Decode enough percent-escapes to see through a disguised traversal. Invalid escapes are left as
/// written rather than dropped — a decoder that silently discarded them could turn an unsafe path
/// into a safe-looking one.
fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_ids_are_routable() {
        assert!(is_valid_identifier("cm3x8k2p40001abcdefgh"));
        assert!(is_valid_identifier("temp_9f2c-41ab"));
        assert!(is_valid_identifier("A1"));
    }

    /// The deep-link half of the guard. Each of these was reachable from a link a stranger could
    /// send.
    #[test]
    fn traversal_ingredients_are_not_identifiers() {
        for id in [
            "", "..", ".", "a/b", "a%2Fb", "a b", "a.b", "../admin", "a\u{0}b",
        ] {
            assert!(!is_valid_identifier(id), "{id:?} should be rejected");
        }
    }

    #[test]
    fn an_absurdly_long_id_is_refused() {
        assert!(is_valid_identifier(&"a".repeat(MAX_IDENTIFIER_LEN)));
        assert!(!is_valid_identifier(&"a".repeat(MAX_IDENTIFIER_LEN + 1)));
    }

    #[test]
    fn ordinary_paths_are_safe() {
        assert!(is_safe_request_path("/api/v1/tasks"));
        assert!(is_safe_request_path("/api/v1/tasks/cm3x8k2p40001"));
        assert!(is_safe_request_path("/api/v1/lists/l1/members/u1"));
    }

    /// The 2026-07-25 finding: `astrid://tasks/abc%2F..%2Fadmin`. It reads as one component here
    /// and as three at the server, which is why the decoded form is checked too.
    #[test]
    fn a_percent_encoded_traversal_is_refused() {
        assert!(!is_safe_request_path("/api/v1/tasks/abc%2F..%2Fadmin"));
        assert!(!is_safe_request_path("/api/v1/tasks/%2E%2E%2Fadmin"));
        assert!(!is_safe_request_path("/api/v1/tasks/../admin"));
        assert!(!is_safe_request_path("/api/v1/tasks/./admin"));
    }

    /// A dot inside a segment is not a traversal. Refusing it would break the filename-shaped
    /// components attachment endpoints carry.
    #[test]
    fn a_dot_inside_a_segment_is_left_alone() {
        assert!(is_safe_request_path("/api/v1/files/plan.v2.pdf"));
    }

    #[test]
    fn escaping_neutralises_an_embedded_separator() {
        assert_eq!(escaped_path_component("a/b"), "a%2Fb");
        assert_eq!(escaped_path_component("../admin"), "..%2Fadmin");
        assert_eq!(escaped_path_component("plain-id_1"), "plain-id_1");
    }

    /// Escaping alone is not enough, which is why there are two guards: the escaped form of a
    /// traversal still contains `..` as a segment once the server decodes it, and
    /// `is_safe_request_path` is what catches that.
    #[test]
    fn escaping_and_the_backstop_cover_each_other() {
        let path = format!("/api/v1/tasks/{}", escaped_path_component("../admin"));
        assert!(!is_safe_request_path(&path));
    }
}
