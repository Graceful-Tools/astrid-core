//! Replays the `showRule` half of `contracts/fixtures/task-identifiers.json` — astrid-web's own
//! hand-authored case set (`docs/specs/TASK_IDENTIFIERS.md` §7) — against this crate's port.
//!
//! Where an id is SHOWN is the kind of rule that drifts silently. Every client can read
//! `task.identifier`, so every client can draw it, and three clients drawing it in three different
//! places is not a bug any test would catch on its own: `AWTD-1007` beside a title on a personal
//! list is not an error, just an id that means nothing to the person reading it. So the answers come
//! from the shared fixture rather than from a reading of the spec.
//!
//! What is locked: which of the three surfaces shows the id for each combination of "has an id" and
//! "is on a board", and whether "Copy task id" is offered at all.
//!
//! The `parse` and `autolink` halves of the same fixture are replayed here too (task `5f3453e2`).
//! They are the other way the same rule drifts: a client that autolinks `UTF-8` because it never
//! checked the key against the reader's projects, or that links an id the reader cannot open and
//! leaves them at a 404, has not made a formatting mistake — it has invented a different rule.

use astrid_core::identifier::{find_links, parse_identifier, LinkContext};
use astrid_core::model::{Task, TaskList};
use astrid_core::rows::detail::is_task_in_project;
use astrid_core::rows::identifier::{offers_copy_identifier, shows_identifier};
use astrid_core::rows::Surface;
use serde::Deserialize;
use std::collections::HashMap;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/task-identifiers.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    parse: Vec<ParseCase>,
    autolink: Autolink,
    show_rule: ShowRule,
}

#[derive(Deserialize)]
struct ParseCase {
    input: String,
    expected: Option<ParsedCase>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
struct ParsedCase {
    key: String,
    sequence: u64,
}

#[derive(Deserialize)]
struct Autolink {
    cases: Vec<AutolinkCase>,
}

#[derive(Deserialize)]
struct AutolinkCase {
    name: String,
    text: String,
    context: FixtureContext,
    links: Vec<ExpectedLink>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureContext {
    project_key: Option<String>,
    keys: Vec<String>,
    #[serde(default)]
    hidden: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedLink {
    #[serde(rename = "match")]
    matched: String,
    identifier: String,
    href: String,
}

#[derive(Deserialize)]
struct ShowRule {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    identifier: Option<String>,
    lists: Vec<FixtureList>,
    /// Keyed by the fixture's surface names: `details`, `row-board`, `row-list`.
    show: HashMap<String, bool>,
    copy: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureList {
    project_id: Option<String>,
}

/// The fixture describes lists by the only property the rule reads, so the ids are ours to invent —
/// they exist solely to join the task to its lists.
fn scene(case: &Case) -> (Task, Vec<TaskList>) {
    let lists: Vec<TaskList> = case
        .lists
        .iter()
        .enumerate()
        .map(|(index, list)| {
            serde_json::from_value(serde_json::json!({
                "id": format!("list-{index}"),
                "name": format!("list-{index}"),
                "projectId": list.project_id,
            }))
            .expect("a list")
        })
        .collect();

    let mut task = Task::new("t1", "a task");
    task.identifier = case.identifier.clone();
    task.list_ids = Some(lists.iter().map(|list| list.id.clone()).collect());
    (task, lists)
}

fn surface_of(name: &str) -> Surface {
    match name {
        "details" => Surface::Detail,
        "row-board" => Surface::BoardCard,
        "row-list" => Surface::ListRow,
        other => panic!("the fixture grew a surface this port does not know: {other}"),
    }
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("contracts/fixtures/task-identifiers.json parses")
}

#[test]
fn every_show_rule_case_matches_web() {
    let fixture = fixture();
    assert!(
        !fixture.show_rule.cases.is_empty(),
        "the show-rule cases went missing — the fixture shape changed"
    );

    for case in &fixture.show_rule.cases {
        let (task, lists) = scene(case);
        let has_identifier = task.identifier.as_deref().is_some_and(|id| !id.is_empty());
        let in_project = is_task_in_project(&task, &lists);

        for (surface_name, expected) in &case.show {
            let actual = shows_identifier(surface_of(surface_name), has_identifier, in_project);
            assert_eq!(
                actual,
                *expected,
                "{}: {surface_name} should {}show the id",
                case.name,
                if *expected { "" } else { "not " }
            );
        }

        assert_eq!(
            offers_copy_identifier(has_identifier),
            case.copy,
            "{}: 'Copy task id' should {}be offered",
            case.name,
            if case.copy { "" } else { "not " }
        );
    }
}

#[test]
fn every_parse_case_matches_web() {
    let fixture = fixture();
    assert!(!fixture.parse.is_empty(), "the parse cases went missing");

    for case in &fixture.parse {
        let actual = parse_identifier(&case.input);
        let why = case.note.as_deref().unwrap_or("no note");
        match &case.expected {
            Some(expected) => {
                let parsed = actual.unwrap_or_else(|| {
                    panic!("{:?} should parse ({why})", case.input);
                });
                assert_eq!(parsed.key, expected.key, "{:?}: key ({why})", case.input);
                assert_eq!(
                    parsed.sequence, expected.sequence,
                    "{:?}: sequence ({why})",
                    case.input
                );
            }
            None => assert!(
                actual.is_none(),
                "{:?} should not parse ({why})",
                case.input
            ),
        }
    }
}

#[test]
fn every_autolink_case_matches_web() {
    let fixture = fixture();
    assert!(
        !fixture.autolink.cases.is_empty(),
        "the autolink cases went missing"
    );

    for case in &fixture.autolink.cases {
        let context = LinkContext {
            project_key: case.context.project_key.clone(),
            keys: case.context.keys.clone(),
            hidden: case.context.hidden.clone(),
        };
        let found = find_links(&case.text, &context);

        assert_eq!(
            found.len(),
            case.links.len(),
            "{}: expected {} link(s), found {:?}",
            case.name,
            case.links.len(),
            found
                .iter()
                .map(|link| link.matched.as_str())
                .collect::<Vec<_>>()
        );

        for (actual, expected) in found.iter().zip(&case.links) {
            assert_eq!(
                actual.matched, expected.matched,
                "{}: matched text",
                case.name
            );
            assert_eq!(
                actual.identifier, expected.identifier,
                "{}: identifier",
                case.name
            );
            assert_eq!(actual.href, expected.href, "{}: href", case.name);
            assert_eq!(
                &case.text[actual.index..actual.index + actual.matched.len()],
                actual.matched,
                "{}: index must point at the match in the ORIGINAL text, since that is what a                  renderer slices",
                case.name
            );
        }
    }
}

/// The fixture has no case for it — the server mints ids only on project lists — but a task can hold
/// a role from a board whose list the viewer cannot see, and hiding an id the task demonstrably has
/// would lose information. Same allowance `shows_task_blockers` already makes.
#[test]
fn a_role_carried_from_an_invisible_board_still_shows_the_id() {
    let plain: TaskList = serde_json::from_value(serde_json::json!({
        "id": "plain", "name": "Personal", "projectId": null
    }))
    .expect("a list");
    let mut task = Task::new("t1", "a task");
    task.identifier = Some("AWTD-1".to_string());
    task.list_ids = Some(vec!["plain".to_string()]);
    task.status_role = Some("ready".to_string());

    assert!(shows_identifier(
        Surface::BoardCard,
        true,
        is_task_in_project(&task, std::slice::from_ref(&plain))
    ));
}
