//! Repo automation, run as `cargo xtask <command>`.
//!
//! Checks live here rather than in shell scripts so they run identically on a developer machine and
//! on CI, and so a contributor can discover them with `cargo xtask`.

use std::process::ExitCode;

mod assets;
mod core_contracts;
mod design;

const COMMANDS: &[(&str, &str)] = &[
    (
        "check-contracts",
        "verify contracts/fixtures still says what astrid-web's does [path, default ../astrid-web]",
    ),
    (
        "gen-core-contracts",
        "write the core-first fixtures (contracts/fixtures/waiting.json) from this crate's rules",
    ),
    (
        "check-core-contracts",
        "verify the core-first fixtures are what this crate's rules answer",
    ),
    (
        "check-pins",
        "verify CONSUMERS.md's revisions are real, merged commits [consumer checkout paths]",
    ),
    (
        "gen-design",
        "write design/generated from design/tokens.json and design/assets",
    ),
    (
        "check-design",
        "verify design/generated is current, and astrid-web's vendored copy [path, default ../astrid-web]",
    ),
];

fn main() -> ExitCode {
    let Some(command) = std::env::args().nth(1) else {
        usage();
        return ExitCode::FAILURE;
    };

    match command.as_str() {
        "check-contracts" => check_contracts(),
        "check-pins" => check_pins(),
        "gen-core-contracts" => core_contracts::write_all(&repo_root()),
        "check-core-contracts" => core_contracts::check_all(&repo_root()),
        "gen-design" => design::generate(&repo_root()),
        "check-design" => design::check(
            &repo_root(),
            std::env::args().nth(2).map(std::path::PathBuf::from),
        ),
        other => {
            eprintln!("unknown command: {other}\n");
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!("usage: cargo xtask <command>\n");
    for (name, description) in COMMANDS {
        eprintln!("  {name:<18} {description}");
    }
}

/// Compares `contracts/fixtures` against a local astrid-web checkout's and fails if they differ.
///
/// The fixtures are GENERATED IN astrid-web, by `scripts/export-contract-fixtures.ts`, and its own
/// predeploy fails when they are stale. So the question here is not "are these still what web
/// defines" — web answers that — but "are our copies still web's". That is a comparison, and it
/// needs no Node.
///
/// This used to run a second exporter that lived in THIS repo, which meant core was checked
/// against a retired generator while the two copies quietly drifted apart (AWTD-1186).
fn check_contracts() -> ExitCode {
    let root = repo_root();
    let web = std::env::args()
        .nth(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("../astrid-web"));
    let theirs = web.join("contracts/fixtures");
    let ours = root.join("contracts/fixtures");

    if !theirs.is_dir() {
        eprintln!("no astrid-web fixtures at {}", theirs.display());
        eprintln!("usage: cargo xtask check-contracts [path-to-astrid-web]");
        return ExitCode::FAILURE;
    }

    let names = match std::fs::read_dir(&theirs) {
        Ok(entries) => {
            let mut names: Vec<_> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name())
                .filter(|n| n.to_string_lossy().ends_with(".json"))
                .collect();
            names.sort();
            names
        }
        Err(err) => {
            eprintln!("could not read {}: {err}", theirs.display());
            return ExitCode::FAILURE;
        }
    };

    if names.is_empty() {
        eprintln!("astrid-web has no fixtures at {}", theirs.display());
        return ExitCode::FAILURE;
    }

    let mut stale = 0usize;
    for name in &names {
        let display = name.to_string_lossy();
        // A core-first fixture is ours, and web's copy follows the revision web pins: a newer one
        // here is web being behind, which its own gate reports, not drift in this repository.
        if core_contracts::CORE_OWNED.contains(&display.as_ref()) {
            println!("ok  contracts/fixtures/{display} (core-owned; web copies it)");
            continue;
        }
        match (
            std::fs::read(theirs.join(name)),
            std::fs::read(ours.join(name)),
        ) {
            (Ok(expected), Ok(actual)) if same_fixture(&expected, &actual) => {
                println!("ok  contracts/fixtures/{display}");
            }
            (Ok(_), Ok(_)) => {
                eprintln!("DIFFERS  contracts/fixtures/{display}");
                stale += 1;
            }
            (Ok(_), Err(_)) => {
                eprintln!("MISSING  contracts/fixtures/{display}");
                stale += 1;
            }
            (Err(err), _) => {
                eprintln!("could not read astrid-web's {display}: {err}");
                stale += 1;
            }
        }
    }

    // A fixture we hold that web no longer generates is drift too: it is compiled into the Rust
    // tests and nothing updates it any more.
    if let Ok(entries) = std::fs::read_dir(&ours) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(".json") {
                continue;
            }
            if core_contracts::CORE_OWNED.contains(&name.to_string_lossy().as_ref()) {
                println!(
                    "ok  contracts/fixtures/{} (core-owned; generated here)",
                    name.to_string_lossy()
                );
                continue;
            }
            if !theirs.join(&name).exists() {
                eprintln!(
                    "ORPHANED  contracts/fixtures/{} — astrid-web no longer generates it",
                    name.to_string_lossy()
                );
                stale += 1;
            }
        }
    }

    if stale == 0 {
        return ExitCode::SUCCESS;
    }
    eprintln!("\n{stale} fixture(s) are not astrid-web's.");
    eprintln!("Regenerate in astrid-web (npm run export:contract-fixtures) and copy them here.");
    ExitCode::FAILURE
}

