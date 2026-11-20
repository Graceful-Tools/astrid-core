//! Repo automation, run as `cargo xtask <command>`.
//!
//! Checks live here rather than in shell scripts so they run identically on a developer machine and
//! on CI, and so a contributor can discover them with `cargo xtask`.

use std::process::ExitCode;

mod assets;
mod brand;
mod core_contracts;
mod deny;
mod design;
mod icons;
mod schema;
mod strings;

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
        "follow-pins",
        "move CONSUMERS.md rows that are behind their consumer's pin on main [consumer checkout paths]",
    ),
    (
        "gen-deny",
        "write each consumer's deny.toml into supply-chain/ from this repo's own",
    ),
    (
        "check-deny",
        "verify supply-chain/deny.<repo>.toml is what deny.toml renders",
    ),
    (
        "gen-brand",
        "write every client's brand resources from one profile [profile] --out <dir>",
    ),
    (
        "gen-design",
        "write design/generated from design/tokens.json and design/assets",
    ),
    (
        "strings-keys",
        "print every string-catalog key this core emits, one per line",
    ),
    (
        "gen-strings",
        "write strings/generated from strings/<locale>.json for all four platforms",
    ),
    (
        "check-strings",
        "verify the catalog covers every key this core emits, and that strings/generated is current",
    ),
    (
        "check-design",
        "verify design/generated is current, and every client's vendored copy [checkouts, default ../<client>]",
    ),
    (
        "gen-schema",
        "write generated/schema from the types that are the wire — Command, Rule and the models",
    ),
    (
        "check-schema",
        "verify generated/schema is what this core's types produce",
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
        "follow-pins" => follow_pins(),
        "gen-core-contracts" => core_contracts::write_all(&repo_root()),
        "check-core-contracts" => core_contracts::check_all(&repo_root()),
        "gen-brand" => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| args.get(i + 1))
                .cloned();
            let profile = args.first().filter(|a| *a != "--out").cloned();
            brand::generate(&repo_root(), profile, out)
        }
        "gen-deny" => deny::generate(&repo_root()),
        "check-deny" => deny::check(&repo_root()),
        "gen-design" => design::generate(&repo_root()),
        "strings-keys" => {
            for key in astrid_rules::copy::keys() {
                println!("{key}");
            }
            ExitCode::SUCCESS
        }
        "gen-strings" => strings::generate(&repo_root()),
        "check-strings" => strings::check(&repo_root()),
        "gen-schema" => schema::generate(&repo_root()),
        "check-schema" => schema::check(&repo_root()),
        "check-design" => design::check(
            &repo_root(),
            std::env::args()
                .skip(2)
                .map(std::path::PathBuf::from)
                .collect(),
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

/// The copies of this core in the world, by the key their `CONSUMERS.md` row carries. A tenth that
/// is not listed here fails the check rather than going unrecorded, which is the whole point: the
/// November 2026 platform assessment read the Windows pin as 96 commits behind off a manifest that
/// had already been bumped three times, because nothing anywhere said what the live revisions were.
///
/// A key is `<checkout>/<copy>` — the directory the consumer is checked out into, and which copy of
/// this core it holds. **Three repositories hold two copies each**: astrid-web, astrid-windows and
/// astrid-ios each vendor generated design tokens as well as taking the rules or the whole core. So
/// this stopped being a list of repositories with task 97285677; the cross-client review of
/// 2026-11-11 found four copies that no row and no const could have described.
///
/// And it is now only "which rows must EXIST". Where a consumer's pin is written, and at which
/// revision, is read out of the table itself — so recording a tenth copy is a row in `CONSUMERS.md`
/// and nothing in here. What the const still buys is the other half of what task 3c1c9092 built: a
/// row cannot be deleted by a bad merge and go unnoticed.
const CONSUMERS: &[&str] = &[
    "astrid-web/astrid-rules",
    "astrid-web/astrid-design",
    "astrid-web/strings",
    "astrid-windows/core",
    "astrid-windows/design",
    "astrid-ios/core",
    "astrid-ios/design",
    "astrid-android/core",
    "astrid-vscode/astrid-rules",
    "astrid-code/astrid-rules",
];

/// One row of `CONSUMERS.md`'s table, parsed.
struct Consumer {
    /// The row's first cell: `<checkout>/<copy>`, and one of [`CONSUMERS`].
    key: String,
    /// The *Pinned in* cell's backticked paths, relative to the consumer's own checkout, in the
    /// order to try. The first that exists is the one read.
    manifests: Vec<String>,
    /// The last cell: a full forty-character sha.
    revision: String,
}

impl Consumer {
    /// The checkout directory this copy lives in — the key up to its first `/`.
    fn repo(&self) -> &str {
        self.key.split('/').next().unwrap_or(&self.key)
    }

    /// Whether this copy **builds** the core rather than vendoring generated output from it.
    ///
    /// A Cargo manifest is the tell, and it is the only consumer kind for which this core's
    /// `rust-toolchain.toml` is any of its business: a repository that vendors a `REVISION` and some
    /// generated JSON compiles no Rust of ours, so its toolchain is not reported.
    fn builds_the_core(&self) -> bool {
        self.manifests
            .iter()
            .any(|path| path.ends_with("Cargo.toml"))
    }
}

/// A pin may only move forward (ACTD-2): each row's revision must descend from the revision the
/// same row had in `base` (the table as `main` has it). Git already rejects two agents bumping one
/// pin at once; what nothing caught was an agent whose core checkout was stale pinning a
/// consumer to a commit that predates one already landed *and* pinned — the consumer quietly loses
/// it while every repository looks healthy.
///
/// A deliberate rollback is allowed when the table carries a marker naming the row —
/// `<!-- pin-rollback: <key> — <why> -->` — so backing out a regression is a reviewed decision in
/// the file, not an accident, and CI needs no flag. Returns the failures, and a note per accepted
/// rollback. Rows absent from either table are not this check's concern (the row checks own that).
fn pins_moved_backwards(
    base: &str,
    table: &str,
    keys: &[&str],
    descends: impl Fn(&str, &str) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut failed = Vec::new();
    let mut rolled_back = Vec::new();
    for key in keys {
        let (Some(before), Some(after)) = (consumer_row(base, key), consumer_row(table, key))
        else {
            continue;
        };
        if before.revision == after.revision || descends(&before.revision, &after.revision) {
            continue;
        }
        if rollback_marked(table, key) {
            rolled_back.push(format!(
                "{key} moved back from {} to {} — a deliberate rollback (pin-rollback marker).",
                before.revision, after.revision
            ));
        } else {
            failed.push(format!(
                "{key} moved from {} to {}, which does not descend from it: the consumer would lose \
                 what the old pin had. A pin only moves forward; to back out a regression on purpose, \
                 add `<!-- pin-rollback: {key} — <why> -->` to CONSUMERS.md.",
                before.revision, after.revision
            ));
        }
    }
    (failed, rolled_back)
}

/// Whether `table` carries a `<!-- pin-rollback: <key> ... -->` marker for `key`.
fn rollback_marked(table: &str, key: &str) -> bool {
    table.lines().any(|line| {
        let line = line.trim();
        line.starts_with("<!--")
            && line
                .trim_start_matches("<!--")
                .trim()
                .strip_prefix("pin-rollback:")
                .is_some_and(|rest| {
                    let rest = rest.trim();
                    rest == key
                        || rest.starts_with(&format!("{key} "))
                        || rest.starts_with(&format!("{key}\u{2014}"))
                })
    })
}

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
/// 3. Does each consumer's own manifest agree with its row — and if not, **who moved?** A consumer
///    that bumped to a newer commit of `main` (the row is an ancestor of the manifest's pin) has
///    left the row *behind its consumer*: that is reported, and `cargo xtask follow-pins` (run
///    daily by the `consumers-follow-pins` workflow) moves the row. It is not a failure, because
///    the client did nothing wrong that this repository can stop, and on 2026-11-15 failing it
///    turned core `main` red three times in a day (task ADEV-7). Every other disagreement still
///    fails: the manifest pins a short sha, a commit that is not here, a commit that is not on
///    `main`, or one *older* than the row (the row is ahead of its consumer). A consumer that is
///    not here is reported *unchecked*, never failed. CI has only the public astrid-web checkout
///    unless `CONSUMER_PINS_TOKEN` is set, and a fork's pull request has no secrets at all (task
///    4c891314).
fn check_pins() -> ExitCode {
    let root = repo_root();
    let Some((table, main)) = pins_inputs(&root) else {
        return ExitCode::FAILURE;
    };
    let checkouts = consumer_checkouts(&root);
    let mut report = evaluate_pins(&root, main, &table, CONSUMERS, &checkouts);
    // ACTD-2: against the table as main has it, no row may move to a revision that does not
    // descend from its old one. On main itself the two tables are the same, and this is silent.
    let mut rollbacks = Vec::new();
    if let Some(base) = git(&root, &["show", &format!("{main}:CONSUMERS.md")]) {
        let (failed, accepted) = pins_moved_backwards(&base, &table, CONSUMERS, |old, new| {
            git(&root, &["merge-base", "--is-ancestor", old, new]).is_some()
        });
        report.failed.extend(failed);
        rollbacks = accepted;
    }

    println!("Consumer pins, against {main}:\n");
    for line in &report.lines {
        println!("{line}");
    }

    let mut unchecked: Vec<String> = report
        .behind
        .iter()
        .map(|behind| {
            format!(
                "{} pins {} in {}, a newer commit of {main} than CONSUMERS.md's {}. The row is \
                 behind its consumer: `cargo xtask follow-pins` moves it, and the \
                 consumers-follow-pins workflow does that daily.",
                behind.key,
                behind.found,
                behind.path.display(),
                behind.row
            )
        })
        .collect();
    unchecked.extend(report.unchecked);

    if !unchecked.is_empty() {
        println!("\nReported, not failed:");
        for row in &unchecked {
            println!("  {row}");
        }
        println!("\nNone of these is a failure. CI holds the five private checkouts only when");
        println!("CONSUMER_PINS_TOKEN is set, a fork's pull request has no secrets at all, and a");
        println!("checkout found beside this one is a working copy that may just be behind. A");
        println!("row behind its consumer is a client that bumped along main before the table");
        println!("did. A consumer's toolchain is another repository's file, reported and no more.");
    }

    for rollback in &rollbacks {
        println!("\nROLLBACK  {rollback}");
    }

    if report.failed.is_empty() {
        println!("\nOK");
        return ExitCode::SUCCESS;
    }
    eprintln!();
    for problem in &report.failed {
        eprintln!("FAIL  {problem}");
    }
    eprintln!("\nThe pins table is out of date. Fix the row, or fix the pin — CONSUMERS.md says");
    eprintln!("which, and how a bump is supposed to go.");
    ExitCode::FAILURE
}

/// Moves every row that is behind its consumer up to the consumer's pin, and writes `CONSUMERS.md`.
///
/// This is the other half of ADEV-7's report-only verdict: `check-pins` no longer fails a client
/// that bumped along `main` before the table did, and this is what catches the table up, so no
/// human edits it. Only rows `check-pins` would call *behind* move — the manifest names a full sha
/// that is on `main` and descends from the row — and only against a checkout **named on the
/// command line**: a sibling found beside this repository may hold a bump nobody pushed. Every other
/// disagreement is left for `check-pins` to fail. Running it twice changes nothing the second time.
fn follow_pins() -> ExitCode {
    let root = repo_root();
    let Some((table, main)) = pins_inputs(&root) else {
        return ExitCode::FAILURE;
    };
    let checkouts = consumer_checkouts(&root);
    let (followed, moved) = follow_table(&root, main, &table, CONSUMERS, &checkouts);

    if moved.is_empty() {
        println!(
            "Every row is level with its consumer's pin, or cannot be followed. Nothing to do."
        );
        return ExitCode::SUCCESS;
    }
    let table_path = root.join("CONSUMERS.md");
    if let Err(err) = std::fs::write(&table_path, followed) {
        eprintln!("cannot write {}: {err}", table_path.display());
        return ExitCode::FAILURE;
    }
    println!("Rows moved to their consumer's pin (all on {main}):");
    for behind in &moved {
        println!("- `{}`: {} -> {}", behind.key, behind.row, behind.found);
    }
    ExitCode::SUCCESS
}

/// `CONSUMERS.md` and the ref ancestry is asked against, or `None` with the reason printed.
fn pins_inputs(root: &std::path::Path) -> Option<(String, &'static str)> {
    let table_path = root.join("CONSUMERS.md");
    let table = match std::fs::read_to_string(&table_path) {
        Ok(table) => table,
        Err(err) => {
            eprintln!("cannot read {}: {err}", table_path.display());
            return None;
        }
    };

    // `origin/main` FIRST, and the local branch only as a fallback. The remote is the shared
    // truth; a developer's local `main` is routinely a few commits behind, and preferring it
    // reported a correctly-pinned consumer as sitting on an unmerged branch — which is this
    // check's loudest failure and would have sent someone hunting a pin that was fine. On a pull
    // request CI checks out a detached head, where `main` exists only as `origin/main` anyway.
    let Some(main) = ["origin/main", "main"]
        .into_iter()
        .find(|reference| git(root, &["rev-parse", "--verify", reference]).is_some())
    else {
        eprintln!("neither main nor origin/main is in this checkout, so ancestry cannot be asked.");
        eprintln!("CI needs `fetch-depth: 0` — a shallow clone cannot answer it, and a wrong");
        eprintln!("answer would fail every pin at once.");
        return None;
    };
    Some((table, main))
}

/// What `check-pins` found, before it is printed.
#[derive(Default)]
struct PinsReport {
    /// One line per row: the revision, its age, and what its manifest said.
    lines: Vec<String>,
    /// Problems that fail the check.
    failed: Vec<String>,
    /// Rows whose consumer moved forward along `main` before the table did (ADEV-7).
    behind: Vec<RowBehind>,
    /// Everything else reported and not failed: unread manifests, stale siblings, toolchains.
    unchecked: Vec<String>,
}

/// A row behind its consumer: the manifest pins a newer commit of `main` than the row says.
struct RowBehind {
    key: String,
    /// The row's revision.
    row: String,
    /// The manifest's revision, a full sha that descends from `row` and is on `main`.
    found: String,
    path: std::path::PathBuf,
    /// Whether the checkout was named on the command line; only those are followed.
    named: bool,
}

/// Every row of `table` named in `keys`, asked the three questions [`check_pins`] documents.
fn evaluate_pins(
    root: &std::path::Path,
    main: &str,
    table: &str,
    keys: &[&str],
    checkouts: &std::collections::HashMap<String, Checkout>,
) -> PinsReport {
    let mut report = PinsReport::default();
    // One line per repository, not per row: astrid-ios holds two copies and has one toolchain.
    let mut toolchains = std::collections::BTreeMap::new();

    for key in keys {
        let Some(consumer) = consumer_row(table, key) else {
            report.failed.push(format!(
                "{key} has no row in CONSUMERS.md with a revision in its last column."
            ));
            continue;
        };
        let revision = consumer.revision.clone();

        if let Err(why) = on_main(root, main, &revision) {
            report
                .failed
                .push(format!("{key} is pinned at '{revision}', which {why}."));
            continue;
        }

        let behind = git(
            root,
            &["rev-list", "--count", &format!("{revision}..{main}")],
        )
        .unwrap_or_default();
        let dated = git(root, &["log", "-1", "--format=%cs", &revision]).unwrap_or_default();
        // Commits AND days, because they answer different questions. Twenty commits behind is
        // nothing if they landed this morning, and two commits behind is a client nobody has
        // touched since August — the cross-client review had to work the second one out by hand
        // for every row (task 97285677).
        let lag = match age(root, &revision, main) {
            Some(days) => format!("{behind:>3} behind, {days:>3}d"),
            None => format!("{behind:>3} behind"),
        };
        let shape = format!("  {key:<27} {} {dated}  {lag}", &revision[..7]);

        let fallback = Checkout {
            path: root.join("..").join(consumer.repo()),
            named: false,
        };
        let checkout = checkouts.get(consumer.repo()).unwrap_or(&fallback);
        if consumer.builds_the_core() && checkout.path.is_dir() {
            if let Some(found) = toolchain_report(root, &checkout.path, consumer.repo()) {
                toolchains.insert(consumer.repo().to_string(), found);
            }
        }
        match read_manifest(&consumer, &checkout.path) {
            Manifest::Agrees(path) => report
                .lines
                .push(format!("{shape}  compared against {}", path.display())),
            Manifest::Disagrees { path, found } => {
                if let Err(why) = consumer_moved_forward(root, main, &revision, &found) {
                    let problem = format!(
                        "{key} pins {found} in {} but CONSUMERS.md says {revision}, and the pin {why}.",
                        path.display()
                    );
                    if checkout.named {
                        report.lines.push(format!("{shape}  MISMATCH"));
                        report.failed.push(problem);
                    } else {
                        report.lines.push(format!(
                            "{shape}  differs from the checkout beside this one"
                        ));
                        report.unchecked.push(format!("{problem} That checkout was not named on the command line, so it may simply need a pull."));
                    }
                } else {
                    report.lines.push(format!(
                        "{shape}  ROW BEHIND its consumer, which pins {}",
                        &found[..7]
                    ));
                    report.behind.push(RowBehind {
                        key: key.to_string(),
                        row: revision,
                        found,
                        path,
                        named: checkout.named,
                    });
                }
            }
            Manifest::Unread(why) => {
                report.lines.push(format!("{shape}  UNCHECKED"));
                report.unchecked.push(format!("{key}: {why}"));
            }
        }
    }

    report.unchecked.extend(toolchains.into_values());
    report
}

/// `Ok` when `revision` is a full sha naming a commit here that is an ancestor of `main`; otherwise
/// why not, phrased to follow "which".
fn on_main(root: &std::path::Path, main: &str, revision: &str) -> Result<(), String> {
    if !is_full_sha(revision) {
        return Err("is not a full forty-character sha".to_string());
    }
    if git(root, &["cat-file", "-e", &format!("{revision}^{{commit}}")]).is_none() {
        return Err("is not a commit in this repository".to_string());
    }
    if git(root, &["merge-base", "--is-ancestor", revision, main]).is_none() {
        return Err(format!(
            "is NOT an ancestor of {main} — a branch nobody merged"
        ));
    }
    Ok(())
}

/// `Ok` when a consumer whose manifest pins `found` has simply moved forward along `main` from the
/// row's `row` — the one disagreement that is the table's to catch up with rather than a failure
/// (ADEV-7). Otherwise why not, phrased to follow "the pin".
fn consumer_moved_forward(
    root: &std::path::Path,
    main: &str,
    row: &str,
    found: &str,
) -> Result<(), String> {
    on_main(root, main, found)?;
    if git(root, &["merge-base", "--is-ancestor", found, row]).is_some() {
        return Err("is OLDER than the row — the row is ahead of its consumer".to_string());
    }
    if git(root, &["merge-base", "--is-ancestor", row, found]).is_none() {
        return Err("does not descend from the row".to_string());
    }
    Ok(())
}

/// `table` with every followable row moved to its consumer's pin, and the rows that moved.
fn follow_table(
    root: &std::path::Path,
    main: &str,
    table: &str,
    keys: &[&str],
    checkouts: &std::collections::HashMap<String, Checkout>,
) -> (String, Vec<RowBehind>) {
    let report = evaluate_pins(root, main, table, keys, checkouts);
    let mut followed = table.to_string();
    let mut moved = Vec::new();
    for behind in report.behind.into_iter().filter(|behind| behind.named) {
        if let Some(next) = with_revision(&followed, &behind.key, &behind.found) {
            followed = next;
            moved.push(behind);
        }
    }
    (followed, moved)
}

/// `table` with the last cell of `key`'s body row set to `revision`, or `None` if there is no such
/// row. Every other byte, line endings included, is left alone.
fn with_revision(table: &str, key: &str, revision: &str) -> Option<String> {
    let mut out = String::with_capacity(table.len());
    let mut in_body = false;
    let mut done = false;
    for line in table.split_inclusive('\n') {
        let row = line.trim_end_matches(['\n', '\r']);
        if !in_body {
            in_body = is_table_separator(row);
        } else if !done && row.starts_with('|') {
            let cells = row_cells(row);
            if cells
                .first()
                .is_some_and(|first| unbackticked(first) == key)
            {
                let last = cells.last()?;
                let at = row.rfind(last.as_str())?;
                out.push_str(&row[..at]);
                out.push('`');
                out.push_str(revision);
                out.push('`');
                out.push_str(&line[at + last.len()..]);
                done = true;
                continue;
            }
        }
        out.push_str(line);
    }
    done.then_some(out)
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
    /// `CONSUMER_PINS_TOKEN` cannot clone the five private repositories, and neither can a fork's
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
fn read_manifest(consumer: &Consumer, checkout: &std::path::Path) -> Manifest {
    let candidates = &consumer.manifests;
    if candidates.is_empty() {
        return Manifest::Unread(format!(
            "{}'s row names no manifest path in its 'Pinned in' cell, so there is nothing to \
             compare it against",
            consumer.key
        ));
    }

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
        Some(found) if found == consumer.revision => Manifest::Agrees(path),
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
fn consumer_checkouts(root: &std::path::Path) -> std::collections::HashMap<String, Checkout> {
    let mut checkouts: std::collections::HashMap<String, Checkout> = repositories()
        .map(|repo| {
            (
                repo.clone(),
                Checkout {
                    path: root.join("..").join(&repo),
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
            .map(str::to_string)
            .filter(|name| repositories().any(|repo| &repo == name));
        match named {
            // One path can serve two rows: astrid-web holds both the rules wasm and the design
            // tokens, so a named checkout is keyed by its repository and found by both.
            Some(repo) => {
                checkouts.insert(repo, Checkout { path, named: true });
            }
            // Silently ignoring this is how a typo stops a row being compared while the check
            // still says OK — the exact shape of the problem this check exists to catch.
            None => eprintln!(
                "warning: '{argument}' does not name a consumer, so it was not used. The directory\n\
                 must be named for the repository it holds (one of {}).",
                repositories().collect::<Vec<_>>().join(", ")
            ),
        }
    }

    checkouts
}

/// The distinct checkout directories [`CONSUMERS`] names, in order and without repeats — six
/// repositories holding nine copies.
fn repositories() -> impl Iterator<Item = String> {
    let mut seen: Vec<String> = Vec::new();
    for key in CONSUMERS {
        let repo = key.split('/').next().unwrap_or(key).to_string();
        if !seen.contains(&repo) {
            seen.push(repo);
        }
    }
    seen.into_iter()
}

/// How a consumer's Rust toolchain compares with the one this core pins, when it differs.
///
/// `None` when they agree, or when the consumer's checkout says nothing — a repository that has no
/// `rust-toolchain` file is choosing the installed default, which is the same fact as `stable` and
/// is reported as such.
///
/// **Reported, never failed.** It is a statement about another repository's file, and this check has
/// no business turning the core's gate red over it. The cross-client review of 2026-11-11 found all
/// three native consumers floating `stable` while this core pins an exact version, which is how a
/// client silently builds the core on a toolchain nobody tested (task 97285677).
fn toolchain_report(
    root: &std::path::Path,
    checkout: &std::path::Path,
    repo: &str,
) -> Option<String> {
    let ours = toolchain_channel(root)?;
    match toolchain_channel(checkout) {
        Some(theirs) if theirs == ours => None,
        Some(theirs) => Some(format!(
            "{repo} builds this core on Rust '{theirs}' while rust-toolchain.toml pins '{ours}'. \
             An older or floating toolchain is unsupported, and 'stable' means whatever the machine \
             happened to install."
        )),
        None => Some(format!(
            "{repo} builds this core with no rust-toolchain file, so it uses whatever Rust the \
             machine has, while rust-toolchain.toml pins '{ours}'."
        )),
    }
}

/// The channel a checkout's toolchain file names: `channel = "1.98.1"` in a `rust-toolchain.toml`,
/// or the bare version in the legacy one-line `rust-toolchain`.
fn toolchain_channel(checkout: &std::path::Path) -> Option<String> {
    let (path, contents) = ["rust-toolchain.toml", "rust-toolchain"]
        .into_iter()
        .map(|name| checkout.join(name))
        .find(|path| path.is_file())
        .and_then(|path| std::fs::read_to_string(&path).ok().map(|read| (path, read)))?;

    if path
        .extension()
        .is_some_and(|extension| extension == "toml")
    {
        return contents
            .lines()
            .map(|line| line.split('#').next().unwrap_or_default().trim())
            .filter_map(|line| line.strip_prefix("channel"))
            .filter_map(|rest| rest.trim_start().strip_prefix('='))
            .filter_map(|rest| rest.trim_start().strip_prefix('"'))
            .filter_map(|rest| rest.split('"').next())
            .map(str::to_string)
            .next();
    }
    let channel = contents.trim();
    (!channel.is_empty()).then(|| channel.to_string())
}

/// How many whole days of history sit between a pin and `main`, by committer timestamp.
fn age(root: &std::path::Path, revision: &str, main: &str) -> Option<i64> {
    let committed = |reference: &str| {
        git(root, &["log", "-1", "--format=%ct", reference])?
            .parse::<i64>()
            .ok()
    };
    Some(days_between(committed(revision)?, committed(main)?))
}

/// Whole days between two unix timestamps, never negative.
///
/// A pin can be *newer* than the tip when `main` is a local branch somebody has not pushed, and a
/// negative age printed beside "0 behind" reads like a bug in the check rather than a stale
/// checkout.
fn days_between(earlier: i64, later: i64) -> i64 {
    ((later - earlier) / 86_400).max(0)
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
    if path
        .extension()
        .is_some_and(|extension| extension == "swift")
    {
        return generated_header_revision(contents);
    }
    let revision = contents.trim();
    (!revision.is_empty()).then(|| revision.to_string())
}

/// The revision a generated file's header names, if it names one.
///
/// `gen-design` writes two header lines — what generated the file, and a `body-sha256` of its
/// contents. **A content hash is not a revision**: it answers "has this copy been edited?", not
/// "which commit of this core was it generated from", so a consumer whose only record is a
/// generated header cannot have its pin compared and is reported *unchecked*. Reading it anyway
/// would compare a sha256 against a git sha and report a mismatch nobody could act on.
///
/// `astrid-core-revision:` is read if it is there, so the day the generator starts writing it this
/// check needs no change (task 97285677).
fn generated_header_revision(contents: &str) -> Option<String> {
    contents
        .lines()
        .take_while(|line| line.trim_start().starts_with("//"))
        .filter_map(|line| line.split_once("astrid-core-revision:"))
        .map(|(_, revision)| revision.trim().to_string())
        .find(|revision| !revision.is_empty())
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

/// A consumer read out of `CONSUMERS.md`'s table, by the key in its first cell.
///
/// The revision comes from the LAST cell and the manifest paths from the one before it, so the
/// prose in the middle cells is free to change without touching this parser, and the row is found
/// by name so reordering the table is harmless.
///
/// The key is matched **exactly**, bar backticks and whitespace. It used to be a substring match,
/// which was safe while the keys were four repository names; it is not safe now that
/// `astrid-ios/core` and `astrid-ios/design` are different rows, and a substring match would have
/// handed the first one's sha to the second (task 97285677).
fn consumer_row(table: &str, key: &str) -> Option<Consumer> {
    // Body rows only: everything up to and including the `|---|---|` separator is the header, and
    // a lookup for a name that appears in the header ("Revision") otherwise comes back as a sha.
    let cells = table
        .lines()
        .skip_while(|line| !is_table_separator(line))
        .skip(1)
        .filter(|line| line.starts_with('|'))
        .map(row_cells)
        .find(|cells| {
            cells
                .first()
                .is_some_and(|first| unbackticked(first) == key)
        })?;

    let revision = unbackticked(cells.last()?);
    if revision.is_empty() {
        return None;
    }
    let manifests = cells
        .len()
        .checked_sub(2)
        .and_then(|before_last| cells.get(before_last))
        .map(|cell| manifest_paths(cell))
        .unwrap_or_default();

    Some(Consumer {
        key: key.to_string(),
        manifests,
        revision,
    })
}

/// A markdown table row's cells, without the empties the leading and trailing pipes produce.
fn row_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn unbackticked(cell: &str) -> String {
    cell.trim().trim_matches('`').trim().to_string()
}

/// The candidate manifest paths in a row's *Pinned in* cell: every backticked span with no
/// whitespace in it.
///
/// The cell is prose as well as paths — astrid-windows's reads ``crates/astrid-ffi/Cargo.toml``
/// (`rev =`) — and the space in `rev =` is what keeps it out. Reading the paths from the table
/// rather than from a const is the point of task 97285677: a tenth copy of this core is then a row
/// somebody wrote down, not a row plus an array entry that has to agree with it.
///
/// More than one path is allowed, because a repository is free to move its wrapper crate and a path
/// that has moved should report the row *unchecked* rather than fail a pin that is perfectly good.
fn manifest_paths(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| !span.is_empty() && !span.contains(char::is_whitespace))
        .map(str::to_string)
        .collect()
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
        consumer_row, days_between, is_full_sha, manifest_paths, manifest_revision,
        pins_moved_backwards, read_manifest, repositories, same_fixture, toolchain_channel,
        Manifest, CONSUMERS,
    };

    /// A pin only moves forward; backwards is a failure unless the table marks it (ACTD-2).
    #[test]
    fn a_pin_that_moves_backwards_fails_unless_marked_actd_2() {
        const OLD: &str = "3677cfb631d6fe415db5a77403e2c79f4e0b8c2b";
        const NEWER: &str = "1111111111111111111111111111111111111111";
        const OLDER: &str = "0000000000000000000000000000000000000000";
        // OLD's descendants are NEWER; OLDER predates it.
        let descends = |old: &str, new: &str| old == OLD && new == NEWER;
        let keys = ["astrid-ios/core"];

        let forward = TABLE.replace(OLD, NEWER);
        assert_eq!(
            pins_moved_backwards(TABLE, &forward, &keys, descends),
            (vec![], vec![])
        );
        assert_eq!(
            pins_moved_backwards(TABLE, TABLE, &keys, descends),
            (vec![], vec![])
        );

        let backward = TABLE.replace(OLD, OLDER);
        let (failed, rolled) = pins_moved_backwards(TABLE, &backward, &keys, descends);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].contains("astrid-ios/core") && failed[0].contains("pin-rollback"));
        assert!(rolled.is_empty());

        let marked = format!("{backward}\n<!-- pin-rollback: astrid-ios/core \u{2014} backing out 3677cfb's crash -->\n");
        let (failed, rolled) = pins_moved_backwards(TABLE, &marked, &keys, descends);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(rolled.len(), 1);

        // A marker for another row does not excuse this one.
        let other =
            format!("{backward}\n<!-- pin-rollback: astrid-ios/design \u{2014} not this one -->\n");
        assert_eq!(
            pins_moved_backwards(TABLE, &other, &keys, descends).0.len(),
            1
        );
    }

    const TABLE: &str = "\
| Consumer | Repository | Takes | Pinned in | Revision |
|---|---|---|---|---|
| `astrid-web/astrid-rules` | [astrid-web](https://example.test) | `astrid-rules`, as WebAssembly | `packages/astrid-rules/REVISION` | `52a371d21781f59dd4814d68794f533df85587ae` |
| `astrid-windows/core` | [astrid-windows](https://example.test) | the whole core | `crates/astrid-ffi/Cargo.toml` (`rev =`) | `e5f4e45efdf8b9064adfd534ced2e9d9b0181517` |
| `astrid-ios/core` | [astrid-ios](https://example.test) | the whole core | `core/Cargo.toml` or `core/astrid-apple/Cargo.toml` | `3677cfb631d6fe415db5a77403e2c79f4e0b8c2b` |
| `astrid-ios/design` | [astrid-ios](https://example.test) | the generated tokens | nothing records it | `a503a62039fca9a19ce7802a2d9f6d38ae5f89d8` |
";

    /// Task 3c1c9092. The row's middle cells contain backticks and pipe-free prose of their own,
    /// including a `rev =` that looks like the thing being parsed — so the revision has to come
    /// from the row's LAST cell rather than from its first backticked span.
    #[test]
    fn a_revision_is_read_from_the_last_cell_3c1c9092() {
        assert_eq!(
            consumer_row(TABLE, "astrid-windows/core").unwrap().revision,
            "e5f4e45efdf8b9064adfd534ced2e9d9b0181517"
        );
        assert_eq!(
            consumer_row(TABLE, "astrid-web/astrid-rules")
                .unwrap()
                .revision,
            "52a371d21781f59dd4814d68794f533df85587ae"
        );
    }

    /// A consumer with no row of its own must answer `None` and fail the check, not silently
    /// inherit a neighbour's sha. The header row must not match either.
    #[test]
    fn a_consumer_with_no_row_is_not_found_3c1c9092() {
        assert!(consumer_row(TABLE, "astrid-android/core").is_none());
        assert!(consumer_row(TABLE, "Consumer").is_none());
        assert!(consumer_row(TABLE, "Revision").is_none());
    }

    /// Task 97285677. The key is matched EXACTLY. It was a substring match while the keys were four
    /// repository names, and a substring match now would hand `astrid-ios/core`'s sha to
    /// `astrid-ios/design` — a row that would then be compared against the wrong revision while
    /// looking perfectly checked.
    #[test]
    fn a_key_is_matched_exactly_not_by_substring_97285677() {
        assert_eq!(
            consumer_row(TABLE, "astrid-ios/design").unwrap().revision,
            "a503a62039fca9a19ce7802a2d9f6d38ae5f89d8"
        );
        assert_eq!(
            consumer_row(TABLE, "astrid-ios/core").unwrap().revision,
            "3677cfb631d6fe415db5a77403e2c79f4e0b8c2b"
        );
        // Neither the repository alone nor a prefix of a key is a row.
        assert!(consumer_row(TABLE, "astrid-ios").is_none());
        assert!(consumer_row(TABLE, "astrid-web").is_none());
    }

    /// The row carries its own manifest paths, so `check-pins` holds no list of them. `rev =` is
    /// backticked in the same cell and is not a path — the whitespace in it is what keeps it out.
    #[test]
    fn the_manifest_paths_come_from_the_row_97285677() {
        assert_eq!(
            consumer_row(TABLE, "astrid-windows/core")
                .unwrap()
                .manifests,
            vec!["crates/astrid-ffi/Cargo.toml".to_string()]
        );
        assert_eq!(
            consumer_row(TABLE, "astrid-ios/core").unwrap().manifests,
            vec![
                "core/Cargo.toml".to_string(),
                "core/astrid-apple/Cargo.toml".to_string()
            ]
        );
        assert_eq!(
            manifest_paths("`crates/astrid-ffi/Cargo.toml` (`rev =`)"),
            vec!["crates/astrid-ffi/Cargo.toml".to_string()]
        );
    }

    /// A row whose *Pinned in* cell names no path reports *unchecked* with that said, rather than
    /// going quietly uncompared. `astrid-ios/design` is that row today: `AstridTokens.swift` records
    /// a `body-sha256`, which is not a revision.
    #[test]
    fn a_row_with_no_manifest_path_is_unchecked_97285677() {
        let consumer = consumer_row(TABLE, "astrid-ios/design").unwrap();
        assert!(consumer.manifests.is_empty());
        assert!(!consumer.builds_the_core());

        match read_manifest(&consumer, std::path::Path::new(".")) {
            Manifest::Unread(why) => assert!(
                why.contains("names no manifest path"),
                "it should say what is missing, said: {why}"
            ),
            Manifest::Agrees(_) | Manifest::Disagrees { .. } => {
                panic!("a row with no path cannot have been compared")
            }
        }
    }

    /// Only a consumer that COMPILES this core cares which toolchain it pins. A copy that vendors a
    /// `REVISION` and some generated JSON builds no Rust of ours.
    #[test]
    fn only_a_cargo_consumer_builds_the_core_97285677() {
        assert!(consumer_row(TABLE, "astrid-windows/core")
            .unwrap()
            .builds_the_core());
        assert!(!consumer_row(TABLE, "astrid-web/astrid-rules")
            .unwrap()
            .builds_the_core());
    }

    /// Ten copies, six repositories, in order and without repeats — so naming `../astrid-web` once
    /// serves both of that repository's rows.
    #[test]
    fn ten_copies_live_in_six_repositories_97285677() {
        let repos: Vec<String> = repositories().collect();
        assert_eq!(
            repos,
            vec![
                "astrid-web",
                "astrid-windows",
                "astrid-ios",
                "astrid-android",
                "astrid-vscode",
                "astrid-code"
            ]
        );
        assert_eq!(CONSUMERS.len(), 10);
    }

    /// The lookup is exact, but a key that CONTAINED another would still be a trap for anyone
    /// reading the table, and for any future loosening of the match. None does.
    #[test]
    fn no_key_contains_another_97285677() {
        for key in CONSUMERS {
            for other in CONSUMERS {
                assert!(
                    key == other || !key.contains(other),
                    "'{key}' contains '{other}'"
                );
            }
        }
    }

    /// A generated header's `body-sha256` is a content hash, not a revision: comparing it against a
    /// git sha would report a mismatch nobody could act on, so it reads as nothing at all. An
    /// `astrid-core-revision:` line, if the generator ever writes one, answers.
    #[test]
    fn a_body_sha256_is_not_a_revision_97285677() {
        let today = "\
// GENERATED by `cargo xtask gen-design` from astrid-core design/tokens.json. Do not edit.
// body-sha256: 71c7bc9ebac73235f1879de7f7e45070fe3b12f4fe3d340f1943d4c24d5c6eee
import SwiftUI
";
        assert_eq!(
            manifest_revision(std::path::Path::new("AstridTokens.swift"), today),
            None
        );

        let one_day = "\
// GENERATED by `cargo xtask gen-design` from astrid-core design/tokens.json. Do not edit.
// astrid-core-revision: a503a62039fca9a19ce7802a2d9f6d38ae5f89d8
// body-sha256: 71c7bc9ebac73235f1879de7f7e45070fe3b12f4fe3d340f1943d4c24d5c6eee
";
        assert_eq!(
            manifest_revision(std::path::Path::new("AstridTokens.swift"), one_day).as_deref(),
            Some("a503a62039fca9a19ce7802a2d9f6d38ae5f89d8")
        );
    }

    /// The core's channel is read from its own file rather than hardcoded, so a toolchain bump is
    /// one edit and this check follows it. The legacy one-line form answers too.
    #[test]
    fn a_toolchain_channel_is_read_from_the_file_97285677() {
        let directory =
            std::env::temp_dir().join("astrid-core-check-pins-97285677-toolchain-floating");
        std::fs::create_dir_all(&directory).expect("a temp directory");
        std::fs::write(
            directory.join("rust-toolchain.toml"),
            "# a comment\n[toolchain]\nchannel = \"stable\"\ncomponents = [\"clippy\"]\n",
        )
        .expect("write the toolchain file");
        assert_eq!(toolchain_channel(&directory).as_deref(), Some("stable"));

        let legacy = std::env::temp_dir().join("astrid-core-check-pins-97285677-toolchain-legacy");
        std::fs::create_dir_all(&legacy).expect("a temp directory");
        std::fs::write(legacy.join("rust-toolchain"), "1.82.0\n").expect("write the legacy file");
        assert_eq!(toolchain_channel(&legacy).as_deref(), Some("1.82.0"));

        // This repository's own file is the one the comparison is made against.
        assert_eq!(
            toolchain_channel(&super::repo_root()).as_deref(),
            Some("1.98.1"),
            "rust-toolchain.toml's channel should be readable; if it was bumped, bump this too"
        );
    }

    /// Days as well as commits, and never a negative number: a pin can be newer than a local `main`
    /// nobody has pushed, and "-3d" beside "0 behind" reads like a bug in the check.
    #[test]
    fn an_age_is_whole_days_and_never_negative_97285677() {
        assert_eq!(days_between(0, 86_400), 1);
        assert_eq!(days_between(0, 86_399), 0);
        assert_eq!(days_between(86_400, 0), 0);
        assert_eq!(days_between(1_762_000_000, 1_762_000_000 + 20 * 86_400), 20);
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
        for key in CONSUMERS {
            let consumer = consumer_row(&table, key)
                .unwrap_or_else(|| panic!("{key} has no row with a revision in CONSUMERS.md"));
            assert!(
                is_full_sha(&consumer.revision),
                "{key} is pinned at '{}', which is not a full forty-character sha",
                consumer.revision
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

    /// **An absent checkout is UNCHECKED, never a failure.** CI holds the five private
    /// repositories only when `CONSUMER_PINS_TOKEN` is set, and a fork's pull request gets no
    /// secrets at all — so failing here would turn every outside contribution red for a reason the
    /// contributor cannot fix, and the check would be worse than the gap it closed.
    #[test]
    fn an_absent_checkout_is_unchecked_not_failed_4c891314() {
        let nowhere = std::env::temp_dir().join("astrid-core-check-pins-4c891314-absent");
        let consumer = consumer_row(TABLE, "astrid-ios/core").expect("the fixture has that row");
        match read_manifest(&consumer, &nowhere) {
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
    /// unchecked forever while looking like it had asked. There is no second table to keep in step
    /// any more — the paths are in the row (task 97285677) — so this asks the committed file.
    ///
    /// **One row is allowed to name nothing**, and exactly one does: `astrid-ios/design`, whose
    /// generated header carries a content hash instead of a revision. CONSUMERS.md says so in a
    /// section of its own. A second such row means somebody recorded a copy without recording where
    /// its pin is written, which is the gap this file exists to close.
    #[test]
    fn every_consumer_has_a_manifest_4c891314() {
        let table = std::fs::read_to_string(super::repo_root().join("CONSUMERS.md"))
            .expect("CONSUMERS.md is committed at the repo root");
        let unrecorded: Vec<&str> = CONSUMERS
            .iter()
            .copied()
            .filter(|key| {
                consumer_row(&table, key)
                    .unwrap_or_else(|| panic!("{key} has no row in CONSUMERS.md"))
                    .manifests
                    .is_empty()
            })
            .collect();
        assert_eq!(unrecorded, vec!["astrid-ios/design"]);
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

/// ADEV-7: a client that bumps its pin along `main` before `CONSUMERS.md` does leaves the row
/// *behind its consumer*. That is reported and followed, never failed; every other disagreement
/// still fails. These run against a throwaway git repository, because the verdict is a question
/// about history and a fake would only test the fake.
#[cfg(test)]
mod follow_tests {
    use super::{evaluate_pins, follow_table, with_revision, Checkout};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    const WEB: &str = "astrid-web/astrid-rules";
    const VSCODE: &str = "astrid-vscode/astrid-rules";
    const CODE: &str = "astrid-code/astrid-rules";

    /// A core with `main` = c1 -> c2 -> c3 and an unmerged branch `side` = c2 -> s1.
    struct History {
        dir: PathBuf,
        core: PathBuf,
        c1: String,
        c3: String,
        s1: String,
    }

    impl Drop for History {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=adev-7",
                "-c",
                "user.email=adev-7@example.test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn history(name: &str) -> History {
        let dir = std::env::temp_dir().join(format!(
            "astrid-core-check-pins-adev-7-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let core = dir.join("astrid-core");
        std::fs::create_dir_all(&core).unwrap();
        git(&core, &["init", "-q", "-b", "main"]);
        let commit = |message: &str| {
            git(&core, &["commit", "-q", "--allow-empty", "-m", message]);
            git(&core, &["rev-parse", "HEAD"])
        };
        let c1 = commit("c1");
        let _c2 = commit("c2");
        let c3 = commit("c3");
        git(&core, &["checkout", "-q", "-b", "side", "HEAD~1"]);
        let s1 = commit("s1");
        git(&core, &["checkout", "-q", "main"]);
        History {
            dir,
            core,
            c1,
            c3,
            s1,
        }
    }

    fn row(key: &str, revision: &str) -> String {
        format!("| `{key}` | repo | the rules | `REVISION` | `{revision}` |\n")
    }

    fn table(rows: &[(&str, &str)]) -> String {
        let mut table = String::from(
            "# Pins\n\n| Consumer | Repository | Takes | Pinned in | Revision |\n|---|---|---|---|---|\n",
        );
        for (key, revision) in rows {
            table.push_str(&row(key, revision));
        }
        table.push_str("\nProse after the table mentions `REVISION`.\n");
        table
    }

    /// A checkout of `key`'s repository whose `REVISION` says `pin`.
    fn consumer(
        history: &History,
        checkouts: &mut HashMap<String, Checkout>,
        key: &str,
        pin: &str,
        named: bool,
    ) {
        let repo = key.split('/').next().unwrap();
        let path = history.dir.join(repo);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("REVISION"), format!("{pin}\n")).unwrap();
        checkouts.insert(repo.to_string(), Checkout { path, named });
    }

    #[test]
    fn a_row_behind_a_consumer_on_main_is_reported_not_failed_adev_7() {
        let h = history("behind");
        let mut checkouts = HashMap::new();
        consumer(&h, &mut checkouts, WEB, &h.c3, true);

        let report = evaluate_pins(&h.core, "main", &table(&[(WEB, &h.c1)]), &[WEB], &checkouts);

        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.behind.len(), 1);
        assert_eq!(report.behind[0].row, h.c1);
        assert_eq!(report.behind[0].found, h.c3);
    }

    #[test]
    fn a_consumer_pinned_off_main_still_fails_adev_7() {
        let h = history("off-main");
        let mut checkouts = HashMap::new();
        consumer(&h, &mut checkouts, WEB, &h.s1, true);

        let report = evaluate_pins(&h.core, "main", &table(&[(WEB, &h.c1)]), &[WEB], &checkouts);

        assert!(report.behind.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert!(
            report.failed[0].contains("NOT an ancestor"),
            "{:?}",
            report.failed
        );
    }

    #[test]
    fn a_row_ahead_of_its_consumer_still_fails_adev_7() {
        let h = history("ahead");
        let mut checkouts = HashMap::new();
        consumer(&h, &mut checkouts, WEB, &h.c1, true);

        let report = evaluate_pins(&h.core, "main", &table(&[(WEB, &h.c3)]), &[WEB], &checkouts);

        assert!(report.behind.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert!(
            report.failed[0].contains("ahead of its consumer"),
            "{:?}",
            report.failed
        );
    }

    #[test]
    fn a_consumer_with_a_short_pin_still_fails_adev_7() {
        let h = history("short");
        let mut checkouts = HashMap::new();
        consumer(&h, &mut checkouts, WEB, &h.c3[..7], true);

        let report = evaluate_pins(&h.core, "main", &table(&[(WEB, &h.c1)]), &[WEB], &checkouts);

        assert!(report.behind.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert!(
            report.failed[0].contains("forty-character"),
            "{:?}",
            report.failed
        );
    }

    #[test]
    fn an_unlisted_consumer_and_an_off_main_row_still_fail_adev_7() {
        let h = history("unlisted");
        let checkouts = HashMap::new();

        let report = evaluate_pins(
            &h.core,
            "main",
            &table(&[(WEB, &h.s1)]),
            &[WEB, VSCODE],
            &checkouts,
        );

        assert_eq!(report.failed.len(), 2, "{:?}", report.failed);
        assert!(report
            .failed
            .iter()
            .any(|f| f.starts_with(VSCODE) && f.contains("no row")));
        assert!(report
            .failed
            .iter()
            .any(|f| f.starts_with(WEB) && f.contains("NOT an ancestor")));
    }

    #[test]
    fn follow_pins_rewrites_exactly_the_behind_rows_adev_7() {
        let h = history("follow");
        let mut checkouts = HashMap::new();
        consumer(&h, &mut checkouts, WEB, &h.c3, true); // behind: follows
        consumer(&h, &mut checkouts, VSCODE, &h.s1, true); // off main: stays, and fails
        consumer(&h, &mut checkouts, CODE, &h.c1, true); // level: stays
        let before = table(&[(WEB, &h.c1), (VSCODE, &h.c1), (CODE, &h.c1)]);

        let (after, moved) =
            follow_table(&h.core, "main", &before, &[WEB, VSCODE, CODE], &checkouts);

        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].key, WEB);
        assert_eq!(
            after,
            table(&[(WEB, &h.c3), (VSCODE, &h.c1), (CODE, &h.c1)]),
            "only the behind row's revision changes, to the full sha"
        );
        assert!(after.contains(&format!("`{}`", h.c3)));
    }

    #[test]
    fn follow_pins_is_idempotent_adev_7() {
        let h = history("idempotent");
        let mut checkouts = HashMap::new();
        consumer(&h, &mut checkouts, WEB, &h.c3, true);
        let keys = [WEB];

        let (once, _) = follow_table(&h.core, "main", &table(&[(WEB, &h.c1)]), &keys, &checkouts);
        let (twice, moved) = follow_table(&h.core, "main", &once, &keys, &checkouts);

        assert!(moved.is_empty());
        assert_eq!(once, twice);
        let report = evaluate_pins(&h.core, "main", &twice, &keys, &checkouts);
        assert!(report.failed.is_empty() && report.behind.is_empty());
    }

    /// A sibling found beside this repository may hold a bump nobody pushed, so only a checkout
    /// named on the command line is followed. It is still reported as behind.
    #[test]
    fn follow_pins_ignores_an_unnamed_checkout_adev_7() {
        let h = history("unnamed");
        let mut checkouts = HashMap::new();
        consumer(&h, &mut checkouts, WEB, &h.c3, false);
        let before = table(&[(WEB, &h.c1)]);

        let (after, moved) = follow_table(&h.core, "main", &before, &[WEB], &checkouts);

        assert!(moved.is_empty());
        assert_eq!(after, before);
    }

    /// The rewrite touches the one cell: other rows, prose, and CRLF line endings are untouched,
    /// and a key that is a prefix of another does not match it.
    #[test]
    fn a_revision_rewrite_touches_one_cell_adev_7() {
        let old = "a".repeat(40);
        let new = "b".repeat(40);
        let crlf = format!(
            "|K|P|R|\r\n|---|---|---|\r\n| `{WEB}` | `REVISION` | `{old}` |\r\n| `{WEB}-x` | `REVISION` | `{old}` |\r\n"
        );

        let rewritten = with_revision(&crlf, WEB, &new).unwrap();

        assert_eq!(
            rewritten,
            format!(
                "|K|P|R|\r\n|---|---|---|\r\n| `{WEB}` | `REVISION` | `{new}` |\r\n| `{WEB}-x` | `REVISION` | `{old}` |\r\n"
            )
        );
        assert!(with_revision(&crlf, "astrid-nobody/core", &new).is_none());
    }
}
