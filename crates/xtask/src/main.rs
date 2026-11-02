//! Repo automation, run as `cargo xtask <command>`.
//!
//! Checks live here rather than in shell scripts so they run identically on a developer machine and
//! on CI, and so a contributor can discover them with `cargo xtask`.

use std::process::ExitCode;

const COMMANDS: &[(&str, &str)] = &[
    (
        "check-contracts",
        "verify contracts/fixtures still says what astrid-web's does [path, default ../astrid-web]",
    ),
    (
        "check-pins",
        "verify CONSUMERS.md's revisions are real, merged commits [astrid-web path for its row]",
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
/// 3. Does astrid-web's `REVISION` file agree with its row? That repo is public and CI already
///    checks it out, so that one row is compared against the manifest it describes. The other
///    three are private; see `CONSUMERS.md`.
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

    let mut failed = Vec::new();
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
        println!(
            "  {consumer:<16} {} {dated}  {behind} behind",
            &revision[..7]
        );
    }

    println!();

    let web = std::env::args()
        .nth(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("../astrid-web"));
    let revision_file = web.join("packages/astrid-rules/REVISION");
    match std::fs::read_to_string(&revision_file) {
        Ok(contents) => {
            let actual = contents.trim();
            let recorded = row_revision(&table, "astrid-web").unwrap_or_default();
            if actual == recorded {
                println!("astrid-web's REVISION file agrees with the table.");
            } else {
                failed.push(format!(
                    "astrid-web pins {actual} but CONSUMERS.md says {recorded}."
                ));
            }
        }
        Err(_) => println!(
            "No astrid-web checkout at {} — its row was checked for shape only.",
            web.display()
        ),
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
    use super::{is_full_sha, row_revision, same_fixture, CONSUMERS};

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
