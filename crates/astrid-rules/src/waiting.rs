//! The Waiting lane: when a task may be in it, what entering it does, and when it leaves by itself.
//!
//! **Canonical here** (Jon, 2026-11-12). Web's server, its scheduled job, the native apps and every
//! agent script call these rules rather than spelling them; web's TypeScript, where it keeps one, is
//! a mirror locked by `contracts/fixtures/waiting.json`, which is generated **from this crate**
//! (`cargo xtask gen-core-contracts`) — the first core-first contract (`contracts/README.md`).
//!
//! ## R1 — what Waiting means
//!
//! A task may be in Waiting (`statusRole: "waiting"`, not completed) only while it has a **wait
//! condition**: a due date strictly in the future, or at least one blocking task that is not
//! complete. Both may hold; nothing else counts — no free-text external condition, no deploy marker.
//!
//! ## R2 — entering
//!
//! A move into Waiting from any other lane needs a condition, supplied in the same write or already
//! on the task; without one it is refused with [`WAITING_NEEDS_CONDITION`], which is a client's cue
//! to show the date / task picker. Entering clears the assignee unless the mover asks to keep it. A
//! task already in Waiting is not entering: an edit that removes its last condition wakes it (R3)
//! rather than being refused.
//!
//! ## R3 — waking
//!
//! A Waiting task whose condition is gone — every blocker complete (or none) and the date reached
//! (or none) — goes to Ready, with whatever assignee it has. One with no condition at all (legacy
//! data) goes to Inbox, keeping its assignee.
//!
//! ## R4 — waiting on a person is not Waiting
//!
//! It is Inbox, assigned to that person: [`hand_back`].
//!
//! ## R5 — Ready with a future date
//!
//! Is moved to Waiting, the date being its condition, and — since that is entering — unassigned.
//! [`settle`] answers R3 and R5 together, so a scheduled job runs one function per task.
//!
//! ## R8 — handing off (AWTD-1686)
//!
//! A handoff files the other half of a task on another list and makes the source wait for it, in
//! one step: [`handoff`]. The half is born in Ready, with the source as its origin. The source
//! enters Waiting with the half as its blocker and is unassigned, as any entry is (R2) — but the
//! plan names who held it (`returnAssigneeId`), and the server remembers that. When the source
//! leaves Waiting by rule (R3) with nobody assigned, it goes back to that holder:
//! [`resolve_status_write_returning`] and [`settle_returning`] answer `assignee: "return"` with the
//! id. Somebody assigned during the wait wins over the remembered holder. A plain move into
//! Waiting remembers nobody and still unassigns. A finished task cannot be handed off
//! ([`HANDOFF_SOURCE_DONE`]).
//!
//! ## "Due to start"
//!
//! Ported from astrid-web `lib/ready-queue-scope.ts` `isDueToStart`: a task with no (readable) date
//! is due now, and one with a date is due once `due <= now`. An all-day date is stored at midnight
//! UTC (`model::date`), so an all-day task becomes due at the start of its day with no special case.
//! A date counts as a wait condition exactly when the task is *not* due to start.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::board::{READY_STATUS_ROLE, WAITING_STATUS_ROLE};
use crate::model::{date, Task};

/// The refusal code for a move into Waiting with no wait condition. The server answers the same
/// code, so a client branches on one spelling whichever side refused.
pub const WAITING_NEEDS_CONDITION: &str = "WAITING_NEEDS_CONDITION";

/// The refusal code for a handoff whose source is already finished (R8).
pub const HANDOFF_SOURCE_DONE: &str = "HANDOFF_SOURCE_DONE";

/// One task this task waits on, and whether it is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    pub id: String,
    #[serde(default)]
    pub completed: bool,
}

impl Blocker {
    pub fn new(id: impl Into<String>, completed: bool) -> Self {
        Blocker {
            id: id.into(),
            completed,
        }
    }
}

