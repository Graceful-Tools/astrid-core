//! The Pending results lane: when a task may wait for an experiment or a verification, what
//! entering it does, and where it goes when the result is in (ADEV-274; D74 in
//! `docs/CONTRACTS.md`, design in `docs/GOAL_SIGNALS.md` §4).
//!
//! **Core-first**, like [`crate::waiting`], which this is modelled on: a write resolver and a settle
//! function, both pure, with `now` passed in. The caller passes the task's **result conditions** as
//! it passes blockers to Waiting. Fetching a result is I/O and belongs to the server; reading it is
//! this module.
//!
//! The column is a preset (`board::RESULTS_STATUS_ROLE`) that no board has until its owner adds it
//! (ADEV-286). A board without it never reaches these rules: experiments and verifications still
//! report on the task, and nothing moves by rule.
//!
//! ## P1 — entering needs something to wait for
//!
//! An open experiment, or an open verification whose `decideBy` is still ahead. Otherwise the write
//! is refused with [`RESULTS_NEEDS_CONDITION`], a client's cue to show the picker, as it reads
//! `WAITING_NEEDS_CONDITION`.
//!
//! ## P2 — entering unassigns
//!
//! And remembers the holder (`rememberAssigneeId`), as R2 and R8 do for Waiting: results are
//! nobody's work while they are pending.
//!
//! ## P3 — passed → Done
//!
//! Every condition concluded with verdict `improving`. A verification that ended with no guardrail
//! breached and no goal regressed reports `improving`. [`settle`] answers `complete`; the caller
//! completes through `TaskService::complete` (README rule 2).
//!
//! ## P4 — not passed → hand back
//!
//! To the escalation target ([`crate::waiting::hand_back`]: Inbox, open, assigned) when any
//! condition is `regressing`; or, once the latest `decideBy` has come, when any condition is still
//! open (its source stayed silent) or concluded `inconclusive`. The caller attaches the Decision
//! (iterate, roll back, ship anyway). A task in the lane with no condition at all is handed back
//! too: nothing is measuring it.
//!
//! ## P5 — not yet → stay
//!
//! Including a source that has gone stale before its deadline (`fetchedAt` is the server's, not
//! this rule's). A concluded condition with no `decideBy` has nothing left to wait for; an open one
//! with none waits for its source.
//!
//! ## P6 — leaving by hand is always allowed
//!
//! The conditions stay on the task. Nothing in these rules rolls anything back.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::board::RESULTS_STATUS_ROLE;
use crate::model::{date, Task};
use crate::waiting::{hand_back, HandBack, Transition};

/// The refusal code for a move into Pending results with nothing to wait for. The server answers
/// the same code.
pub const RESULTS_NEEDS_CONDITION: &str = "RESULTS_NEEDS_CONDITION";

/// What a result condition is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ConditionKind {
    Experiment,
    Verification,
}

/// Whether the source has concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ConditionState {
    Open,
    Concluded,
}

/// What a concluded source found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Improving,
    Regressing,
    Inconclusive,
}

/// One thing a task waits on for its result, as the server stores it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ResultCondition {
    pub kind: ConditionKind,
    pub state: ConditionState,
    /// What it found. Absent on a concluded condition reads as `inconclusive`.
    #[serde(default)]
    pub verdict: Option<Verdict>,
    /// When the owner wants the answer by.
    #[serde(default, deserialize_with = "optional_instant")]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<crate::model::date::Timestamp>")
    )]
    pub decide_by: Option<Timestamp>,
    /// When the server last heard from the source. Carried for the client's "stale" badge; no rule
    /// reads it (P5).
    #[serde(default, deserialize_with = "optional_instant")]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<crate::model::date::Timestamp>")
    )]
    pub fetched_at: Option<Timestamp>,
}

/// Why a write was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ResultsError {
    /// Into Pending results with nothing to wait for (P1).
    #[serde(rename = "RESULTS_NEEDS_CONDITION")]
    ResultsNeedsCondition,
}

