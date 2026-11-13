//! Fixtures generated FROM this crate: the core-first contracts.
//!
//! Every other fixture in `contracts/fixtures/` is generated in astrid-web by running web's
//! implementation, and copied here. A rule that is canonical in core from its first day — the
//! Waiting lane (Jon, 2026-11-12) — has no web implementation to run, so its fixture is generated
//! here instead, by sending each case through the rules door (`astrid_rules::rules::run_json`,
//! the very function the WebAssembly build exports as `runJson`) and recording the answer. The
//! direction is reversed and nothing else is: web copies these files, and its TypeScript mirror
//! (where it keeps one) must answer every case the same.
//!
//! Each case also carries an `expect`: the part of the answer a person wrote down. Generation
//! fails when the rule answers otherwise, so a regenerated fixture cannot quietly record a
//! regression as the new truth.

use serde_json::{json, Value};
use std::path::Path;

/// The fixtures this repository generates, which `check-contracts` does not expect astrid-web to
/// generate.
pub const CORE_OWNED: &[&str] = &["waiting.json", "list-default-picks.json"];

const NOW: &str = "2026-11-12T12:00:00.000Z";

struct Case {
    name: &'static str,
    rule: &'static str,
    request: Value,
    expect: Value,
}

fn case(name: &'static str, rule: &'static str, request: Value, expect: Value) -> Case {
    Case {
        name,
        rule,
        request,
        expect,
    }
}

fn write(task: Value, blockers: Value, change: Value) -> Value {
    json!({ "kind": "waitingWrite", "task": task, "blockers": blockers, "change": change, "now": NOW })
}

fn wake_at(task: Value, blockers: Value, now: &str) -> Value {
    json!({ "kind": "waitingWake", "task": task, "blockers": blockers, "now": now })
}

fn settle(task: Value, blockers: Value) -> Value {
    json!({ "kind": "waitingSettle", "task": task, "blockers": blockers, "now": NOW })
}

fn ok(value: Value) -> Value {
    json!({ "ok": true, "value": value })
}

fn refused() -> Value {
    json!({ "ok": false, "error": { "kind": "refused", "code": "WAITING_NEEDS_CONDITION" } })
}

fn entered(assignee: &str) -> Value {
    ok(json!({ "statusRole": "waiting", "assignee": assignee, "transition": "enter" }))
}