/// What a write asks for, as far as the Waiting rules care.
///
/// Every `Option<Option<_>>` reads: `None` — absent, unchanged; `Some(None)` — cleared;
/// `Some(Some(_))` — set. On the wire that is a missing key, `null`, and a value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct StatusChange {
    /// The lane asked for: a role, `null` for Inbox, absent for "not a move".
    #[serde(default, deserialize_with = "present")]
    pub status_role: Option<Option<String>>,
    /// A due date written in the same operation.
    #[serde(default, deserialize_with = "present_instant")]
    pub due_date_time: Option<Option<DateTime<Utc>>>,
    /// Blockers attached in the same operation.
    #[serde(default)]
    pub add_blockers: Vec<Blocker>,
    /// Blockers detached in the same operation, by id.
    #[serde(default)]
    pub remove_blocker_ids: Vec<String>,
    /// Keep the assignee when entering Waiting. Default: clear it.
    #[serde(default)]
    pub keep_assignee: bool,
}

/// What a write does to the assignee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssigneeWrite {
    /// Leave it as it is (including "there is none").
    Keep,
    /// Write `assigneeId: null`.
    Clear,
    /// Write `assigneeId` back to the holder a handoff remembered (R8) — the answer's
    /// `returnAssigneeId`.
    Return,
}

/// Which Waiting rule a resolved write followed — for a toast, a log, a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Transition {
    /// Neither into, within nor out of Waiting by rule: the write lands as asked.
    None,
    /// Into Waiting from another lane (R2).
    Enter,
    /// Already in Waiting, and still has a condition.
    Stay,
    /// Already in Waiting; the write removed its last condition, so it wakes to Ready (R3).
    Wake,
    /// Already in Waiting with no condition before or after — legacy — so it goes to Inbox (R3).
    Inbox,
}

/// A write the Waiting rules accept: the role to store and what happens to the assignee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedStatus {
    pub status_role: Option<String>,
    pub assignee: AssigneeWrite,
    pub transition: Transition,
    /// With `assignee: "return"`: who the task goes back to (R8). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_assignee_id: Option<String>,
}

/// Why a write was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WaitingError {
    /// Into Waiting with neither a future date nor an incomplete blocker (R2).
    #[serde(rename = "WAITING_NEEDS_CONDITION")]
    WaitingNeedsCondition,
}

impl WaitingError {
    pub fn code(self) -> &'static str {
        match self {
            WaitingError::WaitingNeedsCondition => WAITING_NEEDS_CONDITION,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            WaitingError::WaitingNeedsCondition => "Waiting needs a date or a task to wait on",
        }
    }
}

/// Whether a task may start: no date, or its date has come (`isDueToStart`).
pub fn is_due_to_start(due: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    due.is_none_or(|due| due <= now)
}

/// R1: whether a date and a set of blockers hold a task in Waiting at `now`.
pub fn has_wait_condition(
    due: Option<DateTime<Utc>>,
    blockers: &[Blocker],
    now: DateTime<Utc>,
) -> bool {
    !is_due_to_start(due, now) || blockers.iter().any(|blocker| !blocker.completed)
}

/// Whether a task is in the Waiting lane: the role, and not finished (Done has no status).
pub fn is_waiting(task: &Task) -> bool {
    !task.completed && task.status_role.as_deref() == Some(WAITING_STATUS_ROLE)
}

/// The blockers after a change: the removed ones gone, the added ones in (an added id replaces a
/// held one, so its completion is the caller's latest word).
pub fn blockers_after(blockers: &[Blocker], change: &StatusChange) -> Vec<Blocker> {
    let mut after: Vec<Blocker> = blockers
        .iter()
        .filter(|held| {
            !change.remove_blocker_ids.contains(&held.id)
                && !change.add_blockers.iter().any(|added| added.id == held.id)
        })
        .cloned()
        .collect();
    after.extend(change.add_blockers.iter().cloned());
    after
}

