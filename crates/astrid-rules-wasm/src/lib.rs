//! The web's binding to [`astrid_rules::rules`]: one function, JSON in, JSON out.
//!
//! The same door the Apple apps reach through UniFFI and that `astrid_core::rules` re-exports —
//! so a request answers identically on every client, because it is the same code. The contract is
//! the JSON, which is why nothing richer than a string crosses the boundary: no generated
//! TypeScript types to drift from the Rust ones.
//!
//! Built with `wasm-bindgen` (see `scripts/build-wasm.sh`); astrid-web vendors the output.

use wasm_bindgen::prelude::wasm_bindgen;

/// Answer one rule given as JSON, as JSON — `astrid_rules::rules::run_json`.
///
/// Never throws for a bad request: an unreadable one comes back as
/// `{"ok":false,"error":{"kind":"badRequest",…}}`, the same envelope as every other answer.
#[wasm_bindgen(js_name = runJson)]
pub fn run_json(request: &str) -> String {
    astrid_rules::rules::run_json(request)
}

/// The `kind` of every rule this build answers, sorted — `astrid_rules::rules::kinds`.
///
/// The browser package answers fewer rules than the Node one (docs/BROWSER_DOOR.md), so a caller
/// can choose per rule, with the core or with its own copy, rather than discover a refusal.
#[wasm_bindgen]
pub fn kinds() -> Vec<String> {
    astrid_rules::rules::kinds()
}

/// The crate version this build came from, for a caller's logs.
#[wasm_bindgen(js_name = coreVersion)]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binding_lists_what_the_rules_door_lists() {
        assert_eq!(kinds(), astrid_rules::rules::kinds());
        assert!(kinds().iter().any(|kind| kind == "smartParse"));
    }

    #[test]
    fn the_binding_answers_what_the_rules_door_answers() {
        let request = r#"{"kind":"smartParse","text":"buy milk","today":"2026-03-01"}"#;
        assert_eq!(run_json(request), astrid_rules::rules::run_json(request));
    }

    /// The Node package is built with `markdown` on, and astrid-web's server asks it to render
    /// (AWTD2-162). The browser build refusing is asserted in `astrid-rules/tests/browser_build.rs`
    /// and on the built packages by `scripts/build-wasm.sh`. It cannot be asserted here, because
    /// `cargo test --workspace` unifies `astrid-rules/markdown` back on through astrid-core.
    #[cfg(feature = "markdown")]
    #[test]
    fn the_node_build_renders_markdown() {
        let reply: serde_json::Value =
            serde_json::from_str(&run_json(r#"{"kind":"renderMarkdown","text":"**hi**"}"#))
                .unwrap();
        assert_eq!(
            reply["ok"], true,
            "the Node build must answer renderMarkdown: {reply}"
        );
    }

    /// The Node package is built with `orchestration` on, and astrid-web's server asks it the
    /// orchestration policy wherever an agent acts (AWTD-1685).
    #[cfg(feature = "orchestration")]
    #[test]
    fn the_node_build_answers_the_orchestration_policy_awtd_1685() {
        let reply: serde_json::Value = serde_json::from_str(&run_json(
            r#"{"kind":"orchestrationMay","agent":"claude","action":"merge"}"#,
        ))
        .unwrap();
        assert_eq!(reply["value"]["decision"], "never", "{reply}");
    }

    #[test]
    fn a_bad_request_is_an_envelope_not_a_panic() {
        let reply: serde_json::Value = serde_json::from_str(&run_json("not json")).unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "badRequest");
    }
}
