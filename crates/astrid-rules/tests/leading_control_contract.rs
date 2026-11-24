//! The leading control is drawn and acts as astrid-web's does (ADEV-147).
//!
//! `contracts/fixtures/leading-control.json` is astrid-web's `lib/task-leading-control.ts` run over
//! every assignee (none, `""`, the reader, someone else) × reader (signed in, not) × completed. Each
//! case records the mark (`taskLeadingControlKind`) and whether a tap opens the picker rather than
//! completing, on a list row and a board card (`leadingControlOpensOptions`). Every case runs here
//! through [`LeadingControl::for_row`] and through a whole [`TaskRow::build`], which is what iOS
//! and the Mac draw from.
//!
//! The rule the fixture exists for: **a completed task is the checked checkbox whoever holds it**,
//! while a tap on someone else's completed task still opens the picker instead of reopening it.

use std::collections::HashMap;

use astrid_rules::model::{date, Task};
use astrid_rules::rows::{LeadingAction, LeadingControl, RowContext, Surface, TaskRow};
use jiff::tz::Offset;
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("../../../contracts/fixtures/leading-control.json");

fn cases() -> Vec<Value> {
    let raw: Value = serde_json::from_str(FIXTURE).expect("the fixture parses");
    raw["cases"].as_array().expect("cases").clone()
}

fn kind(control: &LeadingControl) -> &'static str {
    match control {
        LeadingControl::Checkbox => "checkbox",
        LeadingControl::Avatar(_) => "avatar",
        LeadingControl::Unassigned => "unassigned",
        LeadingControl::Copy => "copy",
    }
}

fn label(case: &Value) -> String {
    format!(
        "assignee={} reader={} completed={}",
        case["assigneeId"], case["currentUserId"], case["completed"]
    )
}

#[test]
fn the_fixture_covers_every_combination() {
    // 4 assignees × 2 readers × 2 completion states.
    assert_eq!(cases().len(), 16);
}

#[test]
fn for_row_draws_the_mark_web_draws() {
    for case in cases() {
        let control = LeadingControl::for_row(
            case["assigneeId"].as_str(),
            case["currentUserId"].as_str(),
            case["completed"].as_bool().expect("completed"),
        );
        assert_eq!(
            kind(&control),
            case["kind"].as_str().expect("kind"),
            "{}",
            label(&case)
        );
    }
}

#[test]
fn a_completed_task_of_someone_elses_is_the_checkbox() {
    // ADEV-147 by name, so the one case Jon reported cannot be lost in the table.
    assert_eq!(
        LeadingControl::for_row(Some("other"), Some("me"), true),
        LeadingControl::Checkbox
    );
    assert_eq!(
        LeadingControl::for_row(Some("other"), Some("me"), false),
        LeadingControl::Avatar("other".to_string())
    );
}

#[test]
fn a_built_row_draws_and_acts_as_web_does() {
    let now = date::parse("2026-11-20T12:00:00Z").expect("an instant");
    let depths = HashMap::new();
    let counts = HashMap::new();
    for case in cases() {
        let task: Task = serde_json::from_value(json!({
            "id": "t1",
            "title": "A task",
            "assigneeId": case["assigneeId"],
            "completed": case["completed"],
        }))
        .expect("a task");
        for (surface, key) in [
            (Surface::ListRow, "listRow"),
            (Surface::BoardCard, "boardCard"),
        ] {
            let context = RowContext {
                surface,
                current_user_id: case["currentUserId"].as_str(),
                now,
                offset: Offset::UTC,
                lists: &[],
                users: &[],
                depths: &depths,
                subtask_counts: &counts,
            };
            let row = TaskRow::build(&task, &context);
            assert_eq!(
                kind(&row.leading),
                case["kind"].as_str().unwrap(),
                "{} {key}",
                label(&case)
            );
            let opens_picker = row.action != LeadingAction::Complete;
            assert_eq!(
                opens_picker,
                case["opensPicker"][key].as_bool().unwrap(),
                "{} {key}",
                label(&case)
            );
        }
    }
}

/// ADEV-201: whether completing asks first is carried on the row. Web's rule
/// (`completionNeedsConfirmation`): completed → false; no reader → true; otherwise assignee ≠
/// reader. The expected value is web's own answer, the fixture's `asksBeforeCompleting` column
/// (ADEV-255), not one worked out here — a derivation in this file could drift from web unnoticed.
#[test]
fn adev_201_a_row_carries_whether_completing_asks() {
    let now = date::parse("2026-11-20T12:00:00Z").expect("an instant");
    let depths = HashMap::new();
    let counts = HashMap::new();
    for case in cases() {
        let completed = case["completed"].as_bool().unwrap();
        let reader = case["currentUserId"].as_str();
        let expected = case["asksBeforeCompleting"]
            .as_bool()
            .unwrap_or_else(|| panic!("asksBeforeCompleting on {}", label(&case)));
        let task: Task = serde_json::from_value(json!({
            "id": "t1",
            "title": "A task",
            "assigneeId": case["assigneeId"],
            "completed": completed,
        }))
        .expect("a task");
        let context = RowContext {
            surface: Surface::ListRow,
            current_user_id: reader,
            now,
            offset: Offset::UTC,
            lists: &[],
            users: &[],
            depths: &depths,
            subtask_counts: &counts,
        };
        let row = TaskRow::build(&task, &context);
        assert_eq!(
            row.completion_needs_confirmation,
            expected,
            "{}",
            label(&case)
        );
    }
}