/// R1/R2: resolve a write that may move a task into, within or out of Waiting.
///
/// `task` is the task before the write; `blockers` its blockers before the write, each with its
/// completion; `change` what the write asks for. A write that is not about Waiting at all lands as
/// asked (`Transition::None`).
pub fn resolve_status_write(
    task: &Task,
    blockers: &[Blocker],
    change: &StatusChange,
    now: DateTime<Utc>,
) -> Result<ResolvedStatus, WaitingError> {
    let was_waiting = is_waiting(task);
    let asked = match &change.status_role {
        Some(role) => role.clone().filter(|role| !role.is_empty()),
        None => task.status_role.clone(),
    };
    let to_waiting = match &change.status_role {
        Some(_) => asked.as_deref() == Some(WAITING_STATUS_ROLE),
        None => was_waiting,
    };
    if !to_waiting {
        return Ok(ResolvedStatus {
            status_role: asked,
            assignee: AssigneeWrite::Keep,
            transition: Transition::None,
            return_assignee_id: None,
        });
    }

    let due_after = change.due_date_time.unwrap_or(task.due_date_time);
    let blockers_after = blockers_after(blockers, change);
    let holds_after = has_wait_condition(due_after, &blockers_after, now);

    if !was_waiting {
        if !holds_after {
            return Err(WaitingError::WaitingNeedsCondition);
        }
        let clear = !change.keep_assignee && task.assignee_id.is_some();
        return Ok(ResolvedStatus {
            status_role: Some(WAITING_STATUS_ROLE.to_string()),
            assignee: if clear {
                AssigneeWrite::Clear
            } else {
                AssigneeWrite::Keep
            },
            transition: Transition::Enter,
            return_assignee_id: None,
        });
    }

    if holds_after {
        return Ok(ResolvedStatus {
            status_role: Some(WAITING_STATUS_ROLE.to_string()),
            assignee: AssigneeWrite::Keep,
            transition: Transition::Stay,
            return_assignee_id: None,
        });
    }
    // The write took away the last condition the task had: it wakes, whatever is left of it.
    let held_before = has_wait_condition(task.due_date_time, blockers, now);
    let wakes = held_before || due_after.is_some() || !blockers_after.is_empty();
    Ok(if wakes {
        ResolvedStatus {
            status_role: Some(READY_STATUS_ROLE.to_string()),
            assignee: AssigneeWrite::Keep,
            transition: Transition::Wake,
            return_assignee_id: None,
        }
    } else {
        ResolvedStatus {
            status_role: None,
            assignee: AssigneeWrite::Keep,
            transition: Transition::Inbox,
            return_assignee_id: None,
        }
    })
}

/// What [`wake`] decided for a Waiting task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Wake {
    /// Still waiting (or not in Waiting at all).
    Stay,
    /// Its condition is gone: to Ready, assignee kept.
    WakeToReady,
    /// It never had a condition (legacy): to Inbox, assignee kept.
    ResolveToInbox,
}

/// R3: does a Waiting task leave by itself? `blockers` are its blockers now, with completion.
pub fn wake(task: &Task, blockers: &[Blocker], now: DateTime<Utc>) -> Wake {
    if !is_waiting(task) || has_wait_condition(task.due_date_time, blockers, now) {
        return Wake::Stay;
    }
    if task.due_date_time.is_some() || !blockers.is_empty() {
        Wake::WakeToReady
    } else {
        Wake::ResolveToInbox
    }
}

/// What [`settle`] does to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettleAction {
    /// Nothing to write.
    Stay,
    /// R3: Waiting → Ready.
    WakeToReady,
    /// R3: Waiting with no condition → Inbox.
    ResolveToInbox,
    /// R5: Ready with a future date → Waiting, unassigned.
    ParkInWaiting,
}

/// The lane a task belongs in now, and the write that puts it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settlement {
    pub action: SettleAction,
    /// The role the task should carry afterwards (its current one for `stay`).
    pub status_role: Option<String>,
    pub assignee: AssigneeWrite,
    /// With `assignee: "return"`: who the task goes back to (R8). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_assignee_id: Option<String>,
}

