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
/// generate. Most come from [`generate_all`]; `repeat-summary-l10n.json` is written by the ignored
/// `write_the_fixture_3564db06` test in `crates/astrid-rules/tests/repeat_summary_l10n.rs`
/// (task 3564db06), because the renderer it needs lives in that test.
pub const CORE_OWNED: &[&str] = &[
    "waiting.json",
    "list-default-picks.json",
    "board-rows.json",
    "list-project-moves.json",
    "repeat-summary-l10n.json",
    "callout-placement.json",
    "sidebar-projects.json",
    "orchestration.json",
    "questions.json",
];

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
        // ── R8: handing off (AWTD-1686) ────────────────────────────────────────────────────
        // The round trip: the plan, the half completing, the source waking to its holder.
        case(
            "handoff/doing-source-waits-unassigned-and-remembers-its-holder",
            "R8",
            json!({ "kind": "waitingHandoff", "source": { "id": "src", "statusRole": "doing", "assigneeId": "agent" }, "blockers": [], "now": NOW }),
            ok(json!({
                "half": { "statusRole": "ready", "originTaskId": "src" },
                "source": { "statusRole": "waiting", "assignee": "clear", "transition": "enter" },
                "returnAssigneeId": "agent"
            })),
        ),
        case(
            "handoff/ready-source-with-nobody-assigned-remembers-nobody",
            "R8",
            json!({ "kind": "waitingHandoff", "source": { "id": "src", "statusRole": "ready" }, "blockers": [], "now": NOW }),
            ok(json!({
                "half": { "statusRole": "ready", "originTaskId": "src" },
                "source": { "statusRole": "waiting", "assignee": "keep", "transition": "enter" },
                "returnAssigneeId": null
            })),
        ),
        case(
            "handoff/source-already-waiting-keeps-the-remembered-holder",
            "R8",
            json!({ "kind": "waitingHandoff", "source": { "id": "src", "statusRole": "waiting" }, "blockers": [{ "id": "half-1", "completed": false }], "returnAssigneeId": "agent", "now": NOW }),
            ok(json!({
                "source": { "statusRole": "waiting", "assignee": "keep", "transition": "stay" },
                "returnAssigneeId": "agent"
            })),
        ),
        case(
            "handoff/a-finished-source-is-refused",
            "R8",
            json!({ "kind": "waitingHandoff", "source": { "id": "src", "statusRole": "doing", "completed": true, "assigneeId": "agent" }, "blockers": [], "now": NOW }),
            json!({ "ok": false, "error": { "kind": "refused", "code": "HANDOFF_SOURCE_DONE" } }),
        ),
        case(
            "handoff/half-done-source-wakes-to-ready-back-with-its-holder",
            "R3/R8",
            json!({ "kind": "waitingSettle", "task": { "id": "src", "statusRole": "waiting" }, "blockers": [{ "id": "half", "completed": true }], "returnAssigneeId": "agent", "now": NOW }),
            ok(
                json!({ "action": "wakeToReady", "statusRole": "ready", "assignee": "return", "returnAssigneeId": "agent" }),
            ),
        ),
        case(
            "handoff/half-still-open-source-stays-and-returns-nobody",
            "R3/R8",
            json!({ "kind": "waitingSettle", "task": { "id": "src", "statusRole": "waiting" }, "blockers": [{ "id": "half", "completed": false }], "returnAssigneeId": "agent", "now": NOW }),
            ok(json!({ "action": "stay", "statusRole": "waiting", "assignee": "keep" })),
        ),
        case(
            "handoff/somebody-assigned-during-the-wait-wins",
            "R3/R8",
            json!({ "kind": "waitingSettle", "task": { "id": "src", "statusRole": "waiting", "assigneeId": "u-jon" }, "blockers": [{ "id": "half", "completed": true }], "returnAssigneeId": "agent", "now": NOW }),
            ok(json!({ "action": "wakeToReady", "statusRole": "ready", "assignee": "keep" })),
        ),
        case(
            "handoff/removing-the-half-wakes-the-source-back-to-its-holder",
            "R2/R8",
            json!({ "kind": "waitingWrite", "task": { "id": "src", "statusRole": "waiting" }, "blockers": [{ "id": "half", "completed": false }], "change": { "removeBlockerIds": ["half"] }, "returnAssigneeId": "agent", "now": NOW }),
            ok(
                json!({ "statusRole": "ready", "assignee": "return", "transition": "wake", "returnAssigneeId": "agent" }),
            ),
        ),
        case(
            "handoff/a-plain-wake-remembers-nobody",
            "R3/R8",
            settle(
                json!({ "id": "t", "statusRole": "waiting" }),
                json!([{ "id": "b1", "completed": true }]),
            ),
            ok(json!({ "action": "wakeToReady", "statusRole": "ready", "assignee": "keep" })),
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
/// and matching, arrays as long as expected with each element saying at least what its expected
/// one says, scalars equal.
fn contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|candidate| contains(candidate, value))
        }),
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(candidate, value)| contains(candidate, value))
        }
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

/// The lists of web's `tests/lib/project-board-rows-awtd-1660.test.ts`, plus the Mac test's
/// virtual list (`ProjectBoardRowsTests`), which *Add list* must never offer.
fn board_row_lists() -> Value {
    json!([
        { "id": "web", "name": "Web", "projectId": "p1" },
        { "id": "ios", "name": "iOS", "projectId": "p1" },
        { "id": "status-row", "name": "Ready", "projectId": "p1", "listType": "status" },
        { "id": "label", "name": "Bug", "projectId": "p1", "listType": "label" },
        { "id": "groceries", "name": "Groceries" },
        { "id": "other-project", "name": "Other", "projectId": "p2" },
        { "id": "smart", "name": "Smart", "isVirtual": true },
    ])
}

