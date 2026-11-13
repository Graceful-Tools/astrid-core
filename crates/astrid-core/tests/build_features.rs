//! Which SQLite a client links, asserted where it is actually decided — task `f1210dd8`.
//!
//! rusqlite's `bundled` compiles SQLite into the binary. Windows and Android need that: neither
//! ships a `libsqlite3` an NDK or MSVC link can find. Apple does not — iOS and macOS link the
//! system SQLite already, so bundling a second copy cost the iOS binary and `libastrid_apple.a`
//! megabytes for a library the OS had, and it is how RUSTSEC-2026-0459 reached every native
//! client at once.
//!
//! Until this file existed the choice was made for everybody by one word in the workspace
//! manifest, so no consumer could opt out. It is now this crate's own `bundled-sqlite` feature,
//! **on by default**, and Apple turns it off with `default-features = false`.
//!
//! The gate cannot otherwise see build configuration: dropping `bundled-sqlite` from `default`
//! would compile, pass every one of the other tests here (they run with default features), and
//! fail only on a Windows or Android machine that then has no SQLite to link — a break nothing in
//! this repository would report. So the manifests are read as text and asserted, the same shape
//! astrid-windows' `supply_chain_contract.rs` reads `deny.toml` in.

use std::path::{Path, PathBuf};

/// This crate's directory; the workspace root is two above it.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{path:?} is the manifest being asserted; {e}"))
}

/// The one logical line a `key = ...` entry spans, out of a named table.
///
/// Dependency entries here are written inline, so a line is enough; a `[table]` header ends the
/// search so that `[dev-dependencies]`' rusqlite is never mistaken for `[workspace.dependencies]`'.
fn entry(manifest: &str, table: &str, key: &str) -> String {
    let header = format!("[{table}]");
    let body = manifest
        .split_once(&header)
        .unwrap_or_else(|| panic!("the manifest has no {header} table"))
        .1;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if let Some(rest) = line.strip_prefix(key) {
            if rest.trim_start().starts_with('=') {
                return line.to_string();
            }
        }
    }
    panic!("{header} has no `{key}` entry");
}

/// The workspace manifest must not decide `bundled` for every consumer.
///
/// It did until task `f1210dd8`: `rusqlite = { ..., features = ["bundled", ...] }` is not
/// overridable from a consumer — `default-features = false` does not remove an explicitly
/// requested feature — so Apple had no way to link the SQLite it already had.
#[test]
fn the_workspace_dependency_does_not_decide_bundled_sqlite() {
    let rusqlite = entry(
        &read(&crate_dir().join("../../Cargo.toml")),
        "workspace.dependencies",
        "rusqlite",
    );
    assert!(
        !rusqlite.contains("\"bundled\""),
        "the workspace rusqlite dependency requests `bundled`, which no consumer can turn off; \
         it belongs behind astrid-core's `bundled-sqlite` feature. Found: {rusqlite}"
    );
}

/// `bundled-sqlite` is declared, reaches rusqlite, and is on by default.
///
/// All three halves matter. A feature that does not name `rusqlite/bundled` compiles and does
/// nothing. A feature missing from `default` unbundles Windows and Android, which cannot link a
/// system SQLite, and the break appears only in their builds.
#[test]
fn bundled_sqlite_is_a_default_feature_that_reaches_rusqlite() {
    let manifest = read(&crate_dir().join("Cargo.toml"));

    let default = entry(&manifest, "features", "default");
    assert!(
        default.contains("bundled-sqlite"),
        "`default` must carry `bundled-sqlite`: Windows and Android have no system SQLite to link, \
         so unbundling is Apple's opt-in and never the default. Found: {default}"
    );

    let feature = entry(&manifest, "features", "bundled-sqlite");
    assert!(
        feature.contains("rusqlite/bundled"),
        "`bundled-sqlite` must enable `rusqlite/bundled` or it is a name that does nothing. \
         Found: {feature}"
    );
}

/// The feature is not only declared — the SQLite that linked is the one rusqlite bundles.
///
/// A mistyped feature path is the failure this catches: the manifest reads correctly, the build is
/// green, and the binary quietly linked whatever the machine happened to have. rusqlite 0.41
/// bundles SQLite 3.51; a system copy on a machine this old is older than that, and on a machine
/// that is newer this floor still holds.
#[test]
#[cfg(feature = "bundled-sqlite")]
fn the_bundled_sqlite_is_what_linked() {
    let linked = rusqlite::version_number();
    assert!(
        linked >= 3_051_000,
        "`bundled-sqlite` is on, so the SQLite compiled in should be rusqlite 0.41's own \
         (>= 3.51.0); linked {} ({linked})",
        rusqlite::version()
    );
}
