//! The orchestration policy: who takes a task, what an agent may do on its own, and who it asks.
//!
//! **Canonical here** (AWTD-1685), the way [`crate::waiting`] is. Until this, the answers lived in
//! prompt text, env flags and repo scripts — astrid-web's generated skill hard-codes "Never merge or
//! deploy" (`lib/agent-skill/loop-recipes.ts`), and Astrid's own loops follow
//! `docs/FIXALL_WORKFLOW.md` → *Autonomy*. No developer using Astrid could see or change them. The
//! design, and the decisions it leaves open, are in `docs/ORCHESTRATION_POLICY.md`; the contract is
//! `contracts/fixtures/orchestration.json`, generated from this module
//! (`cargo xtask gen-core-contracts`).
//!
//! ## O1 — one policy per project, overridable per list
//!
//! A project stores a [`PolicyOverride`] and each list may store another. [`resolve`] lays them
//! over [`Policy::default`] field by field — default, then the project's, then the list's — so a
//! list that only says `autonomy.merge` inherits everything else. A list with no project has only
//! its own. Nothing stored means the default.
//!
//! ## O2 — the default is today's behaviour
//!
//! Every action is `never` (a person merges, deploys, migrates, releases and spends), any agent may
//! claim an unassigned Ready task (first claim wins, as `includeUnassigned` does), and questions go
//! to the list's owner. So nothing changes until someone edits a policy.
//!
//! ## O3 — routing
//!
//! [`route`]: may this agent take an unassigned Ready task on this list? `assignTo` names the one
//! agent that does; without it any agent in `agents` (or any agent at all, when `agents` is unset)
//! may, and the claim decides between them. The routed agent may always work the list.
//!
//! ## O4 — autonomy
//!
//! [`may`]: for an action class, one of [`Autonomy::Allowed`], [`Autonomy::Ask`] (post the
//! approval request to the escalation target and wait) or [`Autonomy::Never`] (a person does it).
//! Two floors, whatever a policy stores:
//!
//! - **A destructive migration is never more autonomous than an additive one.** Its answer is the
//!   stricter of the two.
//! - **A migration whose kind the caller does not state is destructive.** An unclassified change
//!   is treated as the one that cannot be undone.
//!
//! An agent the list does not admit (O3) is answered `never` for every action.
//!
//! ## O5 — escalation
//!
//! [`escalate`]: who receives a question or an approval request. The first person named is who the
//! task is handed back to ([`crate::waiting::hand_back`]: Inbox, assigned to them); every person
//! named is notified. A target that names nobody falls back to the list owner, so a question always
//! reaches someone.

use serde::{Deserialize, Deserializer, Serialize};

use crate::tradeoff::{Label, Level, TradeoffOverride, TradeoffPolicy};
use crate::waiting::{hand_back, HandBack};

/// What an agent may do about one action class. Ordered from most to least autonomous, so the
/// stricter of two is the greater.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum Autonomy {
    /// Do it.
    Allowed,
    /// Ask the escalation target first, and do it once a person approves.
    Ask,
    /// Never; a person does it.
    Never,
}

impl Autonomy {
    /// The stricter of two answers.
    pub fn stricter(self, other: Autonomy) -> Autonomy {
        self.max(other)
    }
}

/// The action classes a policy decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Merge a pull request.
    Merge,
    /// Deploy to production.
    Deploy,
    /// Apply a database migration; its [`MigrationKind`] decides which answer applies.
    Migrate,
    /// Submit a release (an App Store or Microsoft Store submission, a signed build).
    Release,
    /// Spend money or act as a person outside the repositories.
    Spend,
    /// Move a `do-now` idea from the Inbox to Ready (ADEV-273).
    PromoteIdea,
    /// Call an experiment service's `start`.
    ExperimentStart,
    /// Call an experiment service's `stop`.
    ExperimentStop,
}

/// What a migration does to existing data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum MigrationKind {
    /// A new table, a nullable column, an index: nothing existing changes.
    Additive,
    /// `DROP`, `RENAME`, `SET NOT NULL`, `UPDATE`, `DELETE`, `TRUNCATE`: applied before the new
    /// code is live, and a redeploy cannot undo it.
    Destructive,
}

