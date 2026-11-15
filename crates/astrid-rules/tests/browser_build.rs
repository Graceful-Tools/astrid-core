//! What the browser build is, and is not — task `4d05306d`.
//!
//! `astrid-rules-wasm` takes this crate with `default-features = false`, which drops the markdown
//! renderer: the browser already renders markdown, so `pulldown-cmark` is bytes it downloads in
//! order never to call them. The size of that download is a product requirement, measured and
//! budgeted by CI's `wasm` job (`scripts/wasm-size.mjs`, README → *The web binding's size is a
//! budget*).
//!
//! A feature that removes code is only as good as the line it draws, so both sides are asserted
//! here rather than only the saving:
//!
//! - **Off, `renderMarkdown` is refused** rather than answered emptily. A build that cannot do
//!   something has to say so; an `ok` with no blocks in it is the failure mode worth a test.
//! - **Off, `commentVisibility` still answers.** It lives in `rows::comment` beside the row
//!   projection that *does* need a renderer, and the easy mistake is to gate the module whole and
//!   take the predicates with it. That would pass any test that only checked the first bullet.
//! - **On, `renderMarkdown` answers as it always did** — the default build is every shell's build,
//!   and this change is meant to be invisible to them.
//!
//! Run both ways:
//!
//! ```text
//! cargo test -p astrid-rules --test browser_build
//! cargo test -p astrid-rules --test browser_build --no-default-features
//! ```

use astrid_rules::rules::run_json;
use serde_json::{json, Value};

fn answer(request: Value) -> Value {
    serde_json::from_str(&run_json(&request.to_string())).expect("the door answers JSON")
}

/// The comment one, asked identically in both builds below.
fn comment_visibility() -> Value {
    answer(json!({
        "kind": "commentVisibility",
        "comment": { "id": "c1", "content": "**look**" },
        "offline": false
    }))
}

#[cfg(not(feature = "markdown"))]
mod without_the_renderer {
    use super::*;

    /// Task `4d05306d`: the browser build has no markdown renderer, so it refuses the rule that
    /// needs one. `Rule` is internally tagged, so the variant's absence is a decode failure — the
    /// same answer the door gives any rule it has no variant for.
    #[test]
    fn render_markdown_is_refused_rather_than_answered_emptily() {
        let reply = answer(json!({ "kind": "renderMarkdown", "text": "**hi**" }));
        assert_eq!(
            reply["ok"], json!(false),
            "the browser build answered renderMarkdown; the renderer is supposed to be gone: {reply}"
        );
        assert!(
            reply["error"].to_string().contains("renderMarkdown"),
            "the refusal has to name the rule it cannot do, or a caller cannot tell it from a typo: {reply}"
        );
    }

    /// The line the feature draws. `shows_text`, `is_empty`, `files_of` and `renders_inline` are
    /// `rows::comment`'s too, and none of them parses anything — gating the module whole would
    /// take a working browser rule out with the renderer and no other test would notice.
    #[test]
    fn comment_visibility_still_answers_without_the_renderer() {
        let reply = comment_visibility();
        assert_eq!(
            reply["ok"],
            json!(true),
            "the browser lost commentVisibility along with the renderer: {reply}"
        );
        assert_eq!(reply["value"]["showsText"], json!(true));
    }
}

#[cfg(feature = "markdown")]
mod with_the_renderer {
    use super::*;

    /// The default build is every shell's build: this change is additive for them.
    #[test]
    fn render_markdown_answers_in_the_default_build() {
        let reply = answer(json!({ "kind": "renderMarkdown", "text": "**hi**" }));
        assert_eq!(reply["ok"], json!(true), "{reply}");
        assert!(
            !reply["value"].as_array().expect("blocks").is_empty(),
            "the default build rendered nothing: {reply}"
        );
    }

    #[test]
    fn comment_visibility_answers_the_same_in_both_builds() {
        let reply = comment_visibility();
        assert_eq!(reply["ok"], json!(true), "{reply}");
        assert_eq!(reply["value"]["showsText"], json!(true));
    }
}