/// R3 and R5 in one: what a scheduled job (or a blocker's completion) does to one task.
pub fn settle(task: &Task, blockers: &[Blocker], now: DateTime<Utc>) -> Settlement {
    let stay = Settlement {
        action: SettleAction::Stay,
        status_role: task.status_role.clone(),
        assignee: AssigneeWrite::Keep,
        return_assignee_id: None,
    };
    if task.completed {
        return stay;
    }
    if is_waiting(task) {
        return match wake(task, blockers, now) {
            Wake::Stay => stay,
            Wake::WakeToReady => Settlement {
                action: SettleAction::WakeToReady,
                status_role: Some(READY_STATUS_ROLE.to_string()),
                assignee: AssigneeWrite::Keep,
                return_assignee_id: None,
            },
            Wake::ResolveToInbox => Settlement {
                action: SettleAction::ResolveToInbox,
                status_role: None,
                assignee: AssigneeWrite::Keep,
                return_assignee_id: None,
            },
        };
    }
    if task.status_role.as_deref() == Some(READY_STATUS_ROLE)
        && !is_due_to_start(task.due_date_time, now)
    {
        return Settlement {
            action: SettleAction::ParkInWaiting,
            status_role: Some(WAITING_STATUS_ROLE.to_string()),
            assignee: if task.assignee_id.is_some() {
                AssigneeWrite::Clear
            } else {
                AssigneeWrite::Keep
            },
            return_assignee_id: None,
        };
    }
    stay
}

/// R8: who a handed-off task goes back to as it leaves Waiting by rule — the remembered holder,
/// unless somebody has been assigned since (their assignment wins) or nobody was remembered.
fn holder_to_return(task: &Task, return_to: Option<&str>) -> Option<String> {
    if task
        .assignee_id
        .as_deref()
        .is_some_and(|held| !held.is_empty())
    {
        return None;
    }
    return_to
        .filter(|holder| !holder.is_empty())
        .map(str::to_string)
}

/// R1/R2/R8: [`resolve_status_write`] for a task a handoff left waiting, `return_to` being the
/// holder the server remembered. A write that wakes it (R3) gives it back to that holder.
pub fn resolve_status_write_returning(
    task: &Task,
    blockers: &[Blocker],
    change: &StatusChange,
    return_to: Option<&str>,
    now: DateTime<Utc>,
) -> Result<ResolvedStatus, WaitingError> {
    let mut resolved = resolve_status_write(task, blockers, change, now)?;
    if matches!(resolved.transition, Transition::Wake | Transition::Inbox) {
        if let Some(holder) = holder_to_return(task, return_to) {
            resolved.assignee = AssigneeWrite::Return;
            resolved.return_assignee_id = Some(holder);
        }
    }
    Ok(resolved)
}

/// R3/R5/R8: [`settle`] for a task a handoff left waiting, `return_to` being the holder the server
/// remembered. Waking (to Ready, or legacy to Inbox) gives it back to that holder.
pub fn settle_returning(
    task: &Task,
    blockers: &[Blocker],
    return_to: Option<&str>,
    now: DateTime<Utc>,
) -> Settlement {
    let mut settlement = settle(task, blockers, now);
    if matches!(
        settlement.action,
        SettleAction::WakeToReady | SettleAction::ResolveToInbox
    ) {
        if let Some(holder) = holder_to_return(task, return_to) {
            settlement.assignee = AssigneeWrite::Return;
            settlement.return_assignee_id = Some(holder);
        }
    }
    settlement
}

/// R8: the other half of a handoff, as it is born.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffHalf {
    /// Always Ready: the target list's agent or person picks it up as actionable work.
    pub status_role: String,
    /// The task it was handed off from, which waits on it.
    pub origin_task_id: String,
}

/// R8: what one handoff writes — the half to create, the source's write, and who the source goes
/// back to when the half is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub half: HandoffHalf,
    /// The source's write, the half being its new open blocker: `enter` (unassigned, R2), or
    /// `stay` for a source already in Waiting.
    pub source: ResolvedStatus,
    /// Who held the source: its assignee, or — for a source already waiting unassigned — the
    /// holder an earlier handoff remembered. `null` when nobody did.
    pub return_assignee_id: Option<String>,
}

/// Why a handoff was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HandoffError {
    /// The source is finished: there is nothing left to wait for the half (R8).
    #[serde(rename = "HANDOFF_SOURCE_DONE")]
    SourceDone,
}

impl HandoffError {
    pub fn code(self) -> &'static str {
        match self {
            HandoffError::SourceDone => HANDOFF_SOURCE_DONE,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            HandoffError::SourceDone => "A finished task cannot be handed off",
        }
    }
}