/// Who takes unassigned Ready work on a list (O3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Routing {
    /// The one agent (by mailbox, e.g. `claude`) that takes an unassigned Ready task. `null`: any
    /// admitted agent, and the claim decides.
    #[serde(default)]
    pub assign_to: Option<String>,
    /// The agents (by mailbox) that may work this list. `null`: any agent.
    #[serde(default)]
    pub agents: Option<Vec<String>>,
}

/// The two migration answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationAutonomy {
    pub additive: Autonomy,
    pub destructive: Autonomy,
}

/// An answer per action class (O4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct AutonomyPolicy {
    pub merge: Autonomy,
    pub deploy: Autonomy,
    pub migrate: MigrationAutonomy,
    pub release: Autonomy,
    pub spend: Autonomy,
    pub promote_idea: Autonomy,
    pub experiment_start: Autonomy,
    pub experiment_stop: Autonomy,
}

/// Who receives a question or an approval request (O5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "to", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Escalation {
    /// The owner of the list the task is on.
    ListOwner,
    /// Whoever created the task; the list owner when that is unknown.
    TaskCreator,
    /// These people (by user id), in order; the first is handed the task.
    People { user_ids: Vec<String> },
}

/// A policy with every field decided: what [`resolve`] answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub routing: Routing,
    pub autonomy: AutonomyPolicy,
    pub escalation: Escalation,
    /// How an estimate is labelled and ordered. Absent when no layer stores one: no policy, so no
    /// label is drawn (ADEV-273, [`crate::tradeoff`] T1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tradeoff: Option<TradeoffPolicy>,
}

impl Default for Policy {
    /// O2: today's behaviour.
    fn default() -> Self {
        Policy {
            routing: Routing::default(),
            autonomy: AutonomyPolicy {
                merge: Autonomy::Never,
                deploy: Autonomy::Never,
                migrate: MigrationAutonomy {
                    additive: Autonomy::Never,
                    destructive: Autonomy::Never,
                },
                release: Autonomy::Never,
                spend: Autonomy::Never,
                promote_idea: Autonomy::Never,
                experiment_start: Autonomy::Never,
                experiment_stop: Autonomy::Never,
            },
            escalation: Escalation::ListOwner,
            tradeoff: None,
        }
    }
}

/// What a project or a list stores: any subset of a [`Policy`]. An absent key inherits.
///
/// For the two routing fields `null` is a value — "any agent" — so it overrides an inherited one;
/// only an absent key inherits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PolicyOverride {
    #[serde(default, skip_serializing_if = "is_default")]
    pub routing: RoutingOverride,
    #[serde(default, skip_serializing_if = "is_default")]
    pub autonomy: AutonomyOverride,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<Escalation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tradeoff: Option<TradeoffOverride>,
}

/// [`Routing`], each field absent (inherit), `null` (any) or set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RoutingOverride {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub assign_to: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub agents: Option<Option<Vec<String>>>,
}

/// [`MigrationAutonomy`], each field optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MigrationOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additive: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive: Option<Autonomy>,
}

/// [`AutonomyPolicy`], each field optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct AutonomyOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub migrate: MigrationOverride,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promote_idea: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_start: Option<Autonomy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_stop: Option<Autonomy>,
}

impl Policy {
    /// This policy with `layer` laid over it, field by field.
    pub fn with(mut self, layer: &PolicyOverride) -> Policy {
        if let Some(assign_to) = &layer.routing.assign_to {
            self.routing.assign_to = assign_to.clone();
        }
        if let Some(agents) = &layer.routing.agents {
            self.routing.agents = agents.clone();
        }
        let (a, o) = (&mut self.autonomy, &layer.autonomy);
        set(&mut a.merge, o.merge);
        set(&mut a.deploy, o.deploy);
        set(&mut a.migrate.additive, o.migrate.additive);
        set(&mut a.migrate.destructive, o.migrate.destructive);
        set(&mut a.release, o.release);
        set(&mut a.spend, o.spend);
        set(&mut a.promote_idea, o.promote_idea);
        set(&mut a.experiment_start, o.experiment_start);
        set(&mut a.experiment_stop, o.experiment_stop);
        if let Some(escalation) = &layer.escalation {
            self.escalation = escalation.clone();
        }
        if let Some(tradeoff) = &layer.tradeoff {
            self.tradeoff = Some(self.tradeoff.take().unwrap_or_default().with(tradeoff));
        }
        self
    }

