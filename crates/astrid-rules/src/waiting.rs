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
        });
    }

    if holds_after {
        return Ok(ResolvedStatus {
            status_role: Some(WAITING_STATUS_ROLE.to_string()),
            assignee: AssigneeWrite::Keep,
            transition: Transition::Stay,
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
        }
    } else {
        ResolvedStatus {
            status_role: None,
            assignee: AssigneeWrite::Keep,
            transition: Transition::Inbox,
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
}

/// R3 and R5 in one: what a scheduled job (or a blocker's completion) does to one task.
pub fn settle(task: &Task, blockers: &[Blocker], now: DateTime<Utc>) -> Settlement {
    let stay = Settlement {
        action: SettleAction::Stay,
        status_role: task.status_role.clone(),
        assignee: AssigneeWrite::Keep,
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
            },
            Wake::ResolveToInbox => Settlement {
                action: SettleAction::ResolveToInbox,
                status_role: None,
                assignee: AssigneeWrite::Keep,
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
        };
    }
    stay
}

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