/// `boardRows` over [`board_row_lists`], with `saved` as the server's `listIds`.
fn board_rows(saved: Value) -> Value {
    json!({ "kind": "boardRows", "projectId": "p1", "savedListIds": saved, "lists": board_row_lists() })
}

/// web's swimlane test board: the three defaults and a Review column.
fn review_board() -> Value {
    json!([{ "role": "review", "name": "Review", "order": 3 }])
}

/// The cards of web's "puts each card in its list's row" test. Done is timed by `updatedAt` on the
/// native board and by `completedAt` on web's (D43), so the finished card carries both.
fn board_row_tasks() -> Value {
    json!([
        { "id": "a", "listIds": ["web"], "statusRole": "doing" },
        { "id": "b", "listIds": ["ios"], "statusRole": "review" },
        { "id": "c", "listIds": ["web"] },
        { "id": "d", "listIds": ["ios"], "completed": true,
          "completedAt": "2026-11-12T11:00:00.000Z", "updatedAt": "2026-11-12T11:00:00.000Z" },
        { "id": "e", "listIds": ["groceries"], "statusRole": "ready" },
        { "id": "shared", "listIds": ["web", "ios"], "statusRole": "ready" },
        { "id": "foreign", "listIds": ["groceries"], "statusRole": "custom-qa" },
    ])
}

/// `boardRow` for `row` under the review board.
fn board_row(row: &str, lists: Value) -> Value {
    json!({ "kind": "boardRow", "rowListId": row, "customStates": review_board(),
            "lists": lists, "tasks": board_row_tasks(), "now": NOW })
}

/// One answered column of `boardRow`: its id and its cards' ids.
fn cell(id: &str, task_ids: Value) -> Value {
    json!({ "id": id, "taskIds": task_ids })
}

/// What the multi-list project board draws (AWTD-1676; web AWTD-1660).
///
/// Web's TypeScript is canonical, so every `expect` here is one of web's own test assertions
/// (`tests/lib/project-board-rows-awtd-1660.test.ts`) or the Mac's (`ProjectBoardRowsTests`),
/// written out rather than computed. The last two cases are D67: a list with no board, and a role
/// the project has no column for.
fn board_rows_cases() -> Vec<Case> {
    let rule = "board_rows";
    let row_lists = || board_row_lists();
    vec![
        case(
            "rows/nothing-saved-is-the-projects-own-domain-lists",
            rule,
            board_rows(Value::Null),
            ok(json!({ "rowListIds": ["web", "ios"] })),
        ),
        case(
            "rows/saved-rows-draw-in-the-saved-order",
            rule,
            board_rows(json!(["groceries", "web"])),
            ok(json!({ "rowListIds": ["groceries", "web"] })),
        ),
        case(
            "rows/an-emptied-board-stays-empty",
            rule,
            board_rows(json!([])),
            ok(json!({ "rowListIds": [] })),
        ),
        case(
            "rows/unseen-repeated-and-non-domain-ids-are-dropped",
            rule,
            board_rows(json!(["gone", "web", "web", "label", "status-row"])),
            ok(json!({ "rowListIds": ["web"] })),
        ),
        case(
            "candidates/domain-lists-not-virtual-not-rows-by-name",
            rule,
            board_rows(json!(["web"])),
            ok(
                json!({ "rowListIds": ["web"], "candidateListIds": ["groceries", "ios", "other-project"] }),
            ),
        ),
        case(
            "row/each-card-in-its-lists-row-and-its-status-column",
            rule,
            board_row("web", row_lists()),
            ok(json!([
                cell("__virtual_inbox__", json!(["c"])),
                cell("ready", json!(["shared"])),
                cell("doing", json!(["a"])),
                cell("waiting", json!([])),
                cell("review", json!([])),
                cell("__virtual_done__", json!([])),
            ])),
        ),
        case(
            "row/a-card-on-two-row-lists-shows-in-both-and-done-keeps-recent-work",
            rule,
            board_row("ios", row_lists()),
            ok(json!([
                cell("__virtual_inbox__", json!([])),
                cell("ready", json!(["shared"])),
                cell("doing", json!([])),
                cell("waiting", json!([])),
                cell("review", json!(["b"])),
                cell("__virtual_done__", json!(["d"])),
            ])),
        ),
        case(
            "row/a-list-with-no-board-draws-its-cards-and-a-role-the-project-lacks-is-inbox",
            "board_rows (D67)",
            board_row("groceries", row_lists()),
            ok(json!([
                cell("__virtual_inbox__", json!(["foreign"])),
                cell("ready", json!(["e"])),
                cell("doing", json!([])),
                cell("waiting", json!([])),
                cell("review", json!([])),
                cell("__virtual_done__", json!([])),
            ])),
        ),
        case(
            "row/the-row-lists-manual-order-arranges-a-column",
            rule,
            board_row(
                "web",
                json!([{ "id": "web", "name": "Web", "projectId": "p1", "manualSortOrder": ["shared", "c"] }]),
            ),
            ok(json!([
                cell("__virtual_inbox__", json!(["c"])),
                cell("ready", json!(["shared"])),
                cell("doing", json!(["a"])),
                cell("waiting", json!([])),
                cell("review", json!([])),
                cell("__virtual_done__", json!([])),
            ])),
        ),
    ]
}

/// The projects every `listProjectMoves` case sees: one of each standing "me" can have.
fn move_projects() -> Value {
    json!([
        { "id": "mine", "name": "Alpha", "ownerId": "me" },
        { "id": "run", "name": "Beta", "ownerId": "boss", "members": [{ "userId": "me", "role": "admin" }] },
        { "id": "join", "name": "Gamma", "ownerId": "boss", "members": [{ "userId": "me", "role": "member" }] },
        { "id": "look", "name": "Delta", "ownerId": "boss", "members": [{ "userId": "me", "role": "viewer" }] },
        { "id": "away", "name": "Epsilon", "ownerId": "boss" },
    ])
}

