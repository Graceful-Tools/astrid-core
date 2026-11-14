//! One source for every repository's `deny.toml` — task `32d7c04d`.
//!
//! Four Rust workspaces carry a cargo-deny config: this one, astrid-windows, astrid-ios' `core/`
//! and astrid-android's `core/`. They used to be four hand-copied files that drifted — the package
//! count in the computed-licences comment, the prose, and by 2026-11-14 a licence (`BSD-2-Clause`,
//! for `arrayref`) that this repository had added and the other three did not know about.
//!
//! What they are NOT is one shared config read across repositories, and the distinction is the
//! point. Both the core's and astrid-windows' headers argued against sharing, in the same words:
//! a licence decision in one repo must not silently change the gate in another, because the repos
//! are pinned to each other by revision precisely so that nothing crosses implicitly. A GENERATED
//! copy keeps that. The copy in a consumer changes when its pin is bumped, like every other shared
//! decision, and in between it gates that repository's own lockfile on its own.
//!
//! The generation is the easy half. The check is the one that matters, because a generator nobody
//! can verify against is a script that rots, and `cargo deny` itself cannot be run on the Windows
//! development machine at all (`cargo install cargo-deny --locked` dies in `ring`'s build script
//! for want of clang). So both ends are plain `cargo test`:
//!
//! - here, [`check`] and [`the_committed_copies_are_current_32d7c04d`] ask whether
//!   `supply-chain/deny.<repo>.toml` is still what `deny.toml` renders;
//! - in each consumer, a test compares its committed `deny.toml` against
//!   [`astrid_core::supply_chain`], which is this directory's files by `include_str!`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The line in `deny.toml` that opens the per-repo tail. Everything from it to the end of the file
/// is replaced for each consumer; everything above is identical in all four copies.
///
/// A marker rather than "parse the `[sources]` table", because the tail is prose as well as a list
/// and the prose differs too — and because a parser that silently found nothing would generate
/// four identical files that all claimed to be right.
pub const MARKER: &str = "# <<per-repo: allow-git>>";

/// This core's git URL, which is the only entry any consumer's `allow-git` has ever had.
const CORE_URL: &str = "https://github.com/Graceful-Tools/astrid-core.git";

/// A repository that takes a generated copy of the config.
pub struct Consumer {
    /// The checkout directory's name, which is also the generated file's: `deny.<repo>.toml`.
    pub repo: &'static str,
    /// Its `[sources] allow-git`. Empty would mean a repository that takes this core without a git
    /// dependency on it, which none does today.
    pub allow_git: &'static [&'static str],
}

/// Every repository whose `deny.toml` is generated from this one.
///
/// astrid-web is not here and should not be: its Rust is `astrid-rules` compiled to WebAssembly
/// from a vendored copy, with no Cargo workspace and no lockfile of its own to gate.
pub const CONSUMERS: &[Consumer] = &[
    Consumer {
        repo: "astrid-windows",
        allow_git: &[CORE_URL],
    },
    Consumer {
        repo: "astrid-ios",
        allow_git: &[CORE_URL],
    },
    Consumer {
        repo: "astrid-android",
        allow_git: &[CORE_URL],
    },
];

/// Where a consumer's generated copy is written.
pub fn generated_path(root: &Path, repo: &str) -> PathBuf {
    root.join("supply-chain").join(format!("deny.{repo}.toml"))
}

/// The shared half of the source: everything above [`MARKER`].
///
/// `Err` when the marker is gone, which is the one failure that must never be quiet — without it
/// every consumer's copy would be the whole source file including an `allow-git = []` that denies
/// the git dependency each of them actually has.
fn shared_half(source: &str) -> Result<&str, String> {
    match source.find(MARKER) {
        Some(at) => Ok(&source[..at]),
        None => Err(format!(
            "deny.toml has no `{MARKER}` line, so there is nothing to generate from. It marks \
             where the per-repo tail begins; put it back above `allow-git`."
        )),
    }
}