/// R8: plan a handoff of `source` (as it is now, with its `blockers`). `held_return` is the holder
/// an earlier handoff remembered for it, if any. The half is not created yet, so it has no id: the
/// plan holds for whatever id the server gives it, an open blocker being a wait condition by R1.
pub fn handoff(
    source: &Task,
    blockers: &[Blocker],
    held_return: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Handoff, HandoffError> {
    if source.completed {
        return Err(HandoffError::SourceDone);
    }
    let holder = source
        .assignee_id
        .clone()
        .filter(|held| !held.is_empty())
        .or_else(|| {
            held_return
                .filter(|held| !held.is_empty())
                .map(str::to_string)
        });
    let change = StatusChange {
        status_role: Some(Some(WAITING_STATUS_ROLE.to_string())),
        add_blockers: vec![Blocker::new(PENDING_HALF_ID, false)],
        ..Default::default()
    };
    // Cannot be refused: the half is an open blocker. Spelled out rather than unwrapped.
    let source_write =
        resolve_status_write(source, blockers, &change, now).unwrap_or_else(|_| ResolvedStatus {
            status_role: Some(WAITING_STATUS_ROLE.to_string()),
            assignee: AssigneeWrite::Clear,
            transition: Transition::Enter,
            return_assignee_id: None,
        });
    Ok(Handoff {
        half: HandoffHalf {
            status_role: READY_STATUS_ROLE.to_string(),
            origin_task_id: source.id.clone(),
        },
        source: source_write,
        return_assignee_id: holder,
    })
}

/// The stand-in id of a half not created yet, inside [`handoff`]'s own reasoning only.
const PENDING_HALF_ID: &str = "\u{0}pending-half";

/// R4: what handing a task back to a person writes — Inbox, open, assigned to them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandBack {
    pub status_role: Option<String>,
    pub assignee_id: String,
    pub completed: bool,
}

/// R4: "waiting on this person to act" is Inbox assigned to them, never Waiting.
pub fn hand_back(person_id: &str) -> HandBack {
    HandBack {
        status_role: None,
        assignee_id: person_id.to_string(),
        completed: false,
    }
}

