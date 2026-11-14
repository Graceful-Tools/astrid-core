//! The wire schema, written out and gated (task 9ef7edb4).
//!
//! `gen-schema` writes `generated/schema/*.json` from `astrid_core::app::schema`; `check-schema`
//! regenerates and compares. Exactly the shape `gen-design`/`check-design` and
//! `gen-strings`/`check-strings` already have, for the same two reasons: a generated file that is
//! COMMITTED can be reviewed in a diff — a schema change is a wire change, and nobody should have
//! to run a generator to see one — and a check that regenerates cannot be fooled by a hand edit.
//!
//! ## Why the file is not the source of truth
//!
//! The derive is. A consumer's bindings gate calls `astrid_core::app::schema::command()` rather
//! than reading `generated/schema/command.json`, the same way it already calls
//! `command_kinds()` rather than parsing `command.rs` — one pinned revision, one answer, nothing
//! to vendor and nothing to go stale.
//!
//! The files are for the consumers that CANNOT call a Rust function: Android generates
//! `model/*.kt` from them at build time. For those, `check-schema` is what makes the committed
//! copy trustworthy.

use std::path::Path;
use std::process::ExitCode;

/// Where the documents are written, relative to the repository root.
pub const OUTPUT: &str = "generated/schema";

/// `cargo xtask gen-schema`.
pub fn generate(root: &Path) -> ExitCode {
    let directory = root.join(OUTPUT);
    if let Err(err) = std::fs::create_dir_all(&directory) {
        eprintln!("cannot create {}: {err}", directory.display());
        return ExitCode::FAILURE;
    }
    for (name, contents) in documents() {
        let path = directory.join(&name);
        if let Err(err) = std::fs::write(&path, contents) {
            eprintln!("cannot write {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {OUTPUT}/{name}");
    }
    ExitCode::SUCCESS
}

/// `cargo xtask check-schema`: the committed documents are what the types produce.
///
/// A failure here is one of two things, and the message says both because they want opposite
/// actions: a wire change that nobody regenerated for (run the generator, and read the diff — it
/// is the list of what every client must now follow), or a hand edit of a generated file (revert
/// it; the types are the source).
pub fn check(root: &Path) -> ExitCode {
    let stale = stale_documents(root);
    if stale.is_empty() {
        println!("\nOK");
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "\n{} schema document(s) are not what this core's types produce.",
        stale.len()
    );
    eprintln!(
        "Run `cargo xtask gen-schema` and READ THE DIFF: a change here is a wire change, and"
    );
    eprintln!("every client's bindings gate is about to see it. If you edited a generated file by");
    eprintln!("hand, revert it — the Rust types are the source.");
    ExitCode::FAILURE
}

/// Which committed documents disagree with what the types produce, as lines to print.
///
/// A function rather than inline, so the check is testable from this crate's own suite: a
/// developer who changes a wire type finds out from `cargo test`, not only from CI.
fn stale_documents(root: &Path) -> Vec<String> {
    let directory = root.join(OUTPUT);
    let mut stale = Vec::new();
    for (name, expected) in documents() {
        let path = directory.join(&name);
        match std::fs::read_to_string(&path) {
            // Line endings are a fact about `core.autocrlf`, not about the wire — the same trap
            // `check-contracts` documents at length (task f1c7b53f).
            Ok(actual) if lf(&actual) == lf(&expected) => println!("ok       {OUTPUT}/{name}"),
            Ok(_) => {
                println!("STALE    {OUTPUT}/{name}");
                stale.push(format!("STALE {OUTPUT}/{name}"));
            }
            Err(_) => {
                println!("MISSING  {OUTPUT}/{name}");
                stale.push(format!("MISSING {OUTPUT}/{name}"));
            }
        }
    }
    stale
}

/// Each document's file name and the exact bytes to write.
///
/// Pretty-printed with a trailing newline: these are committed files that get read in diffs and by
/// people, and a one-line 200 KB JSON blob is neither.
fn documents() -> Vec<(String, String)> {
    astrid_core::app::schema::documents()
        .into_iter()
        .map(|(name, document)| {
            let body = serde_json::to_string_pretty(&document)
                .unwrap_or_else(|err| panic!("{name} could not be serialised: {err}"));
            (name.to_string(), format!("{body}\n"))
        })
        .collect()
}

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::{documents, stale_documents, OUTPUT};

    /// Every document is named, non-empty, and ends in exactly one newline — the shape a committed
    /// file has, so a regeneration does not show up as a whitespace diff on every line.
    #[test]
    fn each_document_is_a_committed_files_shape_9ef7edb4() {
        let written = documents();
        assert_eq!(written.len(), 3, "command, rule and wire");
        for (name, body) in written {
            assert!(name.ends_with(".json"), "{name} should be a .json file");
            assert!(body.ends_with("}\n"), "{name} should end in one newline");
            assert!(
                body.contains("\n  \""),
                "{name} should be pretty-printed, so a diff is readable"
            );
        }
    }

    /// The committed files are current. This is `check-schema` itself, run as a test, so a
    /// developer who changes a wire type and runs `cargo test` finds out immediately rather than
    /// in CI — the same courtesy `check-strings` gets from its own suite.
    #[test]
    fn the_committed_schema_is_current_9ef7edb4() {
        let stale = stale_documents(&super::super::repo_root());
        assert!(
            stale.is_empty(),
            "{OUTPUT} is stale; run `cargo xtask gen-schema`: {stale:?}"
        );
    }
}