    /// Whether `agent` may work this list at all (O3): the routed agent always may.
    pub fn admits(&self, agent: &str) -> bool {
        let routed = self
            .routing
            .assign_to
            .as_deref()
            .is_some_and(|routed| same_agent(routed, agent));
        routed
            || match &self.routing.agents {
                None => true,
                Some(agents) => agents.iter().any(|admitted| same_agent(admitted, agent)),
            }
    }
}

fn set(field: &mut Autonomy, value: Option<Autonomy>) {
    if let Some(value) = value {
        *field = value;
    }
}

/// Mailboxes are compared without regard to case or surrounding space: `Claude` is `claude`.
fn same_agent(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// O1: the effective policy for a list — the default, then the project's, then the list's.
pub fn resolve(project: Option<&PolicyOverride>, list: Option<&PolicyOverride>) -> Policy {
    let mut policy = Policy::default();
    for layer in [project, list].into_iter().flatten() {
        policy = policy.with(layer);
    }
    policy
}

/// Why [`route`] answered as it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteReason {
    /// This agent is the one `assignTo` names.
    Routed,
    /// `assignTo` names another agent.
    RoutedElsewhere,
    /// No agent is named, and this one is admitted: the claim decides.
    AnyAdmittedAgent,
    /// The list does not admit this agent.
    NotAdmitted,
}

/// What [`route`] answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteAnswer {
    pub take: bool,
    pub reason: RouteReason,
}

/// O3: may `agent` take an unassigned Ready task on a list with this policy?
pub fn route(policy: &Policy, agent: &str) -> RouteAnswer {
    let (take, reason) = match policy.routing.assign_to.as_deref() {
        Some(routed) if same_agent(routed, agent) => (true, RouteReason::Routed),
        Some(_) => (false, RouteReason::RoutedElsewhere),
        None if policy.admits(agent) => (true, RouteReason::AnyAdmittedAgent),
        None => (false, RouteReason::NotAdmitted),
    };
    RouteAnswer { take, reason }
}

/// Why [`may`] answered as it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MayReason {
    /// The policy's answer for the action.
    Policy,
    /// A destructive migration, held to the additive answer when that is stricter.
    DestructiveFloor,
    /// A migration whose kind was not stated, so answered as destructive.
    UnclassifiedMigration,
    /// The list does not admit this agent.
    NotAdmitted,
    /// `PromoteIdea` with no estimate to act on.
    NoEstimate,
    /// `PromoteIdea` on an estimate whose label is not `do-now`.
    NotDoNow,
    /// `PromoteIdea` on an estimate less confident than `minConfidenceToAct`.
    BelowConfidence,
}

/// What [`may`] answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MayAnswer {
    pub decision: Autonomy,
    /// Whether the agent must ask a person first: `decision == "ask"`.
    pub must_ask: bool,
    pub reason: MayReason,
}

/// The estimate an agent acts on, as [`may`] reads it: the label it earned and how confident the
/// estimator was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstimateRef {
    pub label: Label,
    pub confidence: Level,
}

