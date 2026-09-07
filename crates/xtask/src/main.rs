//! Repo automation, run as `cargo xtask <command>`.
//!
//! Checks live here rather than in shell scripts so they run identically on a developer machine and
//! on CI, and so a contributor can discover them with `cargo xtask`.

use std::process::{Command, ExitCode};

const COMMANDS: &[(&str, &str)] = &[(
    "check-contracts",
    "verify contracts/fixtures matches what astrid-web currently defines",
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

/// Re-derives the fixtures from a local astrid-web checkout and fails if they differ from what is
/// committed. A rule that is retyped is a rule that drifts; this is what stops that.
fn check_contracts() -> ExitCode {
    let root = repo_root();
    let status = Command::new("node")
        .arg(root.join("contracts/export-from-web.mjs"))
        .arg("--check")
        .current_dir(&root)
        .status();

    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("could not run node: {err}");
            eprintln!("contract fixtures are generated from astrid-web; Node.js is required");
            ExitCode::FAILURE
        }
    }
}

fn repo_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is crates/xtask; the repo root is two levels up.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("xtask lives at <root>/crates/xtask")
        .to_path_buf()
}