/// Whether two fixture bodies say the same thing, ignoring the line endings a checkout chose.
///
/// Not a byte comparison, deliberately. `core.autocrlf=true` in one of the two checkouts and not
/// the other — the ordinary state of a Windows machine, since neither repo commits a
/// `.gitattributes` — makes every fixture differ in its working tree while the committed bytes are
/// identical. That is a fact about `git config`, not about the contract, and the gate reporting it
/// as drift sent a developer off to regenerate files that were already correct (f1c7b53f).
///
/// Only CRLF collapses to LF, which is exactly the translation git did on checkout. Anything else
/// still counts.
fn same_fixture(expected: &[u8], actual: &[u8]) -> bool {
    lf_endings(expected) == lf_endings(actual)
}

fn lf_endings(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (index, byte) in bytes.iter().enumerate() {
        let opens_crlf = *byte == b'\r' && bytes.get(index + 1) == Some(&b'\n');
        if !opens_crlf {
            out.push(*byte);
        }
    }
    out
}

/// The four repositories that pin this core. A fifth that is not listed here fails the check
/// rather than going unrecorded, which is the whole point: the November 2026 platform assessment
/// read the Windows pin as 96 commits behind off a manifest that had already been bumped three
/// times, because nothing anywhere said what the live revisions were.
const CONSUMERS: &[&str] = &[
    "astrid-web",
    "astrid-windows",
    "astrid-ios",
    "astrid-android",
];

/// Where each consumer records its pin, relative to the root of its own checkout, and in the order
/// to try. The first candidate that exists is the one read.
///
/// Two shapes, not four: astrid-web keeps the sha alone in a file, and the other three take this
/// core as a Cargo git dependency, so their sha is the `rev =` beside `astrid-core` in a
/// `Cargo.toml`. More than one candidate is allowed because a repository is free to move its
/// wrapper crate, and a path that has moved should report the row as *unchecked* rather than fail a
/// pin that is perfectly good.
const MANIFESTS: &[(&str, &[&str])] = &[
    ("astrid-web", &["packages/astrid-rules/REVISION"]),
    ("astrid-windows", &["crates/astrid-ffi/Cargo.toml"]),
    (
        "astrid-ios",
        &["core/Cargo.toml", "core/astrid-apple/Cargo.toml"],
    ),
    ("astrid-android", &["core/Cargo.toml"]),
];