/// O4: may `agent` do `action` on its own, under this policy? `migration` is read only for
/// [`Action::Migrate`], and `estimate` only for [`Action::PromoteIdea`], which is `never` unless
/// the estimate's label is `do-now` and its confidence reaches `minConfidenceToAct` — and a caller
/// that passes none is answered `never` (ADEV-273, D73).
pub fn may(
    policy: &Policy,
    agent: &str,
    action: Action,
    migration: Option<MigrationKind>,
    estimate: Option<EstimateRef>,
) -> MayAnswer {
    let a = &policy.autonomy;
    let (decision, reason) = if !policy.admits(agent) {
        (Autonomy::Never, MayReason::NotAdmitted)
    } else {
        match action {
            Action::Merge => (a.merge, MayReason::Policy),
            Action::Deploy => (a.deploy, MayReason::Policy),
            Action::Release => (a.release, MayReason::Policy),
            Action::Spend => (a.spend, MayReason::Policy),
            Action::ExperimentStart => (a.experiment_start, MayReason::Policy),
            Action::ExperimentStop => (a.experiment_stop, MayReason::Policy),
            Action::PromoteIdea => {
                let bar = policy
                    .tradeoff
                    .as_ref()
                    .map_or(TradeoffPolicy::default().min_confidence_to_act, |t| {
                        t.min_confidence_to_act
                    });
                match estimate {
                    None => (Autonomy::Never, MayReason::NoEstimate),
                    Some(e) if e.label != Label::DoNow => (Autonomy::Never, MayReason::NotDoNow),
                    Some(e) if e.confidence < bar => (Autonomy::Never, MayReason::BelowConfidence),
                    Some(_) => (a.promote_idea, MayReason::Policy),
                }
            }
            Action::Migrate => match migration {
                Some(MigrationKind::Additive) => (a.migrate.additive, MayReason::Policy),
                kind => {
                    let floored = a.migrate.destructive.stricter(a.migrate.additive);
                    let reason = if kind.is_none() {
                        MayReason::UnclassifiedMigration
                    } else if floored != a.migrate.destructive {
                        MayReason::DestructiveFloor
                    } else {
                        MayReason::Policy
                    };
                    (floored, reason)
                }
            },
        }
    };
    MayAnswer {
        decision,
        must_ask: decision == Autonomy::Ask,
        reason,
    }
}

/// What [`escalate`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalationAnswer {
    /// Everyone who receives the question, in order, without repeats.
    pub notify: Vec<String>,
    /// The write that hands the task to the first of them (Inbox, assigned to them).
    pub hand_back: HandBack,
}