/// A consumer's whole file: a generated header, the shared half verbatim, and its own tail.
pub fn render(source: &str, consumer: &Consumer) -> Result<String, String> {
    let repo = consumer.repo;
    let entries = consumer
        .allow_git
        .iter()
        .map(|url| format!("\"{url}\""))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(format!(
        "# GENERATED for {repo} by `cargo xtask gen-deny` in astrid-core. DO NOT EDIT HERE.\n\
         #\n\
         # The source is astrid-core's own `deny.toml`. A change to what the Rust workspaces accept\n\
         # is made there and reaches this repository when its pin on the core is bumped, like every\n\
         # other shared decision — editing this copy instead makes the gate disagree with the core\n\
         # it pins, which is what this repository's own test fails on.\n\
         #\n\
         {shared}{MARKER} — {repo}\n\
         #\n\
         # The one git dependency this repository has, and the reason it has one: it pins the shared\n\
         # core by revision. Matched by URL and not by revision, so bumping the pin — which is how\n\
         # every shared-behaviour change arrives — does not need this list touched.\n\
         allow-git = [{entries}]\n",
        shared = shared_half(source)?,
    ))
}

/// `deny.toml`'s bytes as text, with the line endings a checkout chose collapsed to LF.
fn source(root: &Path) -> Result<String, String> {
    let path = root.join("deny.toml");
    std::fs::read_to_string(&path)
        .map(|read| lf_endings(&read))
        .map_err(|err| format!("cannot read {}: {err}", path.display()))
}