/// Checks `CONSUMERS.md` against this repository's history.
///
/// Three questions, and only the third can fail for a reason outside this repo:
///
/// 1. Is every consumer listed, with a full forty-character sha? A short sha becomes ambiguous as
///    the history grows, and an ambiguous abbreviation fails ancestry in a way that reads like a
///    bad pin.
/// 2. Is each revision a commit here, and an **ancestor of `main`**? Being *behind* main is fine
///    and is only reported — a client bumps when it is ready. Being on a commit that is not in
///    main is a pin at a branch nobody merged.
/// 3. Does each consumer's own manifest agree with its row? Every checkout that is here is
///    compared against the file the table claims to describe; a consumer that is not here is
///    reported *unchecked*, never failed. CI has only the public astrid-web checkout unless
///    `CONSUMER_PINS_TOKEN` is set, and a fork's pull request has no secrets at all (task
///    4c891314).
fn check_pins() -> ExitCode {
    let root = repo_root();
    let table_path = root.join("CONSUMERS.md");
    let table = match std::fs::read_to_string(&table_path) {
        Ok(table) => table,
        Err(err) => {
            eprintln!("cannot read {}: {err}", table_path.display());
            return ExitCode::FAILURE;
        }
    };

    // `origin/main` FIRST, and the local branch only as a fallback. The remote is the shared
    // truth; a developer's local `main` is routinely a few commits behind, and preferring it
    // reported a correctly-pinned consumer as sitting on an unmerged branch — which is this
    // check's loudest failure and would have sent someone hunting a pin that was fine. On a pull
    // request CI checks out a detached head, where `main` exists only as `origin/main` anyway.
    let Some(main) = ["origin/main", "main"]
        .into_iter()
        .find(|reference| git(&root, &["rev-parse", "--verify", reference]).is_some())
    else {
        eprintln!("neither main nor origin/main is in this checkout, so ancestry cannot be asked.");
        eprintln!("CI needs `fetch-depth: 0` — a shallow clone cannot answer it, and a wrong");
        eprintln!("answer would fail every pin at once.");
        return ExitCode::FAILURE;
    };

    let checkouts = consumer_checkouts(&root);

    let mut failed = Vec::new();
    let mut unchecked = Vec::new();
    println!("Consumer pins, against {main}:\n");

    for consumer in CONSUMERS {
        let Some(revision) = row_revision(&table, consumer) else {
            failed.push(format!(
                "{consumer} has no row in CONSUMERS.md with a revision in its last column."
            ));
            continue;
        };

        if !is_full_sha(&revision) {
            failed.push(format!(
                "{consumer} is pinned at '{revision}', which is not a full forty-character sha."
            ));
            continue;
        }

        if git(
            &root,
            &["cat-file", "-e", &format!("{revision}^{{commit}}")],
        )
        .is_none()
        {
            failed.push(format!(
                "{consumer} is pinned at {revision}, which is not a commit in this repository."
            ));
            continue;
        }

        if git(&root, &["merge-base", "--is-ancestor", &revision, main]).is_none() {
            failed.push(format!(
                "{consumer} is pinned at {revision}, which is NOT an ancestor of {main} — a branch nobody merged."
            ));
            continue;
        }

        let behind = git(
            &root,
            &["rev-list", "--count", &format!("{revision}..{main}")],
        )
        .unwrap_or_default();
        let dated = git(&root, &["log", "-1", "--format=%cs", &revision]).unwrap_or_default();
        let shape = format!(
            "  {consumer:<16} {} {dated}  {behind:>3} behind",
            &revision[..7]
        );

        let fallback = Checkout {
            path: root.join("..").join(consumer),
            named: false,
        };
        let checkout = checkouts.get(consumer).unwrap_or(&fallback);
        match read_manifest(consumer, &checkout.path, &revision) {
            Manifest::Agrees(path) => println!("{shape}  compared against {}", path.display()),
            Manifest::Disagrees { path, found } => {
                let problem = format!(
                    "{consumer} pins {found} in {} but CONSUMERS.md says {revision}.",
                    path.display()
                );
                if checkout.named {
                    println!("{shape}  MISMATCH");
                    failed.push(problem);
                } else {
                    println!("{shape}  differs from the checkout beside this one");
                    unchecked.push(format!("{problem} That checkout was not named on the command line, so it may simply need a pull."));
                }
            }
            Manifest::Unread(why) => {
                println!("{shape}  UNCHECKED");
                unchecked.push(format!("{consumer}: {why}"));
            }
        }
    }

    if !unchecked.is_empty() {
        println!("\nReported, not failed:");
        for row in &unchecked {
            println!("  {row}");
        }
        println!("\nNone of these is a failure. CI holds the three private checkouts only when");
        println!("CONSUMER_PINS_TOKEN is set, a fork's pull request has no secrets at all, and a");
        println!("checkout found beside this one is a working copy that may just be behind.");
    }

    if failed.is_empty() {
        println!("\nOK");
        return ExitCode::SUCCESS;
    }
    eprintln!();
    for problem in &failed {
        eprintln!("FAIL  {problem}");
    }
    eprintln!("\nThe pins table is out of date. Fix the row, or fix the pin — CONSUMERS.md says");
    eprintln!("which, and how a bump is supposed to go.");
    ExitCode::FAILURE
}