impl ResultsError {
    pub fn code(self) -> &'static str {
        match self {
            ResultsError::ResultsNeedsCondition => RESULTS_NEEDS_CONDITION,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            ResultsError::ResultsNeedsCondition => {
                "Pending results needs an experiment or a verification to wait on"
            }
        }
    }
}

/// Whether a task is in the lane: the role, and not finished.
pub fn is_pending_results(task: &Task) -> bool {
    !task.completed && task.status_role.as_deref() == Some(RESULTS_STATUS_ROLE)
}

/// P1: whether one condition is something to wait for at `now`.
fn is_pending(condition: &ResultCondition, now: Timestamp) -> bool {
    condition.state == ConditionState::Open
        && match condition.kind {
            ConditionKind::Experiment => true,
            ConditionKind::Verification => condition.decide_by.is_some_and(|by| by > now),
        }
}

/// P1: whether the conditions hold a task in the lane at `now`.
pub fn has_result_condition(conditions: &[ResultCondition], now: Timestamp) -> bool {
    conditions
        .iter()
        .any(|condition| is_pending(condition, now))
}

/// What a write asks for, as far as the lane cares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ResultsChange {
    /// The lane asked for: a role, `null` for Inbox, absent for "not a move".
    #[serde(default, deserialize_with = "present")]
    pub status_role: Option<Option<String>>,
    /// Keep the assignee when entering. Default: clear it.
    #[serde(default)]
    pub keep_assignee: bool,
}

/// What a write does to the assignee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultsAssignee {
    Keep,
    Clear,
}

/// A write the lane accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedResults {
    pub status_role: Option<String>,
    pub assignee: ResultsAssignee,
    /// `enter` (P1, P2), `stay`, or `none` for a write that is not about the lane.
    pub transition: Transition,
    /// With `assignee: "clear"`: who held the task, for the server to remember (P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember_assignee_id: Option<String>,
}

/// P1/P2/P6: resolve a write that may move a task into, within or out of Pending results.
///
/// `task` is the task before the write; `conditions` are its result conditions after the write
/// (the caller applies any it adds in the same operation).
pub fn resolve_status_write(
    task: &Task,
    conditions: &[ResultCondition],
    change: &ResultsChange,
    now: Timestamp,
) -> Result<ResolvedResults, ResultsError> {
    let was_in = is_pending_results(task);
    let asked = match &change.status_role {
        Some(role) => role.clone().filter(|role| !role.is_empty()),
        None => task.status_role.clone(),
    };
    let to_lane = match &change.status_role {
        Some(_) => asked.as_deref() == Some(RESULTS_STATUS_ROLE),
        None => was_in,
    };
    let write = |status_role, assignee, transition, remember| ResolvedResults {
        status_role,
        assignee,
        transition,
        remember_assignee_id: remember,
    };
    if !to_lane {
        return Ok(write(asked, ResultsAssignee::Keep, Transition::None, None));
    }
    let role = Some(RESULTS_STATUS_ROLE.to_string());
    if was_in {
        return Ok(write(role, ResultsAssignee::Keep, Transition::Stay, None));
    }
    if !has_result_condition(conditions, now) {
        return Err(ResultsError::ResultsNeedsCondition);
    }
    let holder = task.assignee_id.clone().filter(|held| !held.is_empty());
    Ok(match holder {
        Some(holder) if !change.keep_assignee => write(
            role,
            ResultsAssignee::Clear,
            Transition::Enter,
            Some(holder),
        ),
        _ => write(role, ResultsAssignee::Keep, Transition::Enter, None),
    })
}

/// What [`settle`] does to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ResultsAction {
    /// Nothing to write.
    Stay,
    /// P3: complete it.
    Complete,
    /// P4: hand it back to the escalation target.
    HandBack,
}

/// Why a task was handed back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HandBackReason {
    /// A condition found a regression.
    Regressing,
    /// The deadline came and a source had nothing to say.
    Silent,
    /// The deadline came and a source could not tell.
    Inconclusive,
    /// Nothing is measuring the task.
    NoCondition,
}