fn waiting_cases() -> Vec<Case> {
    let to_waiting = json!({ "statusRole": "waiting" });
    vec![
        // ── R1/R2: entering Waiting ─────────────────────────────────────────────────────────
        case(
            "enter/future-date-already-on-task",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "ready", "dueDateTime": "2026-11-20T09:00:00.000Z", "assigneeId": "agent" }),
                json!([]),
                to_waiting.clone(),
            ),
            entered("clear"),
        ),
        case(
            "enter/future-date-supplied-in-the-write",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "doing", "assigneeId": "agent" }),
                json!([]),
                json!({ "statusRole": "waiting", "dueDateTime": "2026-11-15T00:00:00.000Z" }),
            ),
            entered("clear"),
        ),
        case(
            "enter/incomplete-blocker-already-attached",
            "R2",
            write(
                json!({ "id": "t", "assigneeId": "jon" }),
                json!([{ "id": "b1", "completed": false }]),
                to_waiting.clone(),
            ),
            entered("clear"),
        ),
        case(
            "enter/blocker-attached-in-the-write",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "ready" }),
                json!([]),
                json!({ "statusRole": "waiting", "addBlockers": [{ "id": "b2", "completed": false }] }),
            ),
            entered("keep"),
        ),
        case(
            "enter/only-complete-blockers-is-refused",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "doing" }),
                json!([{ "id": "b1", "completed": true }, { "id": "b2", "completed": true }]),
                to_waiting.clone(),
            ),
            refused(),
        ),
        case(
            "enter/past-date-is-refused",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "doing", "dueDateTime": "2026-11-11T09:00:00.000Z" }),
                json!([]),
                to_waiting.clone(),
            ),
            refused(),
        ),
        case(
            "enter/date-exactly-now-is-refused",
            "R2",
            write(
                json!({ "id": "t", "dueDateTime": NOW }),
                json!([]),
                to_waiting.clone(),
            ),
            refused(),
        ),
        case(
            "enter/past-date-supplied-in-the-write-is-refused",
            "R2",
            write(
                json!({ "id": "t" }),
                json!([]),
                json!({ "statusRole": "waiting", "dueDateTime": "2026-11-12T11:59:59.000Z" }),
            ),
            refused(),
        ),
        case(
            "enter/all-day-today-has-started-and-is-refused",
            "R2",
            write(
                json!({ "id": "t", "dueDateTime": "2026-11-12T00:00:00.000Z", "isAllDay": true }),
                json!([]),
                to_waiting.clone(),
            ),
            refused(),
        ),
        case(
            "enter/all-day-tomorrow-counts",
            "R2",
            write(
                json!({ "id": "t", "dueDateTime": "2026-11-13T00:00:00.000Z", "isAllDay": true }),
                json!([]),
                to_waiting.clone(),
            ),
            entered("keep"),
        ),
        case(
            "enter/no-condition-is-refused",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "ready", "assigneeId": "agent" }),
                json!([]),
                to_waiting.clone(),
            ),
            refused(),
        ),
        case(
            "enter/date-cleared-in-the-same-write-is-refused",
            "R2",
            write(
                json!({ "id": "t", "dueDateTime": "2026-11-20T09:00:00.000Z" }),
                json!([]),
                json!({ "statusRole": "waiting", "dueDateTime": null }),
            ),
            refused(),
        ),
        case(
            "enter/blocker-removed-in-the-same-write-is-refused",
            "R2",
            write(
                json!({ "id": "t" }),
                json!([{ "id": "b1", "completed": false }]),
                json!({ "statusRole": "waiting", "removeBlockerIds": ["b1"] }),
            ),
            refused(),
        ),
        case(
            "enter/keep-assignee-true-keeps-it",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "doing", "dueDateTime": "2026-11-20T09:00:00.000Z", "assigneeId": "jon" }),
                json!([]),
                json!({ "statusRole": "waiting", "keepAssignee": true }),
            ),
            entered("keep"),
        ),
        case(
            "enter/keep-assignee-false-clears-it",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "doing", "dueDateTime": "2026-11-20T09:00:00.000Z", "assigneeId": "jon" }),
                json!([]),
                json!({ "statusRole": "waiting", "keepAssignee": false }),
            ),
            entered("clear"),
        ),
        case(
            "enter/from-done-is-entering",
            "R2",
            write(
                json!({ "id": "t", "completed": true, "statusRole": "waiting", "assigneeId": "jon" }),
                json!([{ "id": "b1", "completed": false }]),
                to_waiting.clone(),
            ),
            entered("clear"),
        ),
        // ── R2 at birth: a new task (no role, no blockers) created in Waiting ──────────────
        case(
            "create/with-a-future-date-is-born-waiting-unassigned",
            "R2",
            write(
                json!({ "id": "", "assigneeId": "jon" }),
                json!([]),
                json!({ "statusRole": "waiting", "dueDateTime": "2026-11-13T00:00:00.000Z" }),
            ),
            entered("clear"),
        ),
        case(
            "create/waiting-on-a-task-is-born-waiting",
            "R2",
            write(
                json!({ "id": "" }),
                json!([]),
                json!({ "statusRole": "waiting", "addBlockers": [{ "id": "b1", "completed": false }] }),
            ),
            entered("keep"),
        ),
        case(
            "create/keep-assignee-keeps-it",
            "R2",
            write(
                json!({ "id": "", "assigneeId": "jon" }),
                json!([]),
                json!({ "statusRole": "waiting", "dueDateTime": "2026-11-13T00:00:00.000Z", "keepAssignee": true }),
            ),
            entered("keep"),
        ),
        case(
            "create/with-no-condition-is-refused",
            "R2",
            write(
                json!({ "id": "", "assigneeId": "jon" }),
                json!([]),
                json!({ "statusRole": "waiting" }),
            ),
            refused(),
        ),
        case(
            "create/with-a-past-date-is-refused",
            "R2",
            write(
                json!({ "id": "" }),
                json!([]),
                json!({ "statusRole": "waiting", "dueDateTime": "2026-11-12T00:00:00.000Z" }),
            ),
            refused(),
        ),
        // ── R2: a task already in Waiting is not entering ──────────────────────────────────
        case(
            "within/an-edit-that-keeps-the-condition-stays",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-20T09:00:00.000Z", "assigneeId": "jon" }),
                json!([]),
                json!({}),
            ),
            ok(json!({ "statusRole": "waiting", "assignee": "keep", "transition": "stay" })),
        ),
        case(
            "within/moving-it-to-waiting-again-does-not-clear-the-assignee",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "waiting", "assigneeId": "jon" }),
                json!([{ "id": "b1", "completed": false }]),
                to_waiting.clone(),
            ),
            ok(json!({ "statusRole": "waiting", "assignee": "keep", "transition": "stay" })),
        ),
        case(
            "within/clearing-the-last-date-wakes-to-ready",
            "R2+R3",
            write(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-20T09:00:00.000Z" }),
                json!([]),
                json!({ "dueDateTime": null }),
            ),
            ok(json!({ "statusRole": "ready", "assignee": "keep", "transition": "wake" })),
        ),
        case(
            "within/moving-the-date-into-the-past-wakes-to-ready",
            "R2+R3",
            write(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-20T09:00:00.000Z" }),
                json!([]),
                json!({ "dueDateTime": "2026-11-01T09:00:00.000Z" }),
            ),
            ok(json!({ "statusRole": "ready", "assignee": "keep", "transition": "wake" })),
        ),
        case(
            "within/removing-the-last-incomplete-blocker-wakes-to-ready",
            "R2+R3",
            write(
                json!({ "id": "t", "statusRole": "waiting", "assigneeId": "jon" }),
                json!([{ "id": "b1", "completed": false }]),
                json!({ "removeBlockerIds": ["b1"] }),
            ),
            ok(json!({ "statusRole": "ready", "assignee": "keep", "transition": "wake" })),
        ),
        case(
            "within/removing-one-of-two-blockers-stays",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "waiting" }),
                json!([{ "id": "b1", "completed": false }, { "id": "b2", "completed": false }]),
                json!({ "removeBlockerIds": ["b1"] }),
            ),
            ok(json!({ "statusRole": "waiting", "assignee": "keep", "transition": "stay" })),
        ),
        case(
            "within/legacy-no-condition-edit-goes-to-inbox",
            "R3",
            write(
                json!({ "id": "t", "statusRole": "waiting", "assigneeId": "jon" }),
                json!([]),
                json!({}),
            ),
            ok(json!({ "statusRole": null, "assignee": "keep", "transition": "inbox" })),
        ),
        // ── Writes that are not about Waiting ──────────────────────────────────────────────
        case(
            "other/leaving-waiting-for-doing-keeps-the-assignee",
            "R2",
            write(
                json!({ "id": "t", "statusRole": "waiting", "assigneeId": "jon" }),
                json!([]),
                json!({ "statusRole": "doing" }),
            ),
            ok(json!({ "statusRole": "doing", "assignee": "keep", "transition": "none" })),
        ),
        case(
            "other/inbox-to-ready-is-untouched",
            "R2",
            write(
                json!({ "id": "t", "assigneeId": "agent" }),
                json!([]),
                json!({ "statusRole": "ready" }),
            ),
            ok(json!({ "statusRole": "ready", "assignee": "keep", "transition": "none" })),
        ),
        case(
            "other/the-testing-preset-needs-no-condition",
            "R7",
            write(
                json!({ "id": "t", "statusRole": "doing", "assigneeId": "agent" }),
                json!([]),
                json!({ "statusRole": "testing" }),
            ),
            ok(json!({ "statusRole": "testing", "assignee": "keep", "transition": "none" })),
        ),
        case(
            "other/to-inbox",
            "R4",
            write(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-20T09:00:00.000Z" }),
                json!([]),
                json!({ "statusRole": null }),
            ),
            ok(json!({ "statusRole": null, "assignee": "keep", "transition": "none" })),
        ),
        // ── R3: waking ─────────────────────────────────────────────────────────────────────
        case(
            "wake/date-reached",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-12T09:00:00.000Z" }),
                json!([]),
                NOW,
            ),
            ok(json!({ "action": "wakeToReady" })),
        ),
        case(
            "wake/date-in-the-future-stays",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-12T12:00:01.000Z" }),
                json!([]),
                NOW,
            ),
            ok(json!({ "action": "stay" })),
        ),
        case(
            "wake/all-day-wakes-at-the-start-of-its-day",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-12T00:00:00.000Z", "isAllDay": true }),
                json!([]),
                "2026-11-12T00:00:00.000Z",
            ),
            ok(json!({ "action": "wakeToReady" })),
        ),
        case(
            "wake/all-day-the-second-before-its-day-stays",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-12T00:00:00.000Z", "isAllDay": true }),
                json!([]),
                "2026-11-11T23:59:59.000Z",
            ),
            ok(json!({ "action": "stay" })),
        ),
        case(
            "wake/last-blocker-completes",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting" }),
                json!([{ "id": "b1", "completed": true }, { "id": "b2", "completed": true }]),
                NOW,
            ),
            ok(json!({ "action": "wakeToReady" })),
        ),
        case(
            "wake/one-blocker-still-open-stays",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting" }),
                json!([{ "id": "b1", "completed": true }, { "id": "b2", "completed": false }]),
                NOW,
            ),
            ok(json!({ "action": "stay" })),
        ),
        case(
            "wake/date-reached-but-a-blocker-is-open-stays",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-10T09:00:00.000Z" }),
                json!([{ "id": "b1", "completed": false }]),
                NOW,
            ),
            ok(json!({ "action": "stay" })),
        ),
        case(
            "wake/blockers-done-but-the-date-is-future-stays",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-20T09:00:00.000Z" }),
                json!([{ "id": "b1", "completed": true }]),
                NOW,
            ),
            ok(json!({ "action": "stay" })),
        ),
        case(
            "wake/legacy-no-condition-resolves-to-inbox",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "waiting", "assigneeId": "jon" }),
                json!([]),
                NOW,
            ),
            ok(json!({ "action": "resolveToInbox" })),
        ),
        case(
            "wake/not-in-waiting-stays",
            "R3",
            wake_at(
                json!({ "id": "t", "statusRole": "doing", "dueDateTime": "2026-11-10T09:00:00.000Z" }),
                json!([]),
                NOW,
            ),
            ok(json!({ "action": "stay" })),
        ),
        // ── R3 + R5: what the scheduled job does ───────────────────────────────────────────
        case(
            "settle/waiting-date-reached-wakes-keeping-the-assignee",
            "R3",
            settle(
                json!({ "id": "t", "statusRole": "waiting", "dueDateTime": "2026-11-12T00:00:00.000Z", "isAllDay": true, "assigneeId": "jon" }),
                json!([]),
            ),
            ok(json!({ "action": "wakeToReady", "statusRole": "ready", "assignee": "keep" })),
        ),
        case(
            "settle/legacy-no-condition-to-inbox-keeping-the-assignee",
            "R3",
            settle(
                json!({ "id": "t", "statusRole": "waiting", "assigneeId": "jon" }),
                json!([]),
            ),
            ok(json!({ "action": "resolveToInbox", "statusRole": null, "assignee": "keep" })),
        ),
        case(
            "settle/waiting-still-blocked-stays",
            "R3",
            settle(
                json!({ "id": "t", "statusRole": "waiting" }),
                json!([{ "id": "b1", "completed": false }]),
            ),
            ok(json!({ "action": "stay", "statusRole": "waiting", "assignee": "keep" })),
        ),
        case(
            "settle/ready-with-a-future-date-parks-in-waiting-unassigned",
            "R5",
            settle(
                json!({ "id": "t", "statusRole": "ready", "dueDateTime": "2026-11-13T00:00:00.000Z", "isAllDay": true, "assigneeId": "agent" }),
                json!([]),
            ),
            ok(json!({ "action": "parkInWaiting", "statusRole": "waiting", "assignee": "clear" })),
        ),
        case(
            "settle/ready-with-a-future-date-and-no-assignee-parks",
            "R5",
            settle(
                json!({ "id": "t", "statusRole": "ready", "dueDateTime": "2026-11-20T09:00:00.000Z" }),
                json!([]),
            ),
            ok(json!({ "action": "parkInWaiting", "statusRole": "waiting", "assignee": "keep" })),
        ),
        case(
            "settle/ready-due-today-stays",
            "R5",
            settle(
                json!({ "id": "t", "statusRole": "ready", "dueDateTime": "2026-11-12T00:00:00.000Z", "isAllDay": true, "assigneeId": "agent" }),
                json!([]),
            ),
            ok(json!({ "action": "stay", "statusRole": "ready", "assignee": "keep" })),
        ),
        case(
            "settle/ready-with-an-open-blocker-and-no-date-stays",
            "R5",
            settle(
                json!({ "id": "t", "statusRole": "ready" }),
                json!([{ "id": "b1", "completed": false }]),
            ),
            ok(json!({ "action": "stay", "statusRole": "ready", "assignee": "keep" })),
        ),
        case(
            "settle/doing-with-a-future-date-stays",
            "R6",
            settle(
                json!({ "id": "t", "statusRole": "doing", "dueDateTime": "2026-11-20T09:00:00.000Z", "assigneeId": "agent" }),
                json!([]),
            ),
            ok(json!({ "action": "stay", "statusRole": "doing", "assignee": "keep" })),
        ),
        case(
            "settle/testing-with-a-future-date-stays",
            "R7",
            settle(
                json!({ "id": "t", "statusRole": "testing", "dueDateTime": "2026-11-20T09:00:00.000Z" }),
                json!([]),
            ),
            ok(json!({ "action": "stay", "statusRole": "testing", "assignee": "keep" })),
        ),
        case(
            "settle/a-finished-task-stays",
            "R3",
            settle(
                json!({ "id": "t", "completed": true, "statusRole": "waiting" }),
                json!([]),
            ),
            ok(json!({ "action": "stay", "statusRole": "waiting", "assignee": "keep" })),
        ),
        // ── R4 ─────────────────────────────────────────────────────────────────────────────
        case(
            "hand-back/inbox-assigned-to-the-person",
            "R4",
            json!({ "kind": "handBack", "assigneeId": "u-jon" }),
            ok(json!({ "statusRole": null, "assigneeId": "u-jon", "completed": false })),
        ),
        // ── R7 ─────────────────────────────────────────────────────────────────────────────
        case(
            "presets/a-board-with-no-columns-of-its-own",
            "R7",
            json!({ "kind": "boardPresets" }),
            ok(
                json!([{ "role": "testing", "name": "Testing", "description": "Deploying, in canary, or in an experiment." }]),
            ),
        ),
        case(
            "presets/a-board-with-other-columns",
            "R7",
            json!({ "kind": "boardPresets", "customStates": [{ "role": "custom-review", "name": "Review", "order": 0 }, { "role": "doing", "name": "In progress", "order": 1 }] }),
            ok(
                json!([{ "role": "testing", "name": "Testing", "description": "Deploying, in canary, or in an experiment." }]),
            ),
        ),
        case(
            "presets/a-board-that-added-testing",
            "R7",
            json!({ "kind": "boardPresets", "customStates": [{ "role": "testing", "name": "Testing", "description": "Deploying, in canary, or in an experiment.", "order": 0 }] }),
            ok(json!([])),
        ),
        case(
            "presets/a-board-with-its-own-column-named-testing",
            "R7",
            json!({ "kind": "boardPresets", "customStates": [{ "role": "custom-testing", "name": "testing", "order": 0 }] }),
            ok(json!([])),
        ),
    ]
}