/// What asking a consumer's own manifest produced.
enum Manifest {
    /// The manifest at this path records the revision the table does.
    Agrees(std::path::PathBuf),
    /// It records a different one. The table and the client disagree, which is the failure
    /// `CONSUMERS.md` exists to prevent.
    Disagrees {
        path: std::path::PathBuf,
        found: String,
    },
    /// No comparison was possible, and why. **Never a failure.** CI without
    /// `CONSUMER_PINS_TOKEN` cannot clone the three private repositories, and neither can a fork's
    /// pull request — failing there would make every outside contribution red for a reason the
    /// contributor cannot fix.
    Unread(String),
}

/// Compares `consumer`'s row against the manifest in its own checkout.
///
/// A checkout that is absent, holds none of the candidate manifests, or holds one this cannot read
/// a revision out of, is `Unread` with that said plainly. The last of those is a defect in this
/// parser rather than in the pin, and the printed line is how anyone finds out — so it is loud, but
/// it is not red.
fn read_manifest(consumer: &str, checkout: &std::path::Path, recorded: &str) -> Manifest {
    let Some((_, candidates)) = MANIFESTS.iter().find(|(name, _)| name == &consumer) else {
        return Manifest::Unread(format!("no manifest is recorded for {consumer}"));
    };

    if !checkout.is_dir() {
        return Manifest::Unread(format!("no checkout at {}", checkout.display()));
    }

    let Some(path) = candidates
        .iter()
        .map(|candidate| checkout.join(candidate))
        .find(|path| path.is_file())
    else {
        return Manifest::Unread(format!(
            "none of {} under {}",
            candidates.join(", "),
            checkout.display()
        ));
    };

    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Manifest::Unread(format!("{} could not be read", path.display()));
    };

    match manifest_revision(&path, &contents) {
        Some(found) if found == recorded => Manifest::Agrees(path),
        Some(found) => Manifest::Disagrees { path, found },
        None => Manifest::Unread(format!("no revision found in {}", path.display())),
    }
}

/// A consumer's checkout, and whether the caller named it.
struct Checkout {
    path: std::path::PathBuf,
    /// A checkout named on the command line is a deliberate statement that it is the right one, so
    /// a mismatch against it FAILS. One merely found beside this repository is a developer's
    /// working copy, which is routinely a few days behind its own remote, so a mismatch there is
    /// reported and no more. CI clones fresh and names every path; a sibling cannot be told from a
    /// stale one from in here, and a gate that says "the pins table is out of date" about a
    /// checkout nobody pulled is a gate people learn to ignore.
    named: bool,
}

/// Where each consumer's checkout is.
///
/// A positional argument is matched to a consumer by its directory name, so one invocation names
/// all four in any order and `cargo xtask check-pins ../astrid-web` — what CI ran while web was
/// the only row it could compare — still means exactly what it meant. Anything left unnamed is
/// looked for beside this repository, which is how a development machine with the siblings checked
/// out compares every row for free.
fn consumer_checkouts(root: &std::path::Path) -> std::collections::HashMap<&'static str, Checkout> {
    let mut checkouts: std::collections::HashMap<&'static str, Checkout> = CONSUMERS
        .iter()
        .map(|consumer| {
            (
                *consumer,
                Checkout {
                    path: root.join("..").join(consumer),
                    named: false,
                },
            )
        })
        .collect();

    for argument in std::env::args().skip(2) {
        let path = std::path::PathBuf::from(&argument);
        let named = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| CONSUMERS.iter().find(|consumer| **consumer == name));
        match named {
            Some(consumer) => {
                checkouts.insert(*consumer, Checkout { path, named: true });
            }
            // Silently ignoring this is how a typo stops a row being compared while the check
            // still says OK — the exact shape of the problem this check exists to catch.
            None => eprintln!(
                "warning: '{argument}' does not name a consumer, so it was not used. The directory\n\
                 must be named for the consumer it holds (one of {}).",
                CONSUMERS.join(", ")
            ),
        }
    }

    checkouts
}

/// The revision a manifest records, by the kind of file it is.
///
/// A `REVISION` file is the sha and nothing else. A `Cargo.toml` carries it as the `rev =` on the
/// `astrid-core` git dependency — and a `rev` belonging to some OTHER git dependency must not be
/// mistaken for the core's, so the line (or the `[dependencies.astrid-core]` section it sits in)
/// has to name `astrid-core` too.
fn manifest_revision(path: &std::path::Path, contents: &str) -> Option<String> {
    if path.file_name().is_some_and(|name| name == "Cargo.toml") {
        return cargo_core_revision(contents);
    }
    let revision = contents.trim();
    (!revision.is_empty()).then(|| revision.to_string())
}

