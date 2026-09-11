//! The C# bindings speak only commands the core understands.
//!
//! `app/Astrid.Core.Bindings/Commands.cs` is hand-written — deliberately, and deliberately
//! partial: a command with no caller yet has no record. What that leaves unguarded is the other
//! direction. A variant renamed here, or a `kind` mistyped there, is a command the core answers
//! with "bad request" — and the shell shows that as a button that does nothing, which is the
//! hardest kind of bug to report. So the two sources are read together, here, in the gate that
//! already runs on every push.
//!
//! Source scanning rather than reflection: `serde` names the variants and Rust cannot enumerate
//! them at runtime without a dependency, while the C# side is a set of string literals. Both
//! scans are anchored on shapes the files have had since M2 and fail loudly if either stops
//! matching, so "the parser broke" cannot read as "the contract holds".

use std::collections::BTreeSet;

const COMMAND_RS: &str = include_str!("../src/app/command.rs");
const COMMANDS_CS: &str = include_str!("../../../app/Astrid.Core.Bindings/Commands.cs");

/// Every variant of `Command`, as the wire spells it: `rename_all = "camelCase"` lowercases the
/// first letter and nothing else, so `CreateOAuthClient` is `createOAuthClient`.
fn rust_kinds() -> BTreeSet<String> {
    let start = COMMAND_RS
        .find("pub enum Command {")
        .expect("command.rs declares `pub enum Command {`");
    let body = &COMMAND_RS[start..];
    let end = body
        .find("\n}\n")
        .expect("the Command enum closes with a bare `}`");
    body[..end]
        .lines()
        .filter_map(|line| {
            // A variant is a four-space-indented capitalised identifier, followed by `{`, `,` or
            // `(`. Doc comments, attributes and fields are all indented differently or start
            // with something else.
            let rest = line.strip_prefix("    ")?;
            if rest.starts_with(' ') {
                return None;
            }
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let after = rest[name.len()..].trim_start();
            let is_variant = !name.is_empty()
                && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && (after.starts_with('{') || after.starts_with(',') || after.starts_with('('));
            is_variant.then(|| camel(&name))
        })
        .collect()
}

fn camel(pascal: &str) -> String {
    let mut chars = pascal.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Every `kind` the C# factories send: the first string literal handed to one of the request
/// records declared in the same file.
fn csharp_kinds() -> BTreeSet<String> {
    let records: Vec<&str> = COMMANDS_CS
        .lines()
        .filter_map(|line| {
            let after = line.trim_start().strip_prefix("private sealed record ")?;
            Some(after.split('(').next()?.trim())
        })
        .collect();
    assert!(
        records.len() >= 10,
        "found only {} request records in Commands.cs; the scan has stopped working",
        records.len()
    );

    let mut kinds = BTreeSet::new();
    for record in records {
        let needle = format!("new {record}(\"");
        let mut rest = COMMANDS_CS;
        while let Some(at) = rest.find(&needle) {
            let literal = &rest[at + needle.len()..];
            let end = literal.find('"').expect("a closed string literal");
            kinds.insert(literal[..end].to_string());
            rest = &literal[end..];
        }
    }
    kinds
}

#[test]
fn every_command_the_shell_sends_is_one_the_core_understands() {
    let rust = rust_kinds();
    let csharp = csharp_kinds();
    assert!(
        rust.len() >= 100,
        "found only {} Command variants; the scan has stopped working",
        rust.len()
    );
    assert!(
        csharp.len() >= 50,
        "found only {} kinds in Commands.cs; the scan has stopped working",
        csharp.len()
    );

    let unknown: Vec<&String> = csharp.difference(&rust).collect();
    assert!(
        unknown.is_empty(),
        "Commands.cs sends kinds the core has no variant for — each is a button that does \
         nothing: {unknown:?}"
    );
}

/// Not a failure: the bindings are a list of what the shell uses, not of what exists. Printed so
/// the gap is visible in a test log rather than only discoverable by diffing two files.
#[test]
fn the_commands_the_shell_does_not_send_yet_are_listed() {
    let rust = rust_kinds();
    let csharp = csharp_kinds();
    let unused: Vec<&String> = rust.difference(&csharp).collect();
    eprintln!("commands with no C# caller yet: {unused:?}");
}
