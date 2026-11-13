//! Resolve the revision of this core, so a client can say what it is actually running.
//!
//! Every shell knows its own version and none of them knew the core's: Windows'
//! `astrid_version()` returned astrid-ffi's `0.1.0`, which says nothing about the code in a crash
//! report, because the core is a separate repository that the clients pin by revision
//! (astrid-windows task `ab249b76`). `cargo` tells a build script the package version and nothing
//! about git, so the revision has to be resolved here and handed to the compiler as an env var.
//!
//! Three sources, in order:
//!
//! 1. **`ASTRID_BUILD_REV` in the environment.** A release pipeline knows the revision it checked
//!    out and should not depend on a `git` being present in the build image.
//! 2. **`git rev-parse --short=12 HEAD`** in this manifest's directory. This covers both a
//!    developer's checkout and a cargo git dependency, whose checkout under
//!    `~/.cargo/git/checkouts/` is a real repository at the pinned revision.
//! 3. **`"unknown"`.** A build from a tarball has no git and no pipeline. A crash report saying
//!    `unknown` is honest; an empty version field reads as a broken client.
//!
//! The constant is `astrid_core::BUILD_REV`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ASTRID_BUILD_REV");
    // So a commit re-stamps the constant rather than leaving yesterday's revision cached. Both
    // files are absent in a tarball build, and `rerun-if-changed` on a missing path is allowed.
    for path in ["../../.git/HEAD", "../../.git/index"] {
        println!("cargo:rerun-if-changed={}", manifest.join(path).display());
    }

    println!("cargo:rustc-env=ASTRID_BUILD_REV={}", resolve(&manifest));
}

/// The revision, by the three routes above. Never empty, never contains whitespace — the
/// constant's own test asserts both, and a report prints it unquoted.
fn resolve(manifest: &PathBuf) -> String {
    if let Some(handed_in) = std::env::var("ASTRID_BUILD_REV")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        return sanitise(&handed_in);
    }

    let from_git = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(manifest)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());

    match from_git {
        Some(revision) => sanitise(&revision),
        None => "unknown".to_owned(),
    }
}

/// Collapse anything that would break a single-token version string. A hand-in from a pipeline is
/// whatever a YAML file put there, and a revision with a space in it would split an about box.
fn sanitise(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_whitespace() {
                '-'
            } else {
                character
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}