/// Whether `actual` says at least what `expected` says: every key of an expected object present
/// and matching, arrays and scalars equal.
fn contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|candidate| contains(candidate, value))
        }),
        _ => actual == expected,
    }
}

/// One pick of a list-defaults picker, as the door serialises it.
fn pick(value: Value, key: Value, is_selected: bool) -> Value {
    json!({ "value": value, "key": key, "isSelected": is_selected })
}

/// `listDefaultPicks` with only the fields a case cares about; the rest are absent, which is what
/// an unconfigured list sends.
fn defaults(chosen: Value) -> Value {
    let mut request = json!({ "kind": "listDefaultPicks" });
    if let (Value::Object(request), Value::Object(chosen)) = (&mut request, chosen) {
        request.extend(chosen);
    }
    request
}

/// The hours a native client offers for a list's default time, written out.
const QUICK_HOURS: [&str; 17] = [
    "06:00", "07:00", "08:00", "09:00", "10:00", "11:00", "12:00", "13:00", "14:00", "15:00",
    "16:00", "17:00", "18:00", "19:00", "20:00", "21:00", "22:00",
];

/// The time picker: all-day, then these times in this order, with `selected` ticked.
fn times(order: &[&str], selected: Option<&str>) -> Value {
    let mut rows = vec![pick(
        Value::Null,
        json!("defaults.all_day"),
        selected.is_none(),
    )];
    rows.extend(
        order
            .iter()
            .map(|time| pick(json!(time), Value::Null, selected == Some(*time))),
    );
    Value::Array(rows)
}