/// The `rev = "<sha>"` on the `astrid-core` git dependency, inline or sectioned.
///
/// Both forms, because nothing stops a consumer writing it either way and reading the wrong one
/// would report a pin that is fine as unreadable. Comments are dropped first: the Windows manifest
/// explains its own `rev` in four lines of prose above it, and a parser that read those would find
/// the word before the value.
fn cargo_core_revision(contents: &str) -> Option<String> {
    let mut in_core_section = false;
    for line in contents.lines() {
        // Naive, and sufficient: a `#` inside a value would have to be in a quoted string, and no
        // consumer's git URL or sha contains one.
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_core_section = line.trim_end_matches(']').ends_with("astrid-core");
            continue;
        }
        if !in_core_section && !line.contains("astrid-core") {
            continue;
        }
        if let Some(revision) = rev_value(line) {
            return Some(revision);
        }
    }
    None
}

/// The value of a `rev` key on one line of TOML, whether it is the only key or one of several.
///
/// `rev` has to be a key and not the tail of another word, or `prev = "…"` would answer.
fn rev_value(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(at) = rest.find("rev") {
        let is_own_word = !rest[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let after = rest[at + 3..].trim_start();
        if is_own_word {
            if let Some(value) = after
                .strip_prefix('=')
                .map(str::trim_start)
                .and_then(|after| after.strip_prefix('"'))
                .and_then(|after| after.split('"').next())
            {
                return Some(value.to_string());
            }
        }
        rest = &rest[at + 3..];
    }
    None
}

/// A revision read out of `CONSUMERS.md`'s table: the last backticked cell of the row whose first
/// cell names `consumer`.
///
/// The sha comes from the LAST cell so the prose in the middle cells is free to change without
/// touching this parser, and the row is found by name so reordering the table is harmless.
fn row_revision(table: &str, consumer: &str) -> Option<String> {
    // Body rows only: everything up to and including the `|---|---|` separator is the header, and
    // a lookup for a name that appears in the header ("Revision") otherwise comes back as a sha.
    table
        .lines()
        .skip_while(|line| !is_table_separator(line))
        .skip(1)
        .filter(|line| line.starts_with('|'))
        .find(|line| {
            line.split('|')
                .nth(1)
                .is_some_and(|first| first.contains(consumer))
        })
        .and_then(|line| {
            line.trim_end()
                .trim_end_matches('|')
                .rsplit('|')
                .next()
                .map(|cell| cell.trim().trim_matches('`').to_string())
        })
        .filter(|revision| !revision.is_empty())
}

/// The `|---|---|` line under a markdown table's header.
fn is_table_separator(line: &str) -> bool {
    line.starts_with('|')
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t' | '\r'))
}

fn is_full_sha(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// `git` in `root`, or `None` when it exited non-zero. The answer is the trimmed stdout, which is
/// empty for the commands asked only as a question (`cat-file -e`, `merge-base --is-ancestor`).
fn git(root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn repo_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is crates/xtask; the repo root is two levels up.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("xtask lives at <root>/crates/xtask")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::{
        is_full_sha, manifest_revision, read_manifest, row_revision, same_fixture, Manifest,
        CONSUMERS, MANIFESTS,
    };

    const TABLE: &str = "\
| Repository | Takes | Pinned in | Revision |
|---|---|---|---|
| [astrid-web](https://example.test) | `astrid-rules`, as WebAssembly | `packages/astrid-rules/REVISION` | `52a371d21781f59dd4814d68794f533df85587ae` |
| [astrid-windows](https://example.test) | the whole core | `crates/astrid-ffi/Cargo.toml` (`rev =`) | `e5f4e45efdf8b9064adfd534ced2e9d9b0181517` |
";

    /// Task 3c1c9092. The row's middle cells contain backticks and pipes-free prose of their own,
    /// including a `rev =` that looks like the thing being parsed — so the revision has to come
    /// from the row's LAST cell rather than from its first backticked span.
    #[test]
    fn a_revision_is_read_from_the_last_cell_3c1c9092() {
        assert_eq!(
            row_revision(TABLE, "astrid-windows").as_deref(),
            Some("e5f4e45efdf8b9064adfd534ced2e9d9b0181517")
        );
        assert_eq!(
            row_revision(TABLE, "astrid-web").as_deref(),
            Some("52a371d21781f59dd4814d68794f533df85587ae")
        );
    }

    /// `astrid-web` is a prefix of nothing here, but `astrid-ios` would match a row for
    /// `astrid-ios-legacy` — so a consumer with no row of its own must answer `None` and fail the
    /// check, not silently inherit a neighbour's sha. The header row must not match either.
    #[test]
    fn a_consumer_with_no_row_is_not_found_3c1c9092() {
        assert_eq!(row_revision(TABLE, "astrid-ios"), None);
        assert_eq!(row_revision(TABLE, "astrid-android"), None);
        assert_eq!(row_revision(TABLE, "Repository"), None);
    }

    /// A short sha becomes ambiguous as the history grows, and `merge-base --is-ancestor` on an
    /// abbreviation that has become ambiguous fails in a way that reads like a bad pin. So the
    /// shape is checked before the question is asked.
    #[test]
    fn only_a_full_lowercase_sha_passes_3c1c9092() {
        assert!(is_full_sha("e5f4e45efdf8b9064adfd534ced2e9d9b0181517"));
        assert!(!is_full_sha("e5f4e45"));
        assert!(!is_full_sha("E5F4E45EFDF8B9064ADFD534CED2E9D9B0181517"));
        assert!(!is_full_sha("e5f4e45efdf8b9064adfd534ced2e9d9b018151z"));
        assert!(!is_full_sha(""));
    }

    /// The table the repository actually ships has a row for every consumer, so the check passes
    /// on the committed file rather than only on a fixture. This is the half that catches a fifth
    /// consumer added to `CONSUMERS` without a row, or a row deleted by a bad merge.
    #[test]
    fn the_committed_table_names_every_consumer_3c1c9092() {
        let table = std::fs::read_to_string(super::repo_root().join("CONSUMERS.md"))
            .expect("CONSUMERS.md is committed at the repo root");
        for consumer in CONSUMERS {
            let revision = row_revision(&table, consumer)
                .unwrap_or_else(|| panic!("{consumer} has no row with a revision in CONSUMERS.md"));
            assert!(
                is_full_sha(&revision),
                "{consumer} is pinned at '{revision}', which is not a full forty-character sha"
            );
        }
    }

    /// Task 4c891314. The three private consumers take this core as a Cargo git dependency, so
    /// their pin is the `rev =` on it — written inline with `git` and `version` on the same line,
    /// which is how astrid-windows writes it.
    #[test]
    fn a_cargo_manifest_pin_is_the_rev_on_the_core_4c891314() {
        let manifest = "\
[dependencies]
# The shared core, by revision. `cargo update -p astrid-core` after changing the rev.
astrid-core = { git = \"https://github.com/Graceful-Tools/astrid-core.git\", version = \"0.1\", rev = \"3e13a78b14b734de83c793124bf150aea637c7b3\" }
serde_json = { workspace = true }
";
        assert_eq!(
            manifest_revision(
                std::path::Path::new("crates/astrid-ffi/Cargo.toml"),
                manifest
            )
            .as_deref(),
            Some("3e13a78b14b734de83c793124bf150aea637c7b3")
        );
    }

    /// The sectioned form answers too, since nothing stops a consumer writing it that way and
    /// reading only the inline form would report a perfectly good pin as unreadable.
    #[test]
    fn the_sectioned_cargo_form_answers_too_4c891314() {
        let manifest = "\
[dependencies.astrid-core]
git = \"https://github.com/Graceful-Tools/astrid-core.git\"
rev = \"2cf2b5fb202ce99147d6a00c01e3041b1cdccf0a\"
";
        assert_eq!(
            manifest_revision(std::path::Path::new("core/Cargo.toml"), manifest).as_deref(),
            Some("2cf2b5fb202ce99147d6a00c01e3041b1cdccf0a")
        );
    }

    /// Another git dependency's `rev` is not this core's. Answering with it would compare the table
    /// against an unrelated repository's sha and report a mismatch nobody could act on — worse than
    /// the gap this task closes, because it would look like an answer.
    #[test]
    fn another_dependencys_rev_is_not_the_cores_4c891314() {
        let manifest = "\
[dependencies]
uniffi = { git = \"https://github.com/mozilla/uniffi-rs.git\", rev = \"1111111111111111111111111111111111111111\" }
";
        assert_eq!(
            manifest_revision(std::path::Path::new("core/Cargo.toml"), manifest),
            None
        );

        // And a key that merely ENDS in `rev` is not the key.
        let shadowed =
            "astrid-core = { git = \"x\", prev = \"2222222222222222222222222222222222222222\" }";
        assert_eq!(
            manifest_revision(std::path::Path::new("core/Cargo.toml"), shadowed),
            None
        );
    }

    /// astrid-web's manifest is the sha alone in a file, and that form still reads — this task
    /// generalised the comparison rather than replacing it.
    #[test]
    fn a_revision_file_is_the_sha_alone_4c891314() {
        assert_eq!(
            manifest_revision(
                std::path::Path::new("packages/astrid-rules/REVISION"),
                "52a371d21781f59dd4814d68794f533df85587ae\n"
            )
            .as_deref(),
            Some("52a371d21781f59dd4814d68794f533df85587ae")
        );
    }

    /// **An absent checkout is UNCHECKED, never a failure.** CI holds the three private
    /// repositories only when `CONSUMER_PINS_TOKEN` is set, and a fork's pull request gets no
    /// secrets at all — so failing here would turn every outside contribution red for a reason the
    /// contributor cannot fix, and the check would be worse than the gap it closed.
    #[test]
    fn an_absent_checkout_is_unchecked_not_failed_4c891314() {
        let nowhere = std::env::temp_dir().join("astrid-core-check-pins-4c891314-absent");
        let outcome = read_manifest(
            "astrid-ios",
            &nowhere,
            "3677cfb631d6fe415db5a77403e2c79f4e0b8c2b",
        );
        match outcome {
            Manifest::Unread(why) => assert!(
                why.contains("no checkout"),
                "an absent checkout should say so, said: {why}"
            ),
            Manifest::Agrees(_) | Manifest::Disagrees { .. } => {
                panic!("a directory that is not there cannot have been compared")
            }
        }
    }

    /// Every consumer needs somewhere to read its pin from, or `check-pins` would report it
    /// unchecked forever while looking like it had asked. The two tables are edited together.
    #[test]
    fn every_consumer_has_a_manifest_4c891314() {
        for consumer in CONSUMERS {
            let (_, candidates) = MANIFESTS
                .iter()
                .find(|(name, _)| name == consumer)
                .unwrap_or_else(|| panic!("{consumer} has no entry in MANIFESTS"));
            assert!(!candidates.is_empty(), "{consumer} has no candidate paths");
        }
    }

    /// Task f1c7b53f: `check-contracts` failed on all 13 fixtures on a Windows machine, because
    /// the astrid-web checkout had `core.autocrlf=true` and this one did not — so web's working
    /// tree held CRLF and ours held LF, while the committed bytes were identical in both repos.
    /// A comparison of working-tree bytes answers a question about local `git config`, not about
    /// the contract, and it reported the one cause that was not true: "Regenerate in astrid-web".
    #[test]
    fn line_endings_are_not_drift_f1c7b53f() {
        let lf = b"{\n  \"case\": 1\n}\n";
        let crlf = b"{\r\n  \"case\": 1\r\n}\r\n";

        assert!(same_fixture(crlf, lf), "CRLF against LF");
        assert!(same_fixture(lf, crlf), "LF against CRLF");
    }

    /// The point of the gate survives: a fixture whose CONTENT differs is still drift. If
    /// normalising line endings cost us this, it would have turned a noisy gate into a silent one.
    #[test]
    fn a_content_difference_is_still_drift_f1c7b53f() {
        let theirs = b"{\r\n  \"case\": 2\r\n}\r\n";
        let ours = b"{\n  \"case\": 1\n}\n";

        assert!(!same_fixture(theirs, ours));
    }

    /// Only CRLF collapses. A bare `\r` is not a line ending a checkout introduces, and inside a
    /// JSON string a literal carriage return has to be escaped (`\r`, two characters), so a raw
    /// `0x0D` byte that is not part of a CRLF is a real difference in the bytes web generated.
    #[test]
    fn a_lone_carriage_return_is_content_f1c7b53f() {
        assert!(!same_fixture(b"{\"a\":1}\r", b"{\"a\":1}"));
        assert!(!same_fixture(b"a\rb", b"ab"));
    }
}
