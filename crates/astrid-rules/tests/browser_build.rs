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
#[cfg(feature = "rules-shell")]
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
    #[cfg(feature = "rules-shell")]
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

    #[cfg(feature = "rules-shell")]
    #[test]
    fn comment_visibility_answers_the_same_in_both_builds() {
        let reply = comment_visibility();
        assert_eq!(reply["ok"], json!(true), "{reply}");
        assert_eq!(reply["value"]["showsText"], json!(true));
    }
}

/// AWTD-1685: the orchestration policy is the server's question, never the browser's, so the
/// browser build refuses it by name; every other build answers it.
#[test]
fn orchestration_is_answered_only_where_the_feature_is_on_awtd_1685() {
    let reply = answer(json!({ "kind": "orchestrationMay", "agent": "claude", "action": "merge" }));
    if cfg!(feature = "orchestration") {
        assert_eq!(reply["ok"], json!(true), "{reply}");
    } else {
        assert_eq!(reply["ok"], json!(false), "{reply}");
        assert!(
            reply["error"].to_string().contains("orchestrationMay"),
            "{reply}"
        );
    }
}

/// ADEV-273: evaluating an estimate is the server's question, so the browser build refuses
/// `tradeoffEvaluate` by name — but the `policy` sort is a comparison and answers in every build.
#[test]
fn tradeoff_is_answered_only_where_the_feature_is_on_and_the_policy_sort_everywhere_adev_273() {
    let reply = answer(json!({ "kind": "tradeoffEvaluate", "estimate": null }));
    let kinds = astrid_rules::rules::kinds();
    assert_eq!(
        kinds.iter().any(|k| k == "tradeoffEvaluate"),
        cfg!(feature = "tradeoff")
    );
    if cfg!(feature = "tradeoff") {
        assert_eq!(reply["ok"], json!(true), "{reply}");
    } else {
        assert_eq!(reply["ok"], json!(false), "{reply}");
        assert!(
            reply["error"].to_string().contains("tradeoffEvaluate"),
            "{reply}"
        );
    }

    let sorted = answer(json!({
        "kind": "rows", "now": "2026-11-25T12:00:00.000Z", "idsOnly": true,
        "list": { "id": "l", "sortBy": "policy", "filterCompletion": "all" },
        "tasks": [
            { "id": "b", "title": "b" },
            { "id": "a", "title": "a", "estimate": { "label": "do-now", "sortKey": [0] } }
        ]
    }));
    assert_eq!(sorted["ok"], json!(true), "{sorted}");
    assert_eq!(sorted["value"]["ids"], json!(["a", "b"]), "{sorted}");
}

/// AWTD2-162, step 2: one rule from each group, and the feature that owns it. In a build with the
/// group on the rule answers; in a build without, it is refused BY NAME (the same `unknown variant`
/// a client sees for a kind its pinned core predates) and `kinds()` does not list it. Asserting both
/// halves is what keeps a group from being gated out of the enum but left in the vocabulary, or the
/// other way round.
#[test]
fn each_rule_group_answers_only_where_its_feature_is_on_awtd2_162() {
    let probes: [(&str, bool, Value); 4] = [
        (
            "completion",
            cfg!(feature = "rules-repeating"),
            json!({
                "kind": "completion", "task": { "id": "t1", "title": "x" },
                "completed": true, "now": "2026-03-01T12:00:00.000Z", "timeZone": "UTC"
            }),
        ),
        (
            "waitingHandoff",
            cfg!(feature = "rules-waiting"),
            json!({
                "kind": "waitingHandoff", "source": { "id": "t1", "title": "x" },
                "now": "2026-03-01T12:00:00.000Z"
            }),
        ),
        (
            "ratingKey",
            cfg!(feature = "rules-decisions"),
            json!({ "kind": "ratingKey" }),
        ),
        (
            "pluralCategory",
            cfg!(feature = "rules-shell"),
            json!({ "kind": "pluralCategory", "locale": "en", "form": "cardinal", "count": 1 }),
        ),
    ];
    let kinds = astrid_rules::rules::kinds();
    for (kind, on, request) in probes {
        let reply = answer(request);
        assert_eq!(kinds.iter().any(|k| k == kind), on, "kinds() and {kind}");
        if on {
            assert_eq!(reply["ok"], json!(true), "{kind} should answer: {reply}");
        } else {
            assert_eq!(
                reply["ok"],
                json!(false),
                "{kind} should be refused: {reply}"
            );
            assert!(
                reply["error"].to_string().contains(kind),
                "the refusal has to name {kind}: {reply}"
            );
        }
    }
}

/// AWTD2-162: what the browser build answers is the rules web's client-side code calls
/// (docs/BROWSER_DOOR.md §4), and `kinds()` is how web reads that list.
#[cfg(not(any(
    feature = "markdown",
    feature = "orchestration",
    feature = "rules-repeating",
    feature = "rules-waiting",
    feature = "rules-decisions",
    feature = "rules-shell"
)))]
#[test]
fn the_browser_build_answers_exactly_the_decided_rule_set_awtd2_162() {
    let mut expected = vec![
        "board",
        "boardMove",
        "boardPresets",
        "boardRow",
        "boardRows",
        "identifierFormat",
        "identifierLinks",
        "identifierParse",
        "listAccess",
        "manualOrder",
        "palette",
        "reminderSnooze",
        "rows",
        "searchParse",
        "smartParse",
        "theme",
    ];
    expected.sort();
    assert_eq!(astrid_rules::rules::kinds(), expected);
}

/// The default build answers every rule it ever did: the groups are additive for every shell.
#[cfg(all(
    feature = "markdown",
    feature = "orchestration",
    feature = "rules-repeating",
    feature = "rules-waiting",
    feature = "rules-decisions",
    feature = "rules-shell"
))]
#[test]
fn the_full_build_lists_every_rule_awtd2_162() {
    let kinds = astrid_rules::rules::kinds();
    for kind in [
        "completion",
        "nextOccurrence",
        "renderMarkdown",
        "orchestrationMay",
        "waitingWrite",
        "decision",
        "sidebar",
        "smartParse",
        "rows",
    ] {
        assert!(
            kinds.iter().any(|k| k == kind),
            "{kind} missing from {kinds:?}"
        );
    }
    assert!(
        kinds.windows(2).all(|w| w[0] < w[1]),
        "sorted, no duplicates"
    );
}
