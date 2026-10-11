//! Repo automation, run as `cargo xtask <command>`.
//!
//! Checks live here rather than in shell scripts so they run identically on a developer machine and
//! on CI, and so a contributor can discover them with `cargo xtask`.

use std::process::ExitCode;

const COMMANDS: &[(&str, &str)] = &[(
    "check-contracts",
    "verify contracts/fixtures is byte-for-byte astrid-web's [path, default ../astrid-web]",
)];

fn main() -> ExitCode {
    let Some(command) = std::env::args().nth(1) else {
        usage();
        return ExitCode::FAILURE;
    };

    match command.as_str() {
        "check-contracts" => check_contracts(),
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
            (Ok(expected), Ok(actual)) if expected == actual => {
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

fn repo_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is crates/xtask; the repo root is two levels up.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("xtask lives at <root>/crates/xtask")
        .to_path_buf()
}
