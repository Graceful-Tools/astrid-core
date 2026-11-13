//! Narrow `contracts/fixtures/smart.json` to the part the parser actually reads.
//!
//! `parse::smart` needs the `keywords` tables — twelve languages of "tomorrow", about 9 KB. It used
//! to `include_str!` the whole fixture to get at them, so the other 75 KB travelled in the
//! `.rodata` of every binary on every platform: the fixture's 220 `cases`, its provenance strings,
//! and the pretty-printing. Nothing in any client can reach them. That was astrid-windows task
//! `4d05306d`, where it mattered most because `astrid-rules-wasm` is downloaded by a browser.
//!
//! Extracting here rather than committing a second fixture keeps `smart.json` the single source:
//! there is no derived file to drift from it, and `contracts/fixtures/` stays what
//! `cargo xtask check-contracts` generates from astrid-web. The `cases` are still the spec — they
//! are read from disk by `tests/smart_contract.rs`, which is where a test's inputs belong.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    let fixture = manifest.join("../../contracts/fixtures/smart.json");

    // Re-run when the fixture is regenerated from web, or when this extraction changes.
    println!("cargo:rerun-if-changed={}", fixture.display());
    println!("cargo:rerun-if-changed=build.rs");

    let source = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("reading {}: {error}", fixture.display()));

    let whole: serde_json::Value = serde_json::from_str(&source)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", fixture.display()));

    let keywords = whole
        .get("keywords")
        .unwrap_or_else(|| panic!("{} has no `keywords` object", fixture.display()));

    // Minified: the parser reads this through serde, and the whitespace was a third of the old
    // embed. `to_string` on a `Value` is stable enough for a build artefact — the tables are
    // checked by the contract fixture, not by the bytes of this file.
    let narrowed = serde_json::to_string(keywords).expect("a JSON object re-serialises");

    assert!(
        !narrowed.is_empty() && narrowed.starts_with('{'),
        "the extracted keywords are not a JSON object"
    );

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("smart_keywords.json");
    std::fs::write(&out, narrowed)
        .unwrap_or_else(|error| panic!("writing {}: {error}", out.display()));
}