/// A list "me" holds `role` on (`owner`, a membership role, or `none`), in `project`.
fn move_list(role: &str, project: Value) -> Value {
    let (owner, members) = match role {
        "owner" => ("me", json!([])),
        "none" => ("someone", json!([])),
        role => ("someone", json!([{ "userId": "me", "role": role }])),
    };
    json!({ "id": "l1", "name": "Groceries", "ownerId": owner, "projectId": project, "listMembers": members })
}

fn moves_request(list: Value) -> Value {
    json!({ "kind": "listProjectMoves", "userId": "me", "list": list, "projects": move_projects() })
}

fn moves_answer(can_leave: bool, destinations: Value) -> Value {
    ok(json!({
        "canLeave": can_leave,
        "destinations": destinations,
        "creatableIn": ["mine", "run", "join"],
    }))
}

/// Moving a list into or out of a project, and creating one in a project (astrid-ios AITD-680,
/// D68): owner/admin/member/viewer of the list by owner/admin/member/viewer/stranger of the
/// project. Core-first: no client had the rule, so the expectations are D68's table written out.
fn list_project_moves_cases() -> Vec<Case> {
    let rule = "list_project_moves (D68)";
    let none = json!([]);
    vec![
        case(
            "loose/list-owner-moves-into-projects-they-own-or-admin-by-name",
            rule,
            moves_request(move_list("owner", Value::Null)),
            moves_answer(false, json!(["mine", "run"])),
        ),
        case(
            "loose/list-admin-moves-a-list-they-do-not-own",
            rule,
            moves_request(move_list("admin", Value::Null)),
            moves_answer(false, json!(["mine", "run"])),
        ),
        case(
            "loose/list-member-moves-nothing",
            rule,
            moves_request(move_list("member", Value::Null)),
            moves_answer(false, none.clone()),
        ),
        case(
            "loose/list-viewer-moves-nothing",
            rule,
            moves_request(move_list("viewer", Value::Null)),
            moves_answer(false, none.clone()),
        ),
        case(
            "in-project/admin-of-the-project-left-may-detach-and-move-on-never-to-its-own",
            rule,
            moves_request(move_list("owner", json!("run"))),
            moves_answer(true, json!(["mine"])),
        ),
        case(
            "in-project/a-plain-member-of-the-project-left-can-neither-detach-nor-move-it",
            rule,
            moves_request(move_list("owner", json!("join"))),
            moves_answer(false, none.clone()),
        ),
        case(
            "in-project/a-viewer-of-the-project-left-cannot-move-it",
            rule,
            moves_request(move_list("owner", json!("look"))),
            moves_answer(false, none.clone()),
        ),
        case(
            "in-project/a-project-the-person-cannot-see-holds-the-list",
            rule,
            moves_request(move_list("owner", json!("unseen"))),
            moves_answer(false, none.clone()),
        ),
        case(
            "in-project/a-project-admin-who-is-nobody-on-the-list-cannot-carry-it-off",
            rule,
            moves_request(move_list("none", json!("run"))),
            moves_answer(false, none.clone()),
        ),
        case(
            "in-project/a-status-list-is-a-column-and-never-moves",
            rule,
            moves_request(json!({
                "id": "col", "name": "Ready", "ownerId": "me", "projectId": "mine",
                "listType": "status", "statusRole": "ready",
            })),
            moves_answer(false, none.clone()),
        ),
        case(
            "create/no-list-answers-only-where-a-list-may-be-made",
            rule,
            json!({ "kind": "listProjectMoves", "userId": "me", "projects": move_projects() }),
            moves_answer(false, none.clone()),
        ),
        case(
            "create/a-stranger-to-every-project-may-create-in-none",
            rule,
            json!({ "kind": "listProjectMoves", "userId": "stranger", "list": move_list("owner", Value::Null), "projects": move_projects() }),
            ok(json!({ "canLeave": false, "destinations": [], "creatableIn": [] })),
        ),
    ]
}

/// The lists every `sidebar` case sees: two projects' lists, a board column and a label inside
/// one, a favourite inside one, a list naming a project the reader cannot see, and a plain list.
fn sidebar_lists() -> Value {
    json!([
        { "id": "web", "name": "Web", "projectId": "astrid" },
        { "id": "api", "name": "api", "projectId": "astrid" },
        { "id": "starred", "name": "Roadmap", "projectId": "astrid",
          "isFavorite": true, "favoriteOrder": 0 },
        { "id": "doing", "name": "Doing", "projectId": "astrid", "listType": "status" },
        { "id": "bug", "name": "Bug", "projectId": "astrid", "listType": "label" },
        { "id": "garden", "name": "Garden", "projectId": "home" },
        { "id": "secret", "name": "Elsewhere", "projectId": "unseen" },
        { "id": "groceries", "name": "Groceries" },
    ])
}

/// The projects the reader can see: one with lists, one with a list, one with none, and one with
/// no name but a key. Astrid's nested `lists` is stale — it still claims Garden, which moved to
/// Home since the projects were read.
fn sidebar_projects() -> Value {
    json!([
        { "id": "home", "name": "home", "color": "#22c55e" },
        { "id": "astrid", "name": "Astrid", "color": "#ef4444",
          "lists": [{ "id": "web", "name": "Web" }, { "id": "garden", "name": "Garden" }] },
        { "id": "empty", "name": "Someday" },
        { "id": "keyed", "name": "", "key": "MOB" },
    ])
}

/// `sidebar` over [`sidebar_lists`], with `projects` as given (`null` is Project Mode off).
fn sidebar(lists: Value, projects: Value) -> Value {
    json!({ "kind": "sidebar", "lists": lists, "projects": projects })
}

