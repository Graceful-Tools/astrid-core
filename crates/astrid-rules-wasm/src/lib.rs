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

/// The crate version this build came from, for a caller's logs.
#[wasm_bindgen(js_name = coreVersion)]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binding_answers_what_the_rules_door_answers() {
        let request = r#"{"kind":"renderMarkdown","text":"**hi**"}"#;
        assert_eq!(run_json(request), astrid_rules::rules::run_json(request));
    }

    #[test]
    fn a_bad_request_is_an_envelope_not_a_panic() {
        let reply: serde_json::Value = serde_json::from_str(&run_json("not json")).unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "badRequest");
    }
}