/// [`QUICK_HOURS`] with one more time, claimed to belong at `at` — the ordering each case asserts.
fn hours_plus(at: usize, time: &'static str) -> Vec<&'static str> {
    let mut order = QUICK_HOURS.to_vec();
    order.insert(at, time);
    order
}

/// What a list's defaults editor offers (task 92830d81).
///
/// `expect` spells every picker out, hours included: where a free-form time lands in clock order
/// is the claim worth writing down, and `at` is written rather than computed so the fixture
/// cannot agree with the rule by sharing its arithmetic.
fn list_default_picks_cases() -> Vec<Case> {
    let rule = "rows::list_default_picks";
    let priorities = |selected: i64| {
        json!([
            pick(json!("0"), json!("priority.none"), selected == 0),
            pick(json!("1"), json!("priority.low"), selected == 1),
            pick(json!("2"), json!("priority.medium"), selected == 2),
            pick(json!("3"), json!("priority.high"), selected == 3),
        ])
    };
    let repeats = |selected: &str| {
        json!([
            pick(
                json!("never"),
                json!("repeating.one_time_only"),
                selected == "never"
            ),
            pick(
                json!("daily"),
                json!("repeating.daily"),
                selected == "daily"
            ),
            pick(
                json!("weekly"),
                json!("repeating.weekly"),
                selected == "weekly"
            ),
            pick(
                json!("monthly"),
                json!("repeating.monthly"),
                selected == "monthly"
            ),
            pick(
                json!("yearly"),
                json!("repeating.yearly"),
                selected == "yearly"
            ),
        ])
    };
    let whens = |selected: &str| {
        json!([
            pick(
                json!("none"),
                json!("picker.no_due_date"),
                selected == "none"
            ),
            pick(json!("today"), json!("picker.today"), selected == "today"),
            pick(
                json!("tomorrow"),
                json!("picker.tomorrow"),
                selected == "tomorrow"
            ),
            pick(
                json!("next_week"),
                json!("picker.next_week"),
                selected == "next_week"
            ),
        ])
    };

    vec![
        // ── What is offered, and what an unconfigured list ticks ────────────────────────────
        case(
            "unset/a-list-nobody-has-configured",
            rule,
            defaults(json!({})),
            ok(json!({
                "priority": priorities(0),
                "repeating": repeats("never"),
                "dueDate": whens("none"),
                // `defaultDueTime` absent means all-day, and all-day leads the picker.
                "dueTime": times(&QUICK_HOURS, None),
            })),
        ),
        // ── The list's own defaults are the ones ticked ─────────────────────────────────────
        case(
            "selected/priority-and-repeat-and-when",
            rule,
            defaults(json!({ "priority": 3, "repeating": "weekly", "dueDate": "tomorrow" })),
            ok(json!({
                "priority": priorities(3),
                "repeating": repeats("weekly"),
                "dueDate": whens("tomorrow"),
            })),
        ),
        case(
            "selected/the-middle-of-each-picker",
            rule,
            defaults(json!({ "priority": 1, "repeating": "monthly", "dueDate": "today" })),
            ok(json!({
                "priority": priorities(1),
                "repeating": repeats("monthly"),
                "dueDate": whens("today"),
            })),
        ),
        // ── Unset and unrecognised are different ───────────────────────────────────────────
        case(
            "unrecognised/a-stored-value-the-picker-cannot-draw-ticks-nothing",
            rule,
            defaults(json!({ "priority": 9, "repeating": "custom", "dueDate": "next_month" })),
            ok(json!({
                "priority": priorities(-1),
                "repeating": repeats(""),
                "dueDate": whens(""),
            })),
        ),
        // ── The time picks ─────────────────────────────────────────────────────────────────
        case(
            "time/a-quick-hour-is-ticked-and-not-duplicated",
            rule,
            defaults(json!({ "dueTime": "14:00" })),
            ok(json!({ "dueTime": times(&QUICK_HOURS, Some("14:00")) })),
        ),
        case(
            "time/a-free-form-time-is-offered-in-clock-order",
            rule,
            defaults(json!({ "dueTime": "09:15" })),
            // The web's TimePicker can store any minute; 09:15 belongs after 09:00, which is the
            // fourth hour offered.
            ok(json!({ "dueTime": times(&hours_plus(4, "09:15"), Some("09:15")) })),
        ),
        case(
            "time/a-time-before-the-working-day",
            rule,
            defaults(json!({ "dueTime": "05:30" })),
            ok(json!({ "dueTime": times(&hours_plus(0, "05:30"), Some("05:30")) })),
        ),
        case(
            "time/a-time-after-the-working-day",
            rule,
            defaults(json!({ "dueTime": "23:45" })),
            ok(json!({ "dueTime": times(&hours_plus(17, "23:45"), Some("23:45")) })),
        ),
    ]
}

/// Runs every case through the door, checks it against what a person wrote down, and renders the
/// fixture.
fn render(cases: Vec<Case>, generated_from: &str, extra: Value) -> Result<String, String> {
    let mut recorded = Vec::new();
    for case in cases {
        let answer: Value =
            serde_json::from_str(&astrid_rules::rules::run_json(&case.request.to_string()))
                .map_err(|error| format!("{}: the door answered non-JSON: {error}", case.name))?;
        if !contains(&answer, &case.expect) {
            return Err(format!(
                "{}: expected {} but the rule answered {}",
                case.name, case.expect, answer
            ));
        }
        recorded.push(json!({
            "name": case.name,
            "rule": case.rule,
            "request": case.request,
            "answer": answer,
        }));
    }
    let mut fixture = json!({
        "$comment": "GENERATED by astrid-core `cargo xtask gen-core-contracts` — do not edit. Source of truth: astrid-core (a core-first contract; contracts/README.md).",
        "generatedFrom": generated_from,
        "door": "astrid_rules::rules::run_json — the WebAssembly build's runJson. Send `request`, expect `answer`.",
        "cases": recorded,
    });
    if let (Value::Object(fixture), Value::Object(extra)) = (&mut fixture, extra) {
        fixture.extend(extra);
    }
    serde_json::to_string_pretty(&fixture)
        .map(|text| text + "\n")
        .map_err(|error| error.to_string())
}

/// The text of `waiting.json`, or the first case whose answer is not what its `expect` says.
pub fn waiting_fixture() -> Result<String, String> {
    render(
        waiting_cases(),
        "crates/astrid-rules/src/waiting.rs, crates/astrid-rules/src/board.rs (presets)",
        json!({ "waitingNeedsCondition": "WAITING_NEEDS_CONDITION" }),
    )
}

/// The text of `list-default-picks.json` (task 92830d81).
pub fn list_default_picks_fixture() -> Result<String, String> {
    render(
        list_default_picks_cases(),
        "crates/astrid-rules/src/rows/list_default_picks.rs",
        json!({
            "canonicalSource": "astrid-web components/list-admin/DefaultTaskSettingsSection.tsx — its repeat and when options are inline SelectItems, so there is no table to export and the rule is core-first. Web's TimePicker is free-form, so the hours are the native clients'.",
        }),
    )
}

/// Every core-owned fixture, by file name, with its text.
pub fn generate_all() -> Result<Vec<(&'static str, String)>, String> {
    Ok(vec![
        ("waiting.json", waiting_fixture()?),
        ("list-default-picks.json", list_default_picks_fixture()?),
    ])
}

pub fn write_all(root: &Path) -> std::process::ExitCode {
    match generate_all() {
        Ok(fixtures) => {
            for (name, text) in fixtures {
                let path = root.join("contracts/fixtures").join(name);
                if let Err(error) = std::fs::write(&path, text) {
                    eprintln!("could not write {}: {error}", path.display());
                    return std::process::ExitCode::FAILURE;
                }
                println!("wrote contracts/fixtures/{name}");
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

pub fn check_all(root: &Path) -> std::process::ExitCode {
    match generate_all() {
        Ok(fixtures) => {
            let mut stale = 0;
            for (name, text) in fixtures {
                let path = root.join("contracts/fixtures").join(name);
                match std::fs::read(&path) {
                    Ok(held) if super::same_fixture(text.as_bytes(), &held) => {
                        println!("ok  contracts/fixtures/{name}");
                    }
                    _ => {
                        eprintln!("STALE  contracts/fixtures/{name} — run `cargo xtask gen-core-contracts`");
                        stale += 1;
                    }
                }
            }
            if stale == 0 {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed core-first fixtures are what the rules answer today — so `cargo test` fails
    /// on a rule change that was not regenerated, without anyone remembering a second command.
    #[test]
    fn the_core_owned_fixtures_are_current() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (name, text) in generate_all().expect("every case answers as expected") {
            let held = std::fs::read(root.join("contracts/fixtures").join(name))
                .unwrap_or_else(|_| panic!("contracts/fixtures/{name} is missing"));
            assert!(
                super::super::same_fixture(text.as_bytes(), &held),
                "contracts/fixtures/{name} is stale — run `cargo xtask gen-core-contracts`"
            );
        }
    }
}