/// A project row as the section draws it: its id, name and the ids of the lists under it.
fn project_row(id: &str, name: &str, list_ids: &[&str]) -> Value {
    let lists: Vec<Value> = list_ids.iter().map(|id| json!({ "id": id })).collect();
    json!({ "id": id, "name": name, "lists": lists })
}

/// The sidebar's Projects section (AWTD-1681, docs/CONTRACTS.md D70).
///
/// Web's `components/TaskManager/Sidebar/ProjectsSection.tsx` (AWTD-1660) is the behaviour; the
/// decisions it left implicit — which list belongs where when two sources disagree, the order of
/// the lists under a project — are settled in `rows::sidebar` and written out here.
fn sidebar_projects_cases() -> Vec<Case> {
    let rule = "sidebar_projects";
    vec![
        case(
            "mode-off/no-section-whatever-projects-exist",
            rule,
            sidebar(sidebar_lists(), Value::Null),
            ok(json!({ "projects": null })),
        ),
        case(
            "mode-on/no-projects-draws-no-section",
            rule,
            sidebar(sidebar_lists(), json!([])),
            ok(json!({ "projects": null })),
        ),
        case(
            "projects/every-visible-project-by-name-each-with-its-own-lists",
            rule,
            sidebar(sidebar_lists(), sidebar_projects()),
            ok(json!({ "projects": [
                project_row("astrid", "Astrid", &["api", "starred", "web"]),
                project_row("home", "home", &["garden"]),
                project_row("keyed", "MOB", &[]),
                project_row("empty", "Someday", &[]),
            ] })),
        ),
        case(
            "projects/a-project-with-no-lists-is-still-a-row",
            rule,
            sidebar(
                json!([{ "id": "groceries", "name": "Groceries" }]),
                json!([{ "id": "empty", "name": "Someday" }]),
            ),
            ok(
                json!({ "projects": [{ "id": "empty", "name": "Someday", "color": "#3b82f6", "lists": [] }] }),
            ),
        ),
        case(
            "lists/a-list-claimed-by-two-projects-sits-under-its-own-projectId-once",
            rule,
            sidebar(
                json!([
                    { "id": "garden", "name": "Garden", "projectId": "home" },
                    { "id": "garden", "name": "Garden", "projectId": "home" },
                ]),
                sidebar_projects(),
            ),
            ok(json!({ "projects": [
                project_row("astrid", "Astrid", &[]),
                project_row("home", "home", &["garden"]),
                project_row("keyed", "MOB", &[]),
                project_row("empty", "Someday", &[]),
            ] })),
        ),
        case(
            "favorites/a-favourite-in-a-project-is-in-favorites-and-under-the-project",
            rule,
            sidebar(
                sidebar_lists(),
                json!([{ "id": "astrid", "name": "Astrid" }]),
            ),
            ok(json!({
                "favorites": [{ "id": "starred" }],
                "projects": [project_row("astrid", "Astrid", &["api", "starred", "web"])],
            })),
        ),
        case(
            "lists/the-lists-section-keeps-project-lists-and-columns-labels-stay-out",
            rule,
            sidebar(sidebar_lists(), sidebar_projects()),
            ok(json!({ "lists": [
                { "id": "api" }, { "id": "secret" }, { "id": "garden" },
                { "id": "groceries" }, { "id": "web" },
            ] })),
        ),
        case(
            "badges/lists-under-a-project-carry-their-badge",
            rule,
            json!({ "kind": "sidebar", "lists": [{ "id": "web", "name": "Web", "projectId": "astrid", "taskCount": 90 }],
                    "badges": { "web": 4 }, "projects": [{ "id": "astrid", "name": "Astrid" }] }),
            ok(
                json!({ "projects": [{ "id": "astrid", "lists": [{ "id": "web", "taskCount": 4 }] }] }),
            ),
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
        json!({ "waitingNeedsCondition": "WAITING_NEEDS_CONDITION", "handoffSourceDone": "HANDOFF_SOURCE_DONE" }),
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

/// Where a task callout opens, and where its arrow points (task `011080d4`); it touches its
/// anchor (astrid-ios AITD-679).
///
/// A 1400x900 window throughout, with a 320-wide column and a 64-tall row in it, because the
/// numbers only mean anything relative to each other and one frame makes the table readable.
fn callout_placement_cases() -> Vec<Case> {
    let viewport = json!({ "left": 0.0, "top": 0.0, "width": 1400.0, "height": 900.0 });
    let ask = |left: f64, top: f64, height: f64, callout_h: f64| {
        json!({
            "kind": "calloutPlacement",
            "anchor": { "left": left, "top": top, "width": 320.0, "height": height },
            "viewport": viewport,
            "width": 420.0,
            "height": callout_h,
        })
    };

    vec![
        case(
            "side/room-on-the-right",
            "calloutPlacement",
            ask(40.0, 300.0, 64.0, 520.0),
            ok(json!({ "side": "right", "left": 360.0 })),
        ),
        case(
            "side/room-on-the-left",
            "calloutPlacement",
            ask(1000.0, 300.0, 64.0, 520.0),
            ok(json!({ "side": "left", "left": 580.0 })),
        ),
        case(
            "side/equal-room-opens-right",
            "calloutPlacement",
            ask(540.0, 300.0, 64.0, 520.0),
            ok(json!({ "side": "right" })),
        ),
        case(
            "arrow/points-at-the-rows-middle",
            "calloutPlacement",
            ask(40.0, 300.0, 64.0, 520.0),
            ok(json!({ "top": 72.0, "arrowOffset": 260.0 })),
        ),
        case(
            "arrow/row-near-the-top-walks-the-arrow-up",
            "calloutPlacement",
            ask(40.0, 10.0, 64.0, 520.0),
            ok(json!({ "top": 8.0, "arrowOffset": 34.0 })),
        ),
        case(
            "arrow/row-near-the-bottom-walks-it-down",
            "calloutPlacement",
            ask(40.0, 800.0, 64.0, 520.0),
            ok(json!({ "top": 372.0, "arrowOffset": 460.0 })),
        ),
        case(
            "arrow/kept-off-the-near-corner",
            "calloutPlacement",
            ask(40.0, 0.0, 24.0, 120.0),
            ok(json!({ "arrowOffset": 20.0 })),
        ),
        case(
            "arrow/kept-off-the-far-corner",
            "calloutPlacement",
            ask(40.0, 880.0, 64.0, 120.0),
            ok(json!({ "arrowOffset": 100.0 })),
        ),
    ]
}

pub fn callout_placement_fixture() -> Result<String, String> {
    render(
        callout_placement_cases(),
        "crates/astrid-rules/src/rows/callout.rs",
        json!({
            "canonicalSource": "astrid-core. Core-first: no client had this rule — Windows' board spliced detail into the column as another card rather than opening a callout at all (task 011080d4), and the Mac, iPad and Android tablet each compute their own. The caller measures the callout, its arrow included; this places it. The callout touches its anchor: its near edge, where the arrow tip is, sits on the anchor's edge (astrid-ios AITD-679), and the gutter is only the clearance from the viewport.",
            "coordinates": "Logical pixels in the window's space, y increasing downward. The caller has already divided by its scale factor.",
            "gutter": astrid_rules::rows::callout::GUTTER,
            "arrowMargin": astrid_rules::rows::callout::ARROW_MARGIN,
        }),
    )
}

/// The text of `board-rows.json` (AWTD-1676).
pub fn board_rows_fixture() -> Result<String, String> {
    render(
        board_rows_cases(),
        "crates/astrid-rules/src/board_rows.rs",
        json!({
            "canonicalSource": "astrid-web lib/project-board-rows.ts (resolveBoardRows, boardRowCandidates, buildSwimlanes) — the expectations are web's own test cases (tests/lib/project-board-rows-awtd-1660.test.ts). Generated here until web's exporter takes it over; docs/CONTRACTS.md D67.",
        }),
    )
}

/// The text of `list-project-moves.json` (astrid-ios AITD-680).
pub fn list_project_moves_fixture() -> Result<String, String> {
    render(
        list_project_moves_cases(),
        "crates/astrid-rules/src/list_project_moves.rs",
        json!({
            "canonicalSource": "astrid-core crates/astrid-rules/src/list_project_moves.rs — core-first; the expectations are docs/CONTRACTS.md D68's permission table written out. astrid-web's PUT /api/v1/lists/{id} and POST /api/v1/lists enforce the same table (AWTD-1702).",
        }),
    )
}

/// The text of `sidebar-projects.json` (AWTD-1681).
pub fn sidebar_projects_fixture() -> Result<String, String> {
    render(
        sidebar_projects_cases(),
        "crates/astrid-rules/src/rows/sidebar.rs",
        json!({
            "canonicalSource": "astrid-web components/TaskManager/Sidebar/ProjectsSection.tsx (AWTD-1660) is the behaviour; the decisions it leaves implicit are settled core-first in crates/astrid-rules/src/rows/sidebar.rs — docs/CONTRACTS.md D70.",
            "decisions": {
                "whichProjects": "Every project the reader can see (GET /api/v1/projects: owned or member), including one with no lists. `projects` is null when Project Mode is off or there are no projects; never [].",
                "order": "Projects by name (falling back to the key), case folded, then id. The project's order, the same for every reader. Lists under a project by name, case folded, then id.",
                "listsUnderAProject": "Domain lists (no board columns, labels, milestones, iterations) whose own projectId names the project. Never Project.lists (a nested copy, stale after a move) and never saved board rows. A list sits under at most one project; repeats collapse. A list naming a project the reader cannot see is only in Lists.",
                "favorites": "A favourited list in a project shows in Favorites AND under its project. The Lists section is unchanged: a project's non-favourite lists stay in it, as on web.",
                "archivedOrHidden": "The server's Project has no archived or hidden field, so every visible project is a row.",
            },
        }),
    )
}

/// Astrid's own boards as one stored project policy: astrid-web `docs/FIXALL_WORKFLOW.md` →
/// *Autonomy* (Jon, 2026-11-15). Merge and deploy on green gates, additive migrations ship,
/// destructive ones and anything outward-facing (a store submission, money) ask Jon.
fn astrid_policy() -> Value {
    json!({
        "autonomy": {
            "merge": "allowed",
            "deploy": "allowed",
            "migrate": { "additive": "allowed", "destructive": "ask" },
            "release": "ask",
            "spend": "ask"
        },
        "escalation": { "to": "people", "userIds": ["jon"] }
    })
}

fn orch(kind: &str, project: Value, list: Value, rest: Value) -> Value {
    let mut request = json!({ "kind": kind, "project": project, "list": list });
    if let (Value::Object(request), Value::Object(rest)) = (&mut request, rest) {
        request.extend(rest);
    }
    request
}

fn may_req(project: Value, list: Value, agent: &str, action: &str, migration: Value) -> Value {
    orch(
        "orchestrationMay",
        project,
        list,
        json!({ "agent": agent, "action": action, "migration": migration }),
    )
}

fn decided(decision: &str, reason: &str) -> Value {
    ok(json!({ "decision": decision, "mustAsk": decision == "ask", "reason": reason }))
}

fn orchestration_cases() -> Vec<Case> {
    let none = Value::Null;
    let astrid = astrid_policy;
    let merge_asks = || json!({ "autonomy": { "merge": "ask" } });
    let set = || json!({ "routing": { "agents": ["claude", "copilot"] } });
    let routed = || json!({ "routing": { "assignTo": "codex", "agents": ["claude"] } });
    let route = |project: Value, agent: &str| {
        orch(
            "orchestrationRoute",
            project,
            Value::Null,
            json!({ "agent": agent }),
        )
    };
    let escalate = |project: Value, creator: Value| {
        orch(
            "orchestrationEscalate",
            project,
            Value::Null,
            json!({ "listOwnerId": "owner", "taskCreatorId": creator }),
        )
    };
    vec![
        case(
            "resolve/nothing-stored-is-todays-behaviour",
            "orchestrationPolicy",
            orch("orchestrationPolicy", none.clone(), none.clone(), json!({})),
            ok(json!({
                "routing": { "assignTo": null, "agents": null },
                "autonomy": {
                    "merge": "never", "deploy": "never",
                    "migrate": { "additive": "never", "destructive": "never" },
                    "release": "never", "spend": "never"
                },
                "escalation": { "to": "listOwner" }
            })),
        ),
        case(
            "resolve/astrid-fixall-autonomy-as-a-project-policy",
            "orchestrationPolicy",
            orch("orchestrationPolicy", astrid(), none.clone(), json!({})),
            ok(json!({
                "routing": { "assignTo": null, "agents": null },
                "autonomy": astrid()["autonomy"],
                "escalation": astrid()["escalation"]
            })),
        ),
        case(
            "resolve/a-list-overrides-one-field-and-inherits-the-rest",
            "orchestrationPolicy",
            orch("orchestrationPolicy", astrid(), merge_asks(), json!({})),
            ok(json!({ "autonomy": { "merge": "ask", "deploy": "allowed" } })),
        ),
        case(
            "resolve/a-list-with-no-project-has-only-its-own",
            "orchestrationPolicy",
            orch("orchestrationPolicy", none.clone(), merge_asks(), json!({})),
            ok(json!({ "autonomy": { "merge": "ask", "deploy": "never" } })),
        ),
        case(
            "resolve/a-null-assign-to-overrides-the-projects-agent",
            "orchestrationPolicy",
            orch(
                "orchestrationPolicy",
                json!({ "routing": { "assignTo": "claude" } }),
                json!({ "routing": { "assignTo": null } }),
                json!({}),
            ),
            ok(json!({ "routing": { "assignTo": null } })),
        ),
        case(
            "may/default-never-merges",
            "orchestrationMay",
            may_req(none.clone(), none.clone(), "claude", "merge", none.clone()),
            decided("never", "policy"),
        ),
        case(
            "may/default-never-deploys",
            "orchestrationMay",
            may_req(none.clone(), none.clone(), "claude", "deploy", none.clone()),
            decided("never", "policy"),
        ),
        case(
            "may/astrid-merges",
            "orchestrationMay",
            may_req(astrid(), none.clone(), "claude", "merge", none.clone()),
            decided("allowed", "policy"),
        ),
        case(
            "may/astrid-deploys",
            "orchestrationMay",
            may_req(astrid(), none.clone(), "claude", "deploy", none.clone()),
            decided("allowed", "policy"),
        ),
        case(
            "may/astrid-ships-an-additive-migration",
            "orchestrationMay",
            may_req(
                astrid(),
                none.clone(),
                "claude",
                "migrate",
                json!("additive"),
            ),
            decided("allowed", "policy"),
        ),
        case(
            "may/astrid-asks-before-a-destructive-migration",
            "orchestrationMay",
            may_req(
                astrid(),
                none.clone(),
                "claude",
                "migrate",
                json!("destructive"),
            ),
            decided("ask", "policy"),
        ),
        case(
            "may/an-unclassified-migration-is-destructive",
            "orchestrationMay",
            may_req(astrid(), none.clone(), "claude", "migrate", none.clone()),
            decided("ask", "unclassifiedMigration"),
        ),
        case(
            "may/astrid-asks-before-a-store-release",
            "orchestrationMay",
            may_req(astrid(), none.clone(), "claude", "release", none.clone()),
            decided("ask", "policy"),
        ),
        case(
            "may/astrid-asks-before-spending",
            "orchestrationMay",
            may_req(astrid(), none.clone(), "claude", "spend", none.clone()),
            decided("ask", "policy"),
        ),
        case(
            "may/a-list-that-asks-to-merge-overrides-the-project",
            "orchestrationMay",
            may_req(astrid(), merge_asks(), "claude", "merge", none.clone()),
            decided("ask", "policy"),
        ),
        case(
            "may/a-destructive-migration-is-never-freer-than-an-additive-one",
            "orchestrationMay",
            may_req(
                json!({ "autonomy": { "migrate": { "additive": "never", "destructive": "allowed" } } }),
                none.clone(),
                "claude",
                "migrate",
                json!("destructive"),
            ),
            decided("never", "destructiveFloor"),
        ),
        case(
            "may/an-agent-the-list-does-not-admit-may-do-nothing",
            "orchestrationMay",
            may_req(set(), none.clone(), "codex", "merge", none.clone()),
            decided("never", "notAdmitted"),
        ),
        case(
            "route/default-any-agent-the-claim-decides",
            "orchestrationRoute",
            route(none.clone(), "codex"),
            ok(json!({ "take": true, "reason": "anyAdmittedAgent" })),
        ),
        case(
            "route/an-admitted-agent-takes",
            "orchestrationRoute",
            route(set(), "Copilot"),
            ok(json!({ "take": true, "reason": "anyAdmittedAgent" })),
        ),
        case(
            "route/an-agent-off-the-set-does-not",
            "orchestrationRoute",
            route(set(), "codex"),
            ok(json!({ "take": false, "reason": "notAdmitted" })),
        ),
        case(
            "route/the-routed-agent-takes-even-off-the-set",
            "orchestrationRoute",
            route(routed(), "codex"),
            ok(json!({ "take": true, "reason": "routed" })),
        ),
        case(
            "route/every-other-agent-leaves-it",
            "orchestrationRoute",
            route(routed(), "claude"),
            ok(json!({ "take": false, "reason": "routedElsewhere" })),
        ),
        case(
            "escalate/default-the-list-owner",
            "orchestrationEscalate",
            escalate(none.clone(), json!("ana")),
            ok(json!({
                "notify": ["owner"],
                "handBack": { "statusRole": null, "assigneeId": "owner", "completed": false }
            })),
        ),
        case(
            "escalate/astrid-asks-jon",
            "orchestrationEscalate",
            escalate(astrid(), json!("ana")),
            ok(json!({ "notify": ["jon"], "handBack": { "assigneeId": "jon" } })),
        ),
        case(
            "escalate/the-creator-or-else-the-owner",
            "orchestrationEscalate",
            escalate(
                json!({ "escalation": { "to": "taskCreator" } }),
                none.clone(),
            ),
            ok(json!({ "notify": ["owner"] })),
        ),
        case(
            "escalate/people-in-order-without-repeats",
            "orchestrationEscalate",
            escalate(
                json!({ "escalation": { "to": "people", "userIds": ["jon", "ana", "jon"] } }),
                none.clone(),
            ),
            ok(json!({ "notify": ["jon", "ana"], "handBack": { "assigneeId": "jon" } })),
        ),
        case(
            "escalate/nobody-named-falls-back-to-the-owner",
            "orchestrationEscalate",
            escalate(
                json!({ "escalation": { "to": "people", "userIds": [] } }),
                none.clone(),
            ),
            ok(json!({ "notify": ["owner"] })),
        ),
        case(
            "refused/an-unknown-autonomy-word",
            "orchestrationMay",
            may_req(
                json!({ "autonomy": { "merge": "sometimes" } }),
                none.clone(),
                "claude",
                "merge",
                none,
            ),
            json!({ "ok": false }),
        ),
    ]
}

/// The text of `orchestration.json` (AWTD-1685).
pub fn orchestration_fixture() -> Result<String, String> {
    render(
        orchestration_cases(),
        "crates/astrid-rules/src/orchestration.rs",
        json!({
            "canonicalSource": "astrid-core crates/astrid-rules/src/orchestration.rs — core-first (AWTD-1685); design in docs/ORCHESTRATION_POLICY.md, decision docs/CONTRACTS.md D71. astrid-web stores the policy on Project and the override on TaskList, and its server asks these rules wherever an agent acts.",
            "astridPolicy": astrid_policy(),
            "astridPolicySource": "astrid-web docs/FIXALL_WORKFLOW.md → Autonomy (Jon, 2026-11-15): additive migrations ship, destructive ones and anything outward-facing wait for Jon. `jon` stands for his user id.",
        }),
    )
}

fn question_json(answered: bool) -> Value {
    let mut question = json!({
        "taskId": "t", "askerId": "agent", "answererId": "jon", "questionCommentId": "c1",
        "resume": "re-run the gate", "returnStatusRole": "doing", "askedAt": "2026-11-16T09:00:00.000Z",
    });
    if answered {
        question["answeredAt"] = json!("2026-11-16T11:00:00.000Z");
        question["answerCommentId"] = json!("c2");
    }
    question
}

fn refused_with(code: &str) -> Value {
    json!({ "ok": false, "error": { "kind": "refused", "code": code } })
}

fn question_ask(task: Value, question: Value, ask: Value) -> Value {
    json!({ "kind": "questionAsk", "task": task, "question": question, "ask": ask, "now": NOW })
}

fn question_cases() -> Vec<Case> {
    let ask = json!({ "askerId": "agent", "answererId": "jon", "questionCommentId": "c1", "resume": "re-run the gate" });
    let answer = |question: Value, completed: bool, replier: &str| {
        json!({ "kind": "questionAnswer", "question": question, "taskCompleted": completed,
                "replierId": replier, "answerCommentId": "c2", "now": NOW })
    };
    vec![
        case(
            "ask/a-doing-task-goes-to-the-answerer-and-remembers-doing",
            "Q1",
            question_ask(
                json!({ "id": "t", "statusRole": "doing", "assigneeId": "agent" }),
                Value::Null,
                ask.clone(),
            ),
            ok(json!({
                "question": { "taskId": "t", "askerId": "agent", "answererId": "jon", "returnStatusRole": "doing" },
                "write": { "statusRole": null, "assigneeId": "jon", "completed": false }
            })),
        ),
        case(
            "ask/a-ready-task-remembers-ready",
            "Q1",
            question_ask(
                json!({ "id": "t", "statusRole": "ready" }),
                Value::Null,
                ask.clone(),
            ),
            ok(json!({ "question": { "returnStatusRole": "ready" } })),
        ),
        case(
            "ask/a-person-may-ask-an-agent",
            "Q1",
            question_ask(
                json!({ "id": "t", "statusRole": "doing" }),
                Value::Null,
                json!({ "askerId": "jon", "answererId": "agent", "questionCommentId": "c1" }),
            ),
            ok(json!({ "write": { "assigneeId": "agent", "statusRole": null } })),
        ),
        case(
            "ask/a-finished-task-is-refused",
            "Q1",
            question_ask(
                json!({ "id": "t", "completed": true }),
                Value::Null,
                ask.clone(),
            ),
            refused_with("QUESTION_TASK_DONE"),
        ),
        case(
            "ask/a-second-open-question-is-refused",
            "Q1",
            question_ask(
                json!({ "id": "t", "statusRole": "doing" }),
                question_json(false),
                ask.clone(),
            ),
            refused_with("QUESTION_ALREADY_OPEN"),
        ),
        case(
            "ask/an-answered-question-does-not-block-the-next",
            "Q1",
            question_ask(
                json!({ "id": "t", "statusRole": "doing" }),
                question_json(true),
                ask.clone(),
            ),
            ok(json!({ "write": { "assigneeId": "jon" } })),
        ),
        case(
            "ask/nobody-asks-themselves",
            "Q1",
            question_ask(
                json!({ "id": "t", "statusRole": "doing" }),
                Value::Null,
                json!({ "askerId": "agent", "answererId": "agent", "questionCommentId": "c1" }),
            ),
            refused_with("QUESTION_NEEDS_ANSWERER"),
        ),
        case(
            "ask/the-question-needs-its-comment",
            "Q1",
            question_ask(
                json!({ "id": "t", "statusRole": "doing" }),
                Value::Null,
                json!({ "askerId": "agent", "answererId": "jon", "questionCommentId": "" }),
            ),
            refused_with("QUESTION_NEEDS_COMMENT"),
        ),
        case(
            "answer/the-answerer-returns-the-task-to-the-asker-in-its-lane",
            "Q2",
            answer(question_json(false), false, "jon"),
            ok(json!({
                "question": { "answerCommentId": "c2", "answeredAt": "2026-11-12T12:00:00Z" },
                "write": { "statusRole": "doing", "assigneeId": "agent", "completed": false }
            })),
        ),
        case(
            "answer/someone-else-leaves-it-open",
            "Q2",
            answer(question_json(false), false, "ana"),
            refused_with("QUESTION_NOT_ANSWERER"),
        ),
        case(
            "answer/an-answered-question-is-not-answered-twice",
            "Q2",
            answer(question_json(true), false, "jon"),
            refused_with("QUESTION_NOT_OPEN"),
        ),
        case(
            "answer/a-finished-task-is-not-written-to",
            "Q2",
            answer(question_json(false), true, "jon"),
            ok(json!({ "question": { "answeredAt": "2026-11-12T12:00:00Z" }, "write": null })),
        ),
        case(
            "resume/the-asker-picks-the-answer-up",
            "Q3",
            json!({ "kind": "questionResume", "question": question_json(true), "now": NOW }),
            ok(json!({ "resumedAt": "2026-11-12T12:00:00Z", "answerCommentId": "c2" })),
        ),
        case(
            "resume/an-open-question-cannot-be-resumed",
            "Q3",
            json!({ "kind": "questionResume", "question": question_json(false), "now": NOW }),
            refused_with("QUESTION_NOT_ANSWERED"),
        ),
        case(
            "attention/the-answerer-sees-open-questions-oldest-first",
            "Q4",
            json!({ "kind": "questionAttention", "personId": "jon", "questions": [
                { "taskId": "newer", "askerId": "agent", "answererId": "jon", "questionCommentId": "c", "returnStatusRole": "ready", "askedAt": "2026-11-16T10:00:00.000Z" },
                { "taskId": "older", "askerId": "agent", "answererId": "jon", "questionCommentId": "c", "returnStatusRole": "ready", "askedAt": "2026-11-15T10:00:00.000Z" },
                { "taskId": "other", "askerId": "agent", "answererId": "ana", "questionCommentId": "c", "returnStatusRole": "ready", "askedAt": "2026-11-14T10:00:00.000Z" }
            ] }),
            ok(json!({ "waitingOnMe": ["older", "newer"], "toResume": [] })),
        ),
        case(
            "attention/the-asker-sees-answers-it-has-not-resumed",
            "Q4",
            json!({ "kind": "questionAttention", "personId": "agent", "questions": [
                { "taskId": "answered", "askerId": "agent", "answererId": "jon", "questionCommentId": "c", "returnStatusRole": "ready", "askedAt": "2026-11-15T10:00:00.000Z", "answeredAt": "2026-11-16T10:00:00.000Z" },
                { "taskId": "resumed", "askerId": "agent", "answererId": "jon", "questionCommentId": "c", "returnStatusRole": "ready", "askedAt": "2026-11-15T10:00:00.000Z", "answeredAt": "2026-11-16T10:00:00.000Z", "resumedAt": "2026-11-16T11:00:00.000Z" },
                { "taskId": "open", "askerId": "agent", "answererId": "jon", "questionCommentId": "c", "returnStatusRole": "ready", "askedAt": "2026-11-15T10:00:00.000Z" }
            ] }),
            ok(json!({ "waitingOnMe": [], "toResume": ["answered"] })),
        ),
    ]
}

/// The text of `questions.json` (AWTD-1687).
pub fn questions_fixture() -> Result<String, String> {
    render(
        question_cases(),
        "crates/astrid-rules/src/questions.rs",
        json!({
            "canonicalSource": "astrid-core crates/astrid-rules/src/questions.rs — core-first (AWTD-1687). An open question to a person is a recorded fact on the task; the server stores a Question and asks these rules to ask, answer, resume and list.",
        }),
    )
}

/// Every core-owned fixture, by file name, with its text.
pub fn generate_all() -> Result<Vec<(&'static str, String)>, String> {
    Ok(vec![
        ("waiting.json", waiting_fixture()?),
        ("list-default-picks.json", list_default_picks_fixture()?),
        ("callout-placement.json", callout_placement_fixture()?),
        ("board-rows.json", board_rows_fixture()?),
        ("list-project-moves.json", list_project_moves_fixture()?),
        ("sidebar-projects.json", sidebar_projects_fixture()?),
        ("orchestration.json", orchestration_fixture()?),
        ("questions.json", questions_fixture()?),
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

    /// Every fixture generated in this repository is listed as core-owned, or `check-contracts`
    /// reports it as ORPHANED because astrid-web does not generate it (red on main since aea2d63).
    #[test]
    fn every_fixture_core_generates_is_core_owned() {
        for (name, _) in generate_all().expect("every case answers as expected") {
            assert!(
                CORE_OWNED.contains(&name),
                "{name} is generated here but not in CORE_OWNED"
            );
        }
        assert!(
            CORE_OWNED.contains(&"repeat-summary-l10n.json"),
            "repeat-summary-l10n.json is written by astrid-rules' repeat_summary_l10n test, not web"
        );
    }

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
