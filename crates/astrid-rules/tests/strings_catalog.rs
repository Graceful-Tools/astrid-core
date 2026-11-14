//! The string catalog covers this core, and this core's source holds no key it does not know about
//! (task 1bdf84d7).
//!
//! Two questions, and they fail for opposite reasons:
//!
//! 1. **Does `strings/` say every key [`astrid_rules::copy::keys`] names, in every language?** That
//!    is the acceptance line, and `cargo xtask check-strings` asks it too — here as well because
//!    `cargo test --workspace` is the inner loop and a catalog that has fallen behind should fail
//!    in the loop rather than at the gate.
//! 2. **Does `copy::keys()` name every key-shaped literal in this crate's source?** This is the
//!    backstop for the failure nothing else can see: a NEW module that returns a `title_key` and is
//!    never added to `copy.rs`. Its keys would be absent from the catalog, every check above would
//!    pass, and the key itself would reach the screen.
//!
//! An integration test and not a unit one, on purpose. It reads files from the repository, and
//! `astrid-rules` builds for `wasm32-unknown-unknown` where there is no filesystem — compiling the
//! catalog into the library with `include_str!` would put twelve locales into a wasm binary that
//! another task is actively trying to shrink.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The repository root, from this crate's manifest directory.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("astrid-rules lives at <root>/crates/astrid-rules")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

/// Every language `strings/locales.json` names.
fn locales() -> Vec<String> {
    let text = read(&root().join("strings/locales.json"));
    let value: serde_json::Value =
        serde_json::from_str(&text).expect("strings/locales.json is JSON");
    value["locales"]
        .as_array()
        .expect("`locales` is an array")
        .iter()
        .map(|row| {
            row["tag"]
                .as_str()
                .expect("every locale has a tag")
                .to_string()
        })
        .collect()
}

/// **The acceptance line.** Every key this core emits has words in every one of the twelve, and the
/// catalog holds nothing the core has stopped emitting.
#[test]
fn every_key_the_core_emits_is_in_the_catalog_in_every_language_1bdf84d7() {
    let keys = astrid_rules::copy::keys();
    let locales = locales();
    assert_eq!(locales.len(), 12, "the product ships twelve languages");

    let catalog_text = read(&root().join("strings/catalog.json"));
    let catalog: serde_json::Value =
        serde_json::from_str(&catalog_text).expect("strings/catalog.json is JSON");
    let declared: BTreeSet<String> = catalog["keys"]
        .as_object()
        .expect("`keys` is an object")
        .keys()
        .cloned()
        .collect();
    let emitted: BTreeSet<String> = keys.iter().cloned().collect();

    let missing: Vec<&String> = emitted.difference(&declared).collect();
    assert!(
        missing.is_empty(),
        "these keys are emitted by this core and are not in strings/catalog.json — a client asking \
         for one draws the key itself on screen: {missing:?}"
    );
    // The other way is not cosmetic either: a key nobody emits is twelve translations nothing will
    // ever read, and it would sit in the catalog for ever because nothing would ever ask for it.
    let orphaned: Vec<&String> = declared.difference(&emitted).collect();
    assert!(
        orphaned.is_empty(),
        "strings/catalog.json carries keys this core no longer emits: {orphaned:?}"
    );

    for locale in &locales {
        let words_text = read(&root().join(format!("strings/{locale}.json")));
        let words: serde_json::Value =
            serde_json::from_str(&words_text).unwrap_or_else(|err| panic!("{locale}.json: {err}"));
        let words = words
            .as_object()
            .unwrap_or_else(|| panic!("strings/{locale}.json is an object"));
        let absent: Vec<&String> = keys
            .iter()
            .filter(|key| !words.contains_key(*key))
            .collect();
        assert!(
            absent.is_empty(),
            "strings/{locale}.json has no words for {absent:?}"
        );
    }
}