/// O5: who receives a question or an approval request about a task on a list owned by
/// `list_owner_id`, created by `task_creator_id`.
pub fn escalate(
    policy: &Policy,
    list_owner_id: &str,
    task_creator_id: Option<&str>,
) -> EscalationAnswer {
    let named: Vec<&str> = match &policy.escalation {
        Escalation::ListOwner => vec![list_owner_id],
        Escalation::TaskCreator => vec![task_creator_id.unwrap_or(list_owner_id)],
        Escalation::People { user_ids } => user_ids.iter().map(String::as_str).collect(),
    };
    let mut notify: Vec<String> = Vec::new();
    for person in named.into_iter().map(str::trim) {
        if !person.is_empty() && !notify.iter().any(|seen| seen == person) {
            notify.push(person.to_string());
        }
    }
    if notify.is_empty() {
        notify.push(list_owner_id.to_string());
    }
    EscalationAnswer {
        hand_back: hand_back(&notify[0]),
        notify,
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// A key that is present, even as `null`, decodes as `Some`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layer(value: serde_json::Value) -> PolicyOverride {
        serde_json::from_value(value).expect("an override decodes")
    }

    /// Astrid's own boards: docs/FIXALL_WORKFLOW.md → Autonomy (Jon, 2026-11-15).
    fn astrid() -> PolicyOverride {
        layer(json!({
            "autonomy": {
                "merge": "allowed", "deploy": "allowed",
                "migrate": { "additive": "allowed", "destructive": "ask" },
                "release": "ask", "spend": "ask"
            },
            "escalation": { "to": "people", "userIds": ["jon"] }
        }))
    }

    #[test]
    fn nothing_stored_is_todays_behaviour_awtd_1685() {
        let policy = resolve(None, None);
        assert_eq!(policy, Policy::default());
        for action in [
            Action::Merge,
            Action::Deploy,
            Action::Release,
            Action::Spend,
        ] {
            assert_eq!(
                may(&policy, "claude", action, None, None).decision,
                Autonomy::Never
            );
        }
        assert_eq!(
            may(
                &policy,
                "claude",
                Action::Migrate,
                Some(MigrationKind::Additive),
                None
            )
            .decision,
            Autonomy::Never
        );
        assert!(route(&policy, "codex").take);
        assert_eq!(
            escalate(&policy, "owner", Some("creator")).notify,
            ["owner"]
        );
    }

    #[test]
    fn a_list_overrides_one_field_and_inherits_the_rest_awtd_1685() {
        let list = layer(json!({ "autonomy": { "merge": "ask" } }));
        let policy = resolve(Some(&astrid()), Some(&list));
        assert_eq!(policy.autonomy.merge, Autonomy::Ask);
        assert_eq!(policy.autonomy.deploy, Autonomy::Allowed);
        assert_eq!(policy.autonomy.migrate.destructive, Autonomy::Ask);
        assert_eq!(
            policy.escalation,
            Escalation::People {
                user_ids: vec!["jon".into()]
            }
        );
    }

    #[test]
    fn a_null_routing_field_overrides_an_absent_one_inherits_awtd_1685() {
        let project = layer(json!({ "routing": { "assignTo": "claude", "agents": ["claude"] } }));
        let cleared = layer(json!({ "routing": { "assignTo": null } }));
        let policy = resolve(Some(&project), Some(&cleared));
        assert_eq!(policy.routing.assign_to, None);
        assert_eq!(policy.routing.agents, Some(vec!["claude".to_string()]));
        let untouched = resolve(Some(&project), Some(&layer(json!({}))));
        assert_eq!(untouched.routing.assign_to.as_deref(), Some("claude"));
    }

    #[test]
    fn astrids_policy_ships_additive_migrations_and_asks_for_destructive_ones_awtd_1685() {
        let policy = resolve(Some(&astrid()), None);
        let migrate = |kind| may(&policy, "claude", Action::Migrate, kind, None);
        assert_eq!(
            migrate(Some(MigrationKind::Additive)).decision,
            Autonomy::Allowed
        );
        let destructive = migrate(Some(MigrationKind::Destructive));
        assert_eq!(destructive.decision, Autonomy::Ask);
        assert!(destructive.must_ask);
        assert_eq!(
            migrate(None).reason,
            MayReason::UnclassifiedMigration,
            "an unclassified migration is the destructive kind"
        );
        assert_eq!(migrate(None).decision, Autonomy::Ask);
    }

    #[test]
    fn a_destructive_migration_is_never_freer_than_an_additive_one_awtd_1685() {
        let policy = resolve(
            Some(&layer(json!({
                "autonomy": { "migrate": { "additive": "never", "destructive": "allowed" } }
            }))),
            None,
        );
        let answer = may(
            &policy,
            "claude",
            Action::Migrate,
            Some(MigrationKind::Destructive),
            None,
        );
        assert_eq!(answer.decision, Autonomy::Never);
        assert_eq!(answer.reason, MayReason::DestructiveFloor);
    }

    #[test]
    fn routing_names_one_agent_or_admits_a_set_awtd_1685() {
        let routed = resolve(
            Some(&layer(json!({ "routing": { "assignTo": "Codex" } }))),
            None,
        );
        assert_eq!(route(&routed, "codex").reason, RouteReason::Routed);
        assert_eq!(
            route(&routed, "claude").reason,
            RouteReason::RoutedElsewhere
        );

        let set = resolve(
            Some(&layer(
                json!({ "routing": { "agents": ["claude", "copilot"] } }),
            )),
            None,
        );
        assert!(route(&set, "copilot").take);
        assert_eq!(route(&set, "codex").reason, RouteReason::NotAdmitted);
        assert_eq!(
            may(&set, "codex", Action::Merge, None, None).reason,
            MayReason::NotAdmitted
        );
    }

    #[test]
    fn the_routed_agent_is_admitted_even_when_left_off_the_set_awtd_1685() {
        let policy = resolve(
            Some(&layer(
                json!({ "routing": { "assignTo": "codex", "agents": ["claude"] } }),
            )),
            None,
        );
        assert!(route(&policy, "codex").take);
        assert!(policy.admits("codex"));
    }

    #[test]
    fn escalation_hands_back_to_the_first_and_notifies_everyone_awtd_1685() {
        let people = resolve(
            Some(&layer(
                json!({ "escalation": { "to": "people", "userIds": ["jon", " ", "ana", "jon"] } }),
            )),
            None,
        );
        let answer = escalate(&people, "owner", None);
        assert_eq!(answer.notify, ["jon", "ana"]);
        assert_eq!(answer.hand_back, hand_back("jon"));

        let nobody = resolve(
            Some(&layer(
                json!({ "escalation": { "to": "people", "userIds": [] } }),
            )),
            None,
        );
        assert_eq!(escalate(&nobody, "owner", None).notify, ["owner"]);

        let creator = resolve(
            Some(&layer(json!({ "escalation": { "to": "taskCreator" } }))),
            None,
        );
        assert_eq!(escalate(&creator, "owner", Some("ana")).notify, ["ana"]);
        assert_eq!(escalate(&creator, "owner", None).notify, ["owner"]);
    }

    fn promote(policy: &Policy, estimate: Option<EstimateRef>) -> MayAnswer {
        may(policy, "claude", Action::PromoteIdea, None, estimate)
    }

    fn do_now(confidence: Level) -> Option<EstimateRef> {
        Some(EstimateRef {
            label: Label::DoNow,
            confidence,
        })
    }

    #[test]
    fn the_new_actions_default_to_never_adev_273() {
        let policy = resolve(None, None);
        for action in [Action::ExperimentStart, Action::ExperimentStop] {
            assert_eq!(
                may(&policy, "claude", action, None, None).decision,
                Autonomy::Never
            );
        }
        assert_eq!(
            promote(&policy, do_now(Level::High)).decision,
            Autonomy::Never
        );
    }

    #[test]
    fn promote_idea_needs_a_do_now_label_and_enough_confidence_adev_273() {
        let policy = resolve(
            Some(&layer(json!({
                "autonomy": { "promoteIdea": "allowed", "experimentStart": "ask" },
                "tradeoff": { "minConfidenceToAct": "high" }
            }))),
            None,
        );
        assert_eq!(promote(&policy, None).reason, MayReason::NoEstimate);
        let review = Some(EstimateRef {
            label: Label::NeedsReview,
            confidence: Level::High,
        });
        assert_eq!(promote(&policy, review).reason, MayReason::NotDoNow);
        let unsure = promote(&policy, do_now(Level::Medium));
        assert_eq!(unsure.decision, Autonomy::Never);
        assert_eq!(unsure.reason, MayReason::BelowConfidence);
        let sure = promote(&policy, do_now(Level::High));
        assert_eq!(sure.decision, Autonomy::Allowed);
        assert_eq!(
            may(&policy, "claude", Action::ExperimentStart, None, None).decision,
            Autonomy::Ask
        );
    }

    #[test]
    fn a_tradeoff_key_resolves_default_project_list_and_absent_means_no_policy_adev_273() {
        assert_eq!(resolve(None, None).tradeoff, None);
        let project = layer(
            json!({ "tradeoff": { "goalOrder": ["g2", "g1"], "risk": { "reviewAt": "low" } } }),
        );
        let list = layer(json!({ "tradeoff": { "risk": { "refuseAt": "medium" } } }));
        let tradeoff = resolve(Some(&project), Some(&list)).tradeoff.unwrap();
        assert_eq!(tradeoff.goal_order, ["g2", "g1"]);
        assert_eq!(tradeoff.risk.review_at, Level::Low);
        assert_eq!(tradeoff.risk.refuse_at, Level::Medium);
        assert_eq!(tradeoff.min_confidence_to_act, Level::Medium);
    }

    #[test]
    fn an_unknown_autonomy_word_is_refused_not_read_as_allowed_awtd_1685() {
        let typo = serde_json::from_value::<PolicyOverride>(
            json!({ "autonomy": { "merge": "sometimes" } }),
        );
        assert!(typo.is_err());
    }

    #[test]
    fn an_override_round_trips_absent_null_and_set_awtd_1685() {
        let stored = json!({
            "routing": { "assignTo": null, "agents": ["claude"] },
            "autonomy": { "merge": "ask", "migrate": { "destructive": "never" } }
        });
        let decoded = layer(stored.clone());
        assert_eq!(serde_json::to_value(&decoded).unwrap(), stored);
    }
}