/// The write that puts a task where its results say it belongs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultsSettlement {
    pub action: ResultsAction,
    /// With `handBack`: why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<HandBackReason>,
    /// With `handBack`: Inbox, open, assigned to the escalation target.
    #[serde(flatten)]
    pub hand_back: Option<HandBack>,
}

impl ResultsSettlement {
    fn of(action: ResultsAction) -> Self {
        ResultsSettlement {
            action,
            reason: None,
            hand_back: None,
        }
    }
}

fn passed(condition: &ResultCondition) -> bool {
    condition.state == ConditionState::Concluded && condition.verdict == Some(Verdict::Improving)
}

/// When the answer is due from one condition: its `decideBy`, else now for a concluded one (it has
/// nothing left to say) and never for an open one.
fn due(condition: &ResultCondition) -> Option<Option<Timestamp>> {
    match (condition.decide_by, condition.state) {
        (Some(by), _) => Some(Some(by)),
        (None, ConditionState::Concluded) => Some(None),
        (None, ConditionState::Open) => None,
    }
}

/// P3–P5: what a scheduled job (or a source reporting) does to one task. `escalate_to` is the
/// escalation target's assignee id.
pub fn settle(
    task: &Task,
    conditions: &[ResultCondition],
    escalate_to: &str,
    now: Timestamp,
) -> ResultsSettlement {
    if !is_pending_results(task) {
        return ResultsSettlement::of(ResultsAction::Stay);
    }
    let handed = |reason| ResultsSettlement {
        action: ResultsAction::HandBack,
        reason: Some(reason),
        hand_back: Some(hand_back(escalate_to)),
    };
    if conditions.is_empty() {
        return handed(HandBackReason::NoCondition);
    }
    if conditions
        .iter()
        .any(|condition| condition.verdict == Some(Verdict::Regressing))
    {
        return handed(HandBackReason::Regressing);
    }
    if conditions.iter().all(passed) {
        return ResultsSettlement::of(ResultsAction::Complete);
    }
    // Not passed, not regressing: wait until the latest answer is due, if every one has a day.
    let latest = conditions
        .iter()
        .map(due)
        .try_fold(None::<Timestamp>, |latest, due| {
            due.map(|due| latest.max(due))
        });
    match latest {
        Some(latest) if latest.is_none_or(|by| by <= now) => {
            let silent = conditions
                .iter()
                .any(|condition| condition.state == ConditionState::Open);
            handed(if silent {
                HandBackReason::Silent
            } else {
                HandBackReason::Inconclusive
            })
        }
        _ => ResultsSettlement::of(ResultsAction::Stay),
    }
}