/// Keys this crate's source holds that are deliberately NOT words.
///
/// A dotted lower-case string is not automatically something to translate. Each of these is the
/// name a **preference is stored under** — a wire value, synced between clients, and translating
/// one would change what the setting is called in the database. Written down with that reason
/// rather than re-decided each time somebody reads the scan below.
const NOT_COPY: &[&str] = &[
    // `astrid_rules::theme::KEY`, `keyboard::chord::KEY`, `title_display::KEY` — stored settings.
    "theme.mode",
    "shell.hotkey",
    "rows.titleDisplay",
    // `decisions::RATING_KEY_PREFIX` — the namespace the five rating words hang under, not a key
    // of its own. `copy::keys()` names `tasks.tradeoffRatings.veryGood` and its four siblings.
    "tasks.tradeoffRatings",
    // Fixture data in tests: file names, a host, and two version strings.
    "shot.png",
    "shot.PNG",
    "plan.pdf",
    "notes.pdf",
    "b.pdf",
    "www.example.com",
    "v1.0",
    "v2.1",
];

/// **The backstop.** Nothing in this crate's source looks like a copy key that `copy::keys()` has
/// never heard of.
///
/// A source scan is a blunt instrument and that is the point: it sees a module nobody remembered to
/// add to `copy.rs`, which is the one failure every structured check here is blind to by
/// construction. A false positive costs one line in [`NOT_COPY`] with a reason; a false negative
/// costs a key drawn on screen in a shipped build.
#[test]
fn no_key_shaped_literal_escapes_the_catalog_1bdf84d7() {
    let known: BTreeSet<String> = astrid_rules::copy::keys().into_iter().collect();
    let mut unknown: BTreeSet<String> = BTreeSet::new();

    let mut stack = vec![root().join("crates/astrid-rules/src")];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", directory.display()))
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            for literal in key_shaped_literals(&read(&path)) {
                if !known.contains(&literal) && !NOT_COPY.contains(&literal.as_str()) {
                    unknown.insert(literal);
                }
            }
        }
    }

    assert!(
        unknown.is_empty(),
        "these look like string-catalog keys but copy::keys() does not name them. Either add the \
         module that emits them to crates/astrid-rules/src/copy.rs, or — if they are not words — \
         add them to NOT_COPY in this file with the reason: {unknown:?}"
    );
}

/// Every double-quoted literal in `source` shaped like a copy key: dotted, lower-case-leading, and
/// made only of what a resource name on any of the four platforms can carry.
///
/// Deliberately ignorant of context. It cannot tell a `title_key` from a `match` arm, and it does
/// not need to: anything of this shape is either a key the catalog should hold or a thing somebody
/// has to name in [`NOT_COPY`].
fn key_shaped_literals(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(open) = rest.find('"') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else { break };
        let literal = &rest[..close];
        rest = &rest[close + 1..];
        if is_key_shaped(literal) {
            found.push(literal.to_string());
        }
    }
    found
}

fn is_key_shaped(literal: &str) -> bool {
    let mut segments = literal.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    if !first.starts_with(|c: char| c.is_ascii_lowercase()) {
        return false;
    }
    let mut dotted = false;
    let mut all = vec![first];
    for segment in segments {
        dotted = true;
        all.push(segment);
    }
    dotted
        && all.iter().all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

#[test]
fn the_scan_knows_a_key_from_a_sentence_1bdf84d7() {
    assert!(is_key_shaped("filter.due.today"));
    assert!(is_key_shaped("smart.time.17_00"));
    assert!(is_key_shaped("tasks.tradeoffRatings.veryGood"));
    // Not keys: no dot, a leading capital, an empty segment, a space, punctuation.
    assert!(!is_key_shaped("sort"));
    assert!(!is_key_shaped("My Tasks"));
    assert!(!is_key_shaped("Repeating.daily"));
    assert!(!is_key_shaped("filter..due"));
    assert!(!is_key_shaped("is this a sentence."));
    assert!(!is_key_shaped("a.b-c"));

    assert_eq!(
        key_shaped_literals(r#"let x = "picker.today"; let y = "Not a key";"#),
        vec!["picker.today".to_string()]
    );
}