/// CRLF collapsed to LF, and nothing else touched.
///
/// Every comparison here goes through it, for the reason task `f1c7b53f` wrote down: a checkout
/// with `core.autocrlf=true` holds CRLF in its working tree while the committed bytes are LF, and
/// a byte comparison between two checkouts that disagree reports drift in every file at once —
/// a fact about `git config`, not about the config being gated.
pub fn lf_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Writes every consumer's copy.
pub fn generate(root: &Path) -> ExitCode {
    let source = match source(root) {
        Ok(source) => source,
        Err(why) => {
            eprintln!("{why}");
            return ExitCode::FAILURE;
        }
    };

    let directory = root.join("supply-chain");
    if let Err(err) = std::fs::create_dir_all(&directory) {
        eprintln!("cannot create {}: {err}", directory.display());
        return ExitCode::FAILURE;
    }

    for consumer in CONSUMERS {
        let rendered = match render(&source, consumer) {
            Ok(rendered) => rendered,
            Err(why) => {
                eprintln!("{why}");
                return ExitCode::FAILURE;
            }
        };
        let path = generated_path(root, consumer.repo);
        if let Err(err) = std::fs::write(&path, rendered.as_bytes()) {
            eprintln!("cannot write {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }

    println!(
        "\n{} copies written. Each consumer takes its own when it bumps its pin on this core.",
        CONSUMERS.len()
    );
    ExitCode::SUCCESS
}

/// Whether every consumer's committed copy is what `deny.toml` renders today.
///
/// Returns the problems rather than printing them, so the xtask test below asks exactly the
/// question the command does instead of a near-miss of it.
pub fn stale_copies(root: &Path) -> Result<Vec<String>, String> {
    let source = source(root)?;
    let mut stale = Vec::new();
    for consumer in CONSUMERS {
        let expected = render(&source, consumer)?;
        let path = generated_path(root, consumer.repo);
        match std::fs::read_to_string(&path) {
            Ok(found) if lf_endings(&found) == expected => {}
            Ok(_) => stale.push(format!("{} is not what deny.toml renders", path.display())),
            Err(err) => stale.push(format!("{} could not be read: {err}", path.display())),
        }
    }
    Ok(stale)
}

/// Fails when a copy has been hand-edited or the source has moved on without regenerating.
pub fn check(root: &Path) -> ExitCode {
    let stale = match stale_copies(root) {
        Ok(stale) => stale,
        Err(why) => {
            eprintln!("{why}");
            return ExitCode::FAILURE;
        }
    };

    if stale.is_empty() {
        for consumer in CONSUMERS {
            println!("ok  {}", generated_path(root, consumer.repo).display());
        }
        println!("\nOK");
        return ExitCode::SUCCESS;
    }

    eprintln!();
    for problem in &stale {
        eprintln!("STALE  {problem}");
    }
    eprintln!(
        "\nRun `cargo xtask gen-deny`. These files are generated from deny.toml; a change to"
    );
    eprintln!(
        "what the workspaces accept belongs there, and a consumer takes it by bumping its pin."
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::{generated_path, lf_endings, render, stale_copies, Consumer, CONSUMERS, MARKER};

    const SOURCE: &str = "\
# A header.
[licenses]
allow = [\"MIT\"]

[sources]
unknown-git = \"deny\"
# <<per-repo: allow-git>>
#
# This repository depends on nothing by git.
allow-git = []
";

    fn windows() -> &'static Consumer {
        CONSUMERS
            .iter()
            .find(|consumer| consumer.repo == "astrid-windows")
            .expect("astrid-windows takes a generated copy")
    }

    /// The whole of the source above the marker reaches the copy UNCHANGED. That is what makes
    /// "the four configs agree" a thing a test can state: if any of it were rewritten per repo,
    /// the copies would differ for reasons no check could tell from drift.
    #[test]
    fn everything_above_the_marker_is_copied_verbatim_32d7c04d() {
        let rendered = render(SOURCE, windows()).expect("the fixture has a marker");
        let shared = SOURCE.split(MARKER).next().expect("split yields a head");

        assert!(
            rendered.contains(shared),
            "the shared half should appear verbatim, rendered:\n{rendered}"
        );
        assert!(rendered.contains("[licenses]"));
        assert!(rendered.contains("unknown-git = \"deny\""));
    }

    /// `allow-git` is the only variance, and it is the consumer's own. The source's empty list and
    /// the prose explaining it must NOT survive into a copy — a consumer that inherited
    /// `allow-git = []` would deny the git dependency on this very core.
    #[test]
    fn the_tail_is_replaced_with_the_consumers_own_32d7c04d() {
        let rendered = render(SOURCE, windows()).expect("the fixture has a marker");

        assert!(rendered
            .contains("allow-git = [\"https://github.com/Graceful-Tools/astrid-core.git\"]"));
        assert!(
            !rendered.contains("allow-git = []"),
            "the source's empty list must not reach a consumer, rendered:\n{rendered}"
        );
        assert!(
            !rendered.contains("This repository depends on nothing by git."),
            "the source's own tail prose is not a consumer's"
        );
    }

    /// A copy says it is generated, and says where the source is. Without that line the obvious
    /// move on finding the file out of date is to edit it, which is the drift this ended.
    #[test]
    fn a_copy_names_its_generator_and_its_repo_32d7c04d() {
        for consumer in CONSUMERS {
            let rendered = render(SOURCE, consumer).expect("the fixture has a marker");
            assert!(rendered.starts_with(&format!(
                "# GENERATED for {} by `cargo xtask gen-deny` in astrid-core. DO NOT EDIT HERE.",
                consumer.repo
            )));
            assert!(rendered.contains("astrid-core's own `deny.toml`"));
        }
    }

    /// A source with no marker is an ERROR, never a copy of the whole file. The quiet failure is
    /// the dangerous one: three consumers would each get `allow-git = []` and their gates would go
    /// red on the core dependency itself, pointing at everything except the cause.
    #[test]
    fn a_source_without_the_marker_fails_loudly_32d7c04d() {
        let why = render("[sources]\nallow-git = []\n", windows())
            .expect_err("a source with no marker cannot be rendered");
        assert!(why.contains(MARKER), "it should name the marker: {why}");
    }

    /// Task `f1c7b53f`'s lesson, applied before it can bite: the comparison normalises CRLF, so a
    /// checkout with `core.autocrlf=true` does not report all three copies as stale at once.
    #[test]
    fn line_endings_are_not_drift_32d7c04d() {
        let rendered = render(SOURCE, windows()).expect("the fixture has a marker");
        let as_checked_out = rendered.replace('\n', "\r\n");

        assert_ne!(
            as_checked_out, rendered,
            "the fixture should differ in bytes"
        );
        assert_eq!(lf_endings(&as_checked_out), rendered);
    }

    /// The copies this repository actually ships are current. This is the half that catches a
    /// hand-edited file, or the source changing with nobody running the generator — and it runs in
    /// an ordinary `cargo test`, which matters because `cargo deny` cannot be installed on the
    /// Windows development machine at all.
    #[test]
    fn the_committed_copies_are_current_32d7c04d() {
        let root = super::super::repo_root();
        let stale = stale_copies(&root).expect("deny.toml is committed at the repo root");
        assert!(
            stale.is_empty(),
            "run `cargo xtask gen-deny`:\n  {}",
            stale.join("\n  ")
        );
    }

    /// Every consumer takes the core by git, so every generated copy allows exactly that URL. A
    /// row added with an empty list would generate a file that denies the dependency it exists for.
    #[test]
    fn every_consumer_allows_the_core_by_git_32d7c04d() {
        for consumer in CONSUMERS {
            assert_eq!(
                consumer.allow_git,
                &["https://github.com/Graceful-Tools/astrid-core.git"],
                "{} should allow this core by URL",
                consumer.repo
            );
            assert!(generated_path(std::path::Path::new("."), consumer.repo)
                .to_string_lossy()
                .ends_with(&format!("deny.{}.toml", consumer.repo)));
        }
    }
}