/// A key that is present, even as `null`, decodes as `Some`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// An instant that may be absent or `null`. Unreadable is refused, never read as "no deadline".
fn optional_instant<'de, D>(deserializer: D) -> Result<Option<Timestamp>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        None => Ok(None),
        Some(text) if text.is_empty() => Ok(None),
        Some(text) => date::parse(&text)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("{text} is not an instant"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-11-25T12:00:00Z";

    fn at(text: &str) -> Timestamp {
        date::parse(text).expect("an instant")
    }

    fn task(role: Option<&str>, assignee: Option<&str>) -> Task {
        Task {
            status_role: role.map(str::to_string),
            assignee_id: assignee.map(str::to_string),
            ..Task::new("t", "t")
        }
    }

    fn cond(
        kind: ConditionKind,
        state: ConditionState,
        verdict: Option<Verdict>,
        decide_by: Option<&str>,
    ) -> ResultCondition {
        ResultCondition {
            kind,
            state,
            verdict,
            decide_by: decide_by.map(at),
            fetched_at: None,
        }
    }

    fn open_experiment() -> ResultCondition {
        cond(ConditionKind::Experiment, ConditionState::Open, None, None)
    }

    fn to_results() -> ResultsChange {
        ResultsChange {
            status_role: Some(Some(RESULTS_STATUS_ROLE.into())),
            ..Default::default()
        }
    }

    #[test]
    fn adev_274_entering_with_an_open_experiment_unassigns_and_remembers_the_holder() {
        let resolved = resolve_status_write(
            &task(Some("doing"), Some("agent")),
            &[open_experiment()],
            &to_results(),
            at(NOW),
        )
        .unwrap();
        assert_eq!(resolved.status_role.as_deref(), Some("results"));
        assert_eq!(resolved.assignee, ResultsAssignee::Clear);
        assert_eq!(resolved.transition, Transition::Enter);
        assert_eq!(resolved.remember_assignee_id.as_deref(), Some("agent"));
    }

    #[test]
    fn adev_274_entering_can_keep_the_assignee() {
        let change = ResultsChange {
            keep_assignee: true,
            ..to_results()
        };
        let resolved = resolve_status_write(
            &task(None, Some("jon")),
            &[open_experiment()],
            &change,
            at(NOW),
        )
        .unwrap();
        assert_eq!(resolved.assignee, ResultsAssignee::Keep);
        assert_eq!(resolved.remember_assignee_id, None);
    }

    #[test]
    fn adev_274_entering_needs_an_open_experiment_or_a_verification_with_its_deadline_ahead() {
        let enter = |conditions: &[ResultCondition]| {
            resolve_status_write(
                &task(Some("doing"), None),
                conditions,
                &to_results(),
                at(NOW),
            )
        };
        let refused = Err(ResultsError::ResultsNeedsCondition);
        assert_eq!(enter(&[]), refused);
        let concluded = cond(
            ConditionKind::Experiment,
            ConditionState::Concluded,
            Some(Verdict::Improving),
            None,
        );
        assert_eq!(enter(&[concluded]), refused);
        let verification =
            |by: Option<&str>| cond(ConditionKind::Verification, ConditionState::Open, None, by);
        assert!(enter(&[verification(Some("2026-11-26T00:00:00Z"))]).is_ok());
        assert_eq!(
            enter(&[verification(Some("2026-11-25T11:00:00Z"))]),
            refused
        );
        assert_eq!(enter(&[verification(Some(NOW))]), refused);
        assert_eq!(enter(&[verification(None)]), refused);
    }

    #[test]
    fn adev_274_leaving_by_hand_and_staying_are_never_refused() {
        let leave = ResultsChange {
            status_role: Some(Some("doing".into())),
            ..Default::default()
        };
        let resolved =
            resolve_status_write(&task(Some("results"), None), &[], &leave, at(NOW)).unwrap();
        assert_eq!(resolved.status_role.as_deref(), Some("doing"));
        assert_eq!(resolved.transition, Transition::None);
        let stay = resolve_status_write(
            &task(Some("results"), None),
            &[],
            &ResultsChange::default(),
            at(NOW),
        )
        .unwrap();
        assert_eq!(stay.transition, Transition::Stay);
    }

    fn pending(conditions: &[ResultCondition]) -> ResultsSettlement {
        settle(&task(Some("results"), None), conditions, "jon", at(NOW))
    }

    fn concluded(verdict: Option<Verdict>, by: Option<&str>) -> ResultCondition {
        cond(
            ConditionKind::Experiment,
            ConditionState::Concluded,
            verdict,
            by,
        )
    }

    #[test]
    fn adev_274_every_condition_passed_completes_the_task() {
        let settled = pending(&[
            concluded(Some(Verdict::Improving), None),
            concluded(Some(Verdict::Improving), Some("2026-12-01T00:00:00Z")),
        ]);
        assert_eq!(settled.action, ResultsAction::Complete);
    }

    #[test]
    fn adev_274_regressing_hands_back_to_the_escalation_target_at_once() {
        let settled = pending(&[
            concluded(Some(Verdict::Regressing), Some("2026-12-01T00:00:00Z")),
            open_experiment(),
        ]);
        assert_eq!(settled.action, ResultsAction::HandBack);
        assert_eq!(settled.reason, Some(HandBackReason::Regressing));
        let back = settled.hand_back.expect("a hand back");
        assert_eq!(back.assignee_id, "jon");
        assert_eq!(back.status_role, None);
        assert!(!back.completed);
    }

    #[test]
    fn adev_274_inconclusive_waits_for_its_deadline_then_hands_back() {
        let before = pending(&[concluded(
            Some(Verdict::Inconclusive),
            Some("2026-11-26T00:00:00Z"),
        )]);
        assert_eq!(before.action, ResultsAction::Stay);
        let after = pending(&[concluded(
            Some(Verdict::Inconclusive),
            Some("2026-11-25T12:00:00Z"),
        )]);
        assert_eq!(after.reason, Some(HandBackReason::Inconclusive));
        // No verdict on a concluded condition reads as inconclusive; no deadline means it is due.
        let unsaid = pending(&[concluded(None, None)]);
        assert_eq!(unsaid.reason, Some(HandBackReason::Inconclusive));
    }

    #[test]
    fn adev_274_a_silent_source_hands_back_at_the_deadline_and_not_before() {
        let open = |by: &str| {
            cond(
                ConditionKind::Verification,
                ConditionState::Open,
                None,
                Some(by),
            )
        };
        assert_eq!(
            pending(&[open("2026-11-25T12:30:00Z")]).action,
            ResultsAction::Stay
        );
        let late = pending(&[open("2026-11-25T11:00:00Z")]);
        assert_eq!(late.reason, Some(HandBackReason::Silent));
        // An experiment with no deadline waits for its source.
        assert_eq!(pending(&[open_experiment()]).action, ResultsAction::Stay);
    }

    #[test]
    fn adev_274_mixed_conditions_wait_for_the_latest_deadline() {
        let passed = concluded(Some(Verdict::Improving), None);
        let slow = cond(
            ConditionKind::Verification,
            ConditionState::Open,
            None,
            Some("2026-11-30T00:00:00Z"),
        );
        // One has passed, the other has until the 30th: stay.
        assert_eq!(pending(&[passed.clone(), slow]).action, ResultsAction::Stay);
        let inconclusive_early =
            concluded(Some(Verdict::Inconclusive), Some("2026-11-20T00:00:00Z"));
        let late = cond(
            ConditionKind::Verification,
            ConditionState::Open,
            None,
            Some("2026-11-30T00:00:00Z"),
        );
        // The early one is already inconclusive, but the late one is not due: stay.
        assert_eq!(
            pending(&[inconclusive_early, late]).action,
            ResultsAction::Stay
        );
    }

    #[test]
    fn adev_274_a_task_outside_the_lane_or_finished_is_left_alone() {
        let regressing = [concluded(Some(Verdict::Regressing), None)];
        let ready = settle(&task(Some("ready"), None), &regressing, "jon", at(NOW));
        assert_eq!(ready.action, ResultsAction::Stay);
        let done = Task {
            completed: true,
            ..task(Some("results"), None)
        };
        assert_eq!(
            settle(&done, &regressing, "jon", at(NOW)).action,
            ResultsAction::Stay
        );
    }

    #[test]
    fn adev_274_a_task_in_the_lane_with_nothing_measuring_it_is_handed_back() {
        let settled = pending(&[]);
        assert_eq!(settled.reason, Some(HandBackReason::NoCondition));
    }

    #[test]
    fn adev_274_a_condition_reads_absent_null_and_a_value_apart() {
        let condition: ResultCondition = serde_json::from_value(serde_json::json!({
            "kind": "verification", "state": "open", "decideBy": "2026-11-26T00:00:00.000Z",
            "fetchedAt": null
        }))
        .unwrap();
        assert_eq!(condition.decide_by, Some(at("2026-11-26T00:00:00Z")));
        assert_eq!(condition.fetched_at, None);
        assert!(
            serde_json::from_value::<ResultCondition>(serde_json::json!({
                "kind": "experiment", "state": "open", "decideBy": "soon"
            }))
            .is_err()
        );
    }
}