/// A key that is present, even as `null`, decodes as `Some`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// [`present`] for an instant. Unlike a model's dates, an unreadable date in a *write* is refused:
/// read leniently it would be "clear the date", which is not what anybody sent.
fn present_instant<'de, D>(deserializer: D) -> Result<Option<Option<DateTime<Utc>>>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        None => Ok(Some(None)),
        Some(text) if text.is_empty() => Ok(Some(None)),
        Some(text) => date::parse(&text)
            .map(|at| Some(Some(at)))
            .ok_or_else(|| serde::de::Error::custom(format!("{text} is not an instant"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        date::parse(text).expect("an instant")
    }

    const NOW: &str = "2026-11-12T12:00:00Z";

    fn task(role: Option<&str>, due: Option<&str>, assignee: Option<&str>) -> Task {
        Task {
            status_role: role.map(str::to_string),
            due_date_time: due.map(at),
            assignee_id: assignee.map(str::to_string),
            ..Task::new("t", "t")
        }
    }

    fn to_waiting() -> StatusChange {
        StatusChange {
            status_role: Some(Some(WAITING_STATUS_ROLE.into())),
            ..Default::default()
        }
    }

    #[test]
    fn entering_with_a_future_date_clears_the_assignee() {
        let resolved = resolve_status_write(
            &task(Some("ready"), Some("2026-11-13T00:00:00Z"), Some("agent")),
            &[],
            &to_waiting(),
            at(NOW),
        )
        .unwrap();
        assert_eq!(resolved.status_role.as_deref(), Some("waiting"));
        assert_eq!(resolved.assignee, AssigneeWrite::Clear);
        assert_eq!(resolved.transition, Transition::Enter);
    }

    #[test]
    fn entering_can_keep_the_assignee() {
        let change = StatusChange {
            keep_assignee: true,
            ..to_waiting()
        };
        let resolved = resolve_status_write(
            &task(None, None, Some("jon")),
            &[Blocker::new("b", false)],
            &change,
            at(NOW),
        )
        .unwrap();
        assert_eq!(resolved.assignee, AssigneeWrite::Keep);
    }

    #[test]
    fn entering_with_no_condition_is_refused() {
        let refused = |due: Option<&str>, blockers: &[Blocker]| {
            resolve_status_write(
                &task(Some("doing"), due, None),
                blockers,
                &to_waiting(),
                at(NOW),
            )
        };
        assert_eq!(refused(None, &[]), Err(WaitingError::WaitingNeedsCondition));
        assert_eq!(
            refused(Some("2026-11-12T11:00:00Z"), &[]),
            Err(WaitingError::WaitingNeedsCondition)
        );
        // Exactly now is due, not future.
        assert_eq!(
            refused(Some(NOW), &[]),
            Err(WaitingError::WaitingNeedsCondition)
        );
        assert_eq!(
            refused(None, &[Blocker::new("b", true)]),
            Err(WaitingError::WaitingNeedsCondition)
        );
    }

    #[test]
    fn a_condition_supplied_in_the_same_write_counts() {
        let with_date = StatusChange {
            due_date_time: Some(Some(at("2026-12-01T00:00:00Z"))),
            ..to_waiting()
        };
        assert!(resolve_status_write(&task(None, None, None), &[], &with_date, at(NOW)).is_ok());
        let with_blocker = StatusChange {
            add_blockers: vec![Blocker::new("b", false)],
            ..to_waiting()
        };
        assert!(resolve_status_write(&task(None, None, None), &[], &with_blocker, at(NOW)).is_ok());
    }

    #[test]
    fn clearing_the_last_condition_inside_waiting_wakes_rather_than_refuses() {
        let waiting = task(Some("waiting"), Some("2026-11-20T00:00:00Z"), None);
        let clear_date = StatusChange {
            due_date_time: Some(None),
            ..Default::default()
        };
        let resolved = resolve_status_write(&waiting, &[], &clear_date, at(NOW)).unwrap();
        assert_eq!(resolved.status_role.as_deref(), Some("ready"));
        assert_eq!(resolved.transition, Transition::Wake);

        let blocked = task(Some("waiting"), None, None);
        let remove = StatusChange {
            remove_blocker_ids: vec!["b".into()],
            ..Default::default()
        };
        let resolved =
            resolve_status_write(&blocked, &[Blocker::new("b", false)], &remove, at(NOW)).unwrap();
        assert_eq!(resolved.transition, Transition::Wake);
    }

    #[test]
    fn a_move_out_of_waiting_is_not_the_waiting_rules_business() {
        let resolved = resolve_status_write(
            &task(Some("waiting"), None, Some("jon")),
            &[],
            &StatusChange {
                status_role: Some(Some("doing".into())),
                ..Default::default()
            },
            at(NOW),
        )
        .unwrap();
        assert_eq!(resolved.status_role.as_deref(), Some("doing"));
        assert_eq!(resolved.transition, Transition::None);
    }

    #[test]
    fn wake_follows_the_date_and_the_blockers() {
        let now = at(NOW);
        let dated = task(Some("waiting"), Some("2026-11-12T00:00:00Z"), None);
        assert_eq!(wake(&dated, &[], now), Wake::WakeToReady);
        assert_eq!(wake(&dated, &[Blocker::new("b", false)], now), Wake::Stay);
        let blocked = task(Some("waiting"), None, None);
        assert_eq!(
            wake(&blocked, &[Blocker::new("b", true)], now),
            Wake::WakeToReady
        );
        assert_eq!(wake(&blocked, &[], now), Wake::ResolveToInbox);
        let future = task(Some("waiting"), Some("2026-11-13T00:00:00Z"), None);
        assert_eq!(wake(&future, &[Blocker::new("b", true)], now), Wake::Stay);
    }

    #[test]
    fn settle_parks_a_dated_ready_task_unassigned() {
        let settled = settle(
            &task(Some("ready"), Some("2026-11-13T00:00:00Z"), Some("agent")),
            &[],
            at(NOW),
        );
        assert_eq!(settled.action, SettleAction::ParkInWaiting);
        assert_eq!(settled.assignee, AssigneeWrite::Clear);
        let due = settle(&task(Some("ready"), Some(NOW), Some("agent")), &[], at(NOW));
        assert_eq!(due.action, SettleAction::Stay);
    }

    #[test]
    fn handoff_awtd_1686_parks_the_source_unassigned_and_remembers_its_holder() {
        let plan = handoff(
            &task(Some("doing"), None, Some("agent")),
            &[],
            None,
            at(NOW),
        )
        .unwrap();
        assert_eq!(plan.half.status_role, "ready");
        assert_eq!(plan.half.origin_task_id, "t");
        assert_eq!(plan.source.status_role.as_deref(), Some("waiting"));
        assert_eq!(plan.source.assignee, AssigneeWrite::Clear);
        assert_eq!(plan.source.transition, Transition::Enter);
        assert_eq!(plan.return_assignee_id.as_deref(), Some("agent"));
    }

    #[test]
    fn handoff_awtd_1686_of_a_waiting_source_keeps_the_remembered_holder() {
        let plan = handoff(
            &task(Some("waiting"), None, None),
            &[Blocker::new("b", false)],
            Some("agent"),
            at(NOW),
        )
        .unwrap();
        assert_eq!(plan.source.transition, Transition::Stay);
        assert_eq!(plan.return_assignee_id.as_deref(), Some("agent"));
    }

    #[test]
    fn handoff_awtd_1686_of_a_finished_task_is_refused() {
        let done = Task {
            completed: true,
            ..task(Some("ready"), None, Some("agent"))
        };
        assert_eq!(
            handoff(&done, &[], None, at(NOW)),
            Err(HandoffError::SourceDone)
        );
    }

    #[test]
    fn awtd_1686_the_half_completing_gives_the_source_back_to_its_holder() {
        let waiting = task(Some("waiting"), None, None);
        let settled = settle_returning(
            &waiting,
            &[Blocker::new("half", true)],
            Some("agent"),
            at(NOW),
        );
        assert_eq!(settled.action, SettleAction::WakeToReady);
        assert_eq!(settled.assignee, AssigneeWrite::Return);
        assert_eq!(settled.return_assignee_id.as_deref(), Some("agent"));
        // Still held: nothing returns.
        let held = settle_returning(
            &waiting,
            &[Blocker::new("half", false)],
            Some("agent"),
            at(NOW),
        );
        assert_eq!(held.assignee, AssigneeWrite::Keep);
        assert_eq!(held.return_assignee_id, None);
    }

    #[test]
    fn awtd_1686_somebody_assigned_during_the_wait_wins() {
        let settled = settle_returning(
            &task(Some("waiting"), None, Some("jon")),
            &[Blocker::new("half", true)],
            Some("agent"),
            at(NOW),
        );
        assert_eq!(settled.assignee, AssigneeWrite::Keep);
        assert_eq!(settled.return_assignee_id, None);
    }

    #[test]
    fn awtd_1686_removing_the_half_wakes_the_source_back_to_its_holder() {
        let resolved = resolve_status_write_returning(
            &task(Some("waiting"), None, None),
            &[Blocker::new("half", false)],
            &StatusChange {
                remove_blocker_ids: vec!["half".into()],
                ..Default::default()
            },
            Some("agent"),
            at(NOW),
        )
        .unwrap();
        assert_eq!(resolved.transition, Transition::Wake);
        assert_eq!(resolved.assignee, AssigneeWrite::Return);
        assert_eq!(resolved.return_assignee_id.as_deref(), Some("agent"));
    }

    #[test]
    fn a_hand_back_is_inbox_assigned_to_the_person() {
        assert_eq!(
            hand_back("jon"),
            HandBack {
                status_role: None,
                assignee_id: "jon".into(),
                completed: false
            }
        );
    }

    #[test]
    fn a_change_reads_absent_null_and_a_value_apart() {
        let change: StatusChange = serde_json::from_value(serde_json::json!({
            "statusRole": null, "dueDateTime": "2026-11-13T00:00:00.000Z"
        }))
        .unwrap();
        assert_eq!(change.status_role, Some(None));
        assert_eq!(change.due_date_time, Some(Some(at("2026-11-13T00:00:00Z"))));
        let empty: StatusChange = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(empty.status_role, None);
        assert_eq!(empty.due_date_time, None);
        assert!(serde_json::from_value::<StatusChange>(
            serde_json::json!({ "dueDateTime": "tomorrow" })
        )
        .is_err());
    }
}
