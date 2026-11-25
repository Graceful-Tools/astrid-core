//! The trade-off policy: what label an estimate earns, and how two ideas order (ADEV-273).
//!
//! **Core-first**, like [`crate::orchestration`], which holds the policy this one is a key of.
//! Design: `docs/GOAL_SIGNALS.md` §2; decision record: `docs/CONTRACTS.md` D72; contract:
//! `contracts/fixtures/tradeoff.json`, generated from this module (`cargo xtask gen-core-contracts`).
//!
//! ## T1 — no policy, no label
//!
//! A `tradeoff` key is stored on a project or a list, and [`crate::orchestration::resolve`] lays the
//! layers over each other field by field. With no key stored anywhere the resolved policy has no
//! trade-off, and [`evaluate`] answers `None`: the caller draws no label and offers no sort.
//!
//! ## T2 — four labels, strictest first
//!
//! [`Label`] is `dont`, `needs-review`, `experiment-first` or `do-now`; [`evaluate`] returns the
//! first row of the table in `docs/GOAL_SIGNALS.md` §2 that holds. Two floors apply whatever a
//! policy stores: **unknown is never good** (a section the estimator did not return is a reason
//! to review, never a pass) and **a built-in estimate is capped at `experiment-first`**.
//!
//! ## T3 — an order, not a score
//!
//! [`Evaluation::sort_key`] is a tuple of integers compared lexicographically, smaller first. Nothing
//! is weighted, multiplied or totalled, so there is no combined score to store or draw (web's
//! `docs/DECISIONS.md` rule 7 forbids one on the decision grid, and a sort that reduced an idea to a
//! number would be that column by another route).
//!
//! The rule reads no clock. An estimate older than the caller's staleness window arrives with
//! `stale: true`; the rule does not own the window.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::date;

/// A three-step scale: risk, and confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Low,
    Medium,
    High,
}

/// What an estimate earns. Ordered most to least permissive, so the stricter of two is the
/// greater, and the order is the first element of the sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Label {
    /// Build it.
    DoNow,
    /// Run an experiment before building.
    ExperimentFirst,
    /// A person decides.
    NeedsReview,
    /// Do not.
    Dont,
}

/// A probability: a number from 0 to 1. Anything else is refused when the policy is read.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(with = "f64"))]
pub struct Probability(f64);

impl TryFrom<f64> for Probability {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, String> {
        if (0.0..=1.0).contains(&value) {
            Ok(Probability(value))
        } else {
            Err(format!("{value} is not a probability between 0 and 1"))
        }
    }
}

impl From<Probability> for f64 {
    fn from(value: Probability) -> f64 {
        value.0
    }
}

/// Where the risk level sends an idea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RiskThresholds {
    /// At or above: `needs-review`.
    pub review_at: Level,
    /// At or above: `dont`.
    pub refuse_at: Level,
}

/// Where a guardrail's breach probability sends an idea.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GuardrailThresholds {
    /// At or above: `needs-review`.
    pub review_at_breach_probability: Probability,
    /// At or above: `dont`.
    pub refuse_at_breach_probability: Probability,
}

/// When an idea runs an experiment first: confidence below one level and risk at or above another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ExperimentFirst {
    pub confidence_below: Level,
    pub risk_at_least: Level,
}

/// A trade-off policy with every field decided.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TradeoffPolicy {
    /// The goals, most important first. They order ideas that tie on everything above them.
    pub goal_order: Vec<String>,
    pub risk: RiskThresholds,
    pub guardrail: GuardrailThresholds,
    pub experiment_first: ExperimentFirst,
    /// The least confidence an agent may promote an idea on (`PromoteIdea`'s floor).
    pub min_confidence_to_act: Level,
}

impl Default for TradeoffPolicy {
    /// What a stored key inherits where it names less than all of it. Not a policy by itself:
    /// with no key stored anywhere there is no trade-off (T1).
    fn default() -> Self {
        TradeoffPolicy {
            goal_order: Vec::new(),
            risk: RiskThresholds {
                review_at: Level::Medium,
                refuse_at: Level::High,
            },
            guardrail: GuardrailThresholds {
                review_at_breach_probability: Probability(0.1),
                refuse_at_breach_probability: Probability(0.5),
            },
            experiment_first: ExperimentFirst {
                confidence_below: Level::Medium,
                risk_at_least: Level::Medium,
            },
            min_confidence_to_act: Level::Medium,
        }
    }
}

/// [`RiskThresholds`], each field optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RiskOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_at: Option<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuse_at: Option<Level>,
}

/// [`GuardrailThresholds`], each field optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GuardrailOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_at_breach_probability: Option<Probability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuse_at_breach_probability: Option<Probability>,
}

/// [`ExperimentFirst`], each field optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ExperimentFirstOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_below: Option<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_at_least: Option<Level>,
}

/// What a project or a list stores under `tradeoff`: any subset of a [`TradeoffPolicy`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TradeoffOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub risk: RiskOverride,
    #[serde(default, skip_serializing_if = "is_default")]
    pub guardrail: GuardrailOverride,
    #[serde(default, skip_serializing_if = "is_default")]
    pub experiment_first: ExperimentFirstOverride,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence_to_act: Option<Level>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn set<T>(field: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *field = value;
    }
}

impl TradeoffPolicy {
    /// This policy with `layer` laid over it, field by field.
    pub fn with(mut self, layer: &TradeoffOverride) -> TradeoffPolicy {
        set(&mut self.goal_order, layer.goal_order.clone());
        set(&mut self.risk.review_at, layer.risk.review_at);
        set(&mut self.risk.refuse_at, layer.risk.refuse_at);
        set(
            &mut self.guardrail.review_at_breach_probability,
            layer.guardrail.review_at_breach_probability,
        );
        set(
            &mut self.guardrail.refuse_at_breach_probability,
            layer.guardrail.refuse_at_breach_probability,
        );
        set(
            &mut self.experiment_first.confidence_below,
            layer.experiment_first.confidence_below,
        );
        set(
            &mut self.experiment_first.risk_at_least,
            layer.experiment_first.risk_at_least,
        );
        set(&mut self.min_confidence_to_act, layer.min_confidence_to_act);
        self
    }
}

/// Which way a goal improves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// A larger value is better (the default).
    #[default]
    Up,
    /// A smaller value is better.
    Down,
}

/// An estimate's effect on one goal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GoalImpact {
    pub goal_id: String,
    /// The expected change.
    pub delta: f64,
    /// The interval around it.
    pub low: f64,
    pub high: f64,
    #[serde(default)]
    pub direction: Direction,
}

impl GoalImpact {
    /// The expected change, positive when it is in the goal's better direction.
    fn better(&self) -> f64 {
        match self.direction {
            Direction::Up => self.delta,
            Direction::Down => -self.delta,
        }
    }

    /// The pessimistic end of the interval, positive when even that is better.
    fn conservative(&self) -> f64 {
        match self.direction {
            Direction::Up => self.low,
            Direction::Down => -self.high,
        }
    }

    /// How the goal fares, best first: `0` even the pessimistic end gains, `1` the expectation
    /// gains, `2` no effect, `3` the expectation loses.
    fn band(&self) -> i64 {
        if self.conservative() > 0.0 {
            0
        } else if self.better() > 0.0 {
            1
        } else if self.better() == 0.0 {
            2
        } else {
            3
        }
    }
}

/// An estimate's effect on one guardrail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GuardrailImpact {
    pub guardrail_id: String,
    pub breach_probability: Probability,
}

/// The estimate's risk. A numeric `score` may ride along; the rule reads only the level, the
/// vocabulary policies are written in (`docs/GOAL_SIGNALS.md` §8.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Risk {
    pub level: Level,
}

/// An estimate, as the server stores it. A `None` section is one the estimator did not return.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    #[serde(default)]
    pub goal_impacts: Option<Vec<GoalImpact>>,
    #[serde(default)]
    pub guardrail_impacts: Option<Vec<GuardrailImpact>>,
    #[serde(default)]
    pub risk: Option<Risk>,
    #[serde(default)]
    pub confidence: Option<Level>,
    /// `builtin_llm` for the built-in estimator; anything else is a service the owner connected.
    #[serde(default)]
    pub source: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "Option<date::Timestamp>"))]
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microcents: Option<i64>,
    /// The caller's staleness window has passed.
    #[serde(default)]
    pub stale: bool,
    /// The project states guardrails, so an estimate with no `guardrailImpacts` has not been
    /// checked against them.
    #[serde(default)]
    pub guardrails_stated: bool,
}

/// The `source` of the built-in estimator.
pub const BUILTIN_SOURCE: &str = "builtin_llm";

/// Why an estimate earned its label: a code and the values that produced it, so every client
/// words it in its own strings and none re-derives it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "code",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Reason {
    /// There is no estimate.
    NoEstimate,
    /// The estimate is older than the caller's window.
    Stale,
    GuardrailAtRefuse {
        guardrail_id: String,
        breach_probability: f64,
        threshold: f64,
    },
    RiskAtRefuse {
        level: Level,
        threshold: Level,
    },
    GuardrailAtReview {
        guardrail_id: String,
        breach_probability: f64,
        threshold: f64,
    },
    RiskAtReview {
        level: Level,
        threshold: Level,
    },
    /// The estimator returned no risk.
    RiskUnknown,
    /// The project states guardrails and the estimator returned no impact on them.
    GuardrailsUnknown,
    /// Confidence is low, or was not returned.
    ConfidenceLow {
        confidence: Option<Level>,
    },
    /// Confidence is below the policy's bar while the risk is at or above its bar.
    ExperimentFirst {
        confidence: Level,
        risk: Level,
    },
    /// A built-in estimate may not earn more than `experiment-first`.
    BuiltinCapped,
    /// The expected effect is better for this goal.
    GoalImproves {
        goal_id: String,
        delta: f64,
    },
    /// No goal is expected to improve.
    NoGoalImproves,
}

/// What [`evaluate`] answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evaluation {
    pub label: Label,
    pub reasons: Vec<Reason>,
    /// Compared lexicographically, smaller first. Stored beside the estimate and compared, never
    /// evaluated, by the `policy` sort ([`crate::filters::sort_by_setting`]).
    pub sort_key: Vec<i64>,
}

fn level_rank(level: Option<Level>) -> i64 {
    level.map_or(3, |level| level as i64)
}

/// T2 and T3: the label an estimate earns under `policy`, why, and where it sorts. `None` is "no
/// policy" (T1): the caller draws nothing.
pub fn evaluate(
    policy: Option<&TradeoffPolicy>,
    estimate: Option<&Estimate>,
) -> Option<Evaluation> {
    let policy = policy?;
    let Some(estimate) = estimate else {
        return Some(unreadable(policy, Reason::NoEstimate, None));
    };
    if estimate.stale {
        return Some(unreadable(policy, Reason::Stale, Some(estimate)));
    }

    let refuse = f64::from(policy.guardrail.refuse_at_breach_probability);
    let review = f64::from(policy.guardrail.review_at_breach_probability);
    let guardrails = estimate.guardrail_impacts.as_deref().unwrap_or_default();
    let at = |threshold: f64| {
        guardrails
            .iter()
            .filter(move |g| f64::from(g.breach_probability) >= threshold)
    };
    let risk = estimate.risk.map(|risk| risk.level);

    // Row 2: refuse.
    let mut refuse_reasons: Vec<Reason> = at(refuse)
        .map(|g| Reason::GuardrailAtRefuse {
            guardrail_id: g.guardrail_id.clone(),
            breach_probability: f64::from(g.breach_probability),
            threshold: refuse,
        })
        .collect();
    if let Some(level) = risk.filter(|level| *level >= policy.risk.refuse_at) {
        refuse_reasons.push(Reason::RiskAtRefuse {
            level,
            threshold: policy.risk.refuse_at,
        });
    }

    // Row 3: review. A breach at the refuse threshold is also at the review threshold, so the count
    // the sort uses is every guardrail at or past review.
    let guardrails_at_review = at(review).count() as i64;
    let mut review_reasons: Vec<Reason> = at(review)
        .map(|g| Reason::GuardrailAtReview {
            guardrail_id: g.guardrail_id.clone(),
            breach_probability: f64::from(g.breach_probability),
            threshold: review,
        })
        .collect();
    if let Some(level) = risk.filter(|level| *level >= policy.risk.review_at) {
        review_reasons.push(Reason::RiskAtReview {
            level,
            threshold: policy.risk.review_at,
        });
    }
    if risk.is_none() {
        review_reasons.push(Reason::RiskUnknown);
    }
    if estimate.guardrail_impacts.is_none() && estimate.guardrails_stated {
        review_reasons.push(Reason::GuardrailsUnknown);
    }

    let builtin = estimate.source.as_deref() == Some(BUILTIN_SOURCE);
    let (label, reasons) = if !refuse_reasons.is_empty() {
        (Label::Dont, refuse_reasons)
    } else if !review_reasons.is_empty() {
        (Label::NeedsReview, review_reasons)
    } else if estimate.confidence.is_none_or(|c| c == Level::Low) {
        (
            Label::ExperimentFirst,
            vec![Reason::ConfidenceLow {
                confidence: estimate.confidence,
            }],
        )
    } else if let Some((confidence, risk)) = estimate.confidence.zip(risk).filter(|(c, r)| {
        *c < policy.experiment_first.confidence_below && *r >= policy.experiment_first.risk_at_least
    }) {
        (
            Label::ExperimentFirst,
            vec![Reason::ExperimentFirst { confidence, risk }],
        )
    } else {
        let improves: Vec<Reason> = estimate
            .goal_impacts
            .iter()
            .flatten()
            .filter(|impact| impact.better() > 0.0)
            .map(|impact| Reason::GoalImproves {
                goal_id: impact.goal_id.clone(),
                delta: impact.delta,
            })
            .collect();
        if improves.is_empty() {
            (Label::NeedsReview, vec![Reason::NoGoalImproves])
        } else if builtin {
            let mut reasons = vec![Reason::BuiltinCapped];
            reasons.extend(improves);
            (Label::ExperimentFirst, reasons)
        } else {
            (Label::DoNow, improves)
        }
    };

    let mut sort_key = vec![label as i64, guardrails_at_review];
    for goal in &policy.goal_order {
        let band = estimate
            .goal_impacts
            .iter()
            .flatten()
            .find(|impact| &impact.goal_id == goal)
            .map_or(2, GoalImpact::band);
        sort_key.push(band);
    }
    sort_key.push(level_rank(risk));
    sort_key.push(estimate.cost_microcents.unwrap_or(i64::MAX));
    sort_key.push(
        estimate
            .created_at
            .map_or(i64::MAX, |at| -at.as_millisecond()),
    );
    Some(Evaluation {
        label,
        reasons,
        sort_key,
    })
}

/// Row 1: no estimate, or one too old to act on. It is `needs-review` and sorts after every readable
/// `needs-review` idea, because nothing is known about it.
fn unreadable(policy: &TradeoffPolicy, reason: Reason, estimate: Option<&Estimate>) -> Evaluation {
    let mut sort_key = vec![Label::NeedsReview as i64, i64::MAX];
    sort_key.extend(policy.goal_order.iter().map(|_| 3));
    sort_key.push(level_rank(None));
    sort_key.push(i64::MAX);
    sort_key.push(
        estimate
            .and_then(|estimate| estimate.created_at)
            .map_or(i64::MAX, |at| -at.as_millisecond()),
    );
    Evaluation {
        label: Label::NeedsReview,
        reasons: vec![reason],
        sort_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> TradeoffPolicy {
        TradeoffPolicy::default().with(&TradeoffOverride {
            goal_order: Some(vec!["g2".into(), "g1".into()]),
            ..Default::default()
        })
    }

    fn estimate(value: serde_json::Value) -> Estimate {
        serde_json::from_value(value).expect("an estimate decodes")
    }

    /// A good estimate: both goals up, low risk, high confidence, a clear guardrail.
    fn good() -> serde_json::Value {
        json!({
            "goalImpacts": [
                { "goalId": "g1", "delta": 5.0, "low": 2.0, "high": 8.0 },
                { "goalId": "g2", "delta": 1.0, "low": -1.0, "high": 3.0 }
            ],
            "guardrailImpacts": [{ "guardrailId": "r1", "breachProbability": 0.01 }],
            "risk": { "level": "low" },
            "confidence": "high",
            "source": "service"
        })
    }

    fn label(value: serde_json::Value) -> Label {
        evaluate(Some(&policy()), Some(&estimate(value)))
            .expect("a policy")
            .label
    }

    fn with(mut value: serde_json::Value, edit: serde_json::Value) -> serde_json::Value {
        value.as_object_mut().unwrap().extend(
            edit.as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        value
    }

    #[test]
    fn no_policy_answers_no_policy_adev_273() {
        assert_eq!(evaluate(None, Some(&estimate(good()))), None);
    }

    #[test]
    fn a_good_estimate_is_do_now_adev_273() {
        assert_eq!(label(good()), Label::DoNow);
    }

    #[test]
    fn a_missing_or_stale_estimate_needs_review_adev_273() {
        let missing = evaluate(Some(&policy()), None).unwrap();
        assert_eq!(missing.label, Label::NeedsReview);
        assert_eq!(missing.reasons, [Reason::NoEstimate]);
        let stale = label(with(good(), json!({ "stale": true })));
        assert_eq!(stale, Label::NeedsReview);
    }

    #[test]
    fn a_guardrail_at_its_threshold_is_dont_or_needs_review_adev_273() {
        let breach = |p: f64| {
            with(
                good(),
                json!({ "guardrailImpacts": [{ "guardrailId": "r1", "breachProbability": p }] }),
            )
        };
        assert_eq!(label(breach(0.09)), Label::DoNow);
        assert_eq!(label(breach(0.1)), Label::NeedsReview);
        assert_eq!(label(breach(0.5)), Label::Dont);
    }

    #[test]
    fn risk_at_its_threshold_is_dont_or_needs_review_adev_273() {
        let risk = |level: &str| with(good(), json!({ "risk": { "level": level } }));
        assert_eq!(label(risk("medium")), Label::NeedsReview);
        assert_eq!(label(risk("high")), Label::Dont);
    }

    #[test]
    fn low_confidence_runs_an_experiment_first_adev_273() {
        assert_eq!(
            label(with(good(), json!({ "confidence": "low" }))),
            Label::ExperimentFirst
        );
    }

    #[test]
    fn experiment_first_when_confidence_is_below_the_bar_and_risk_is_at_it_adev_273() {
        let policy = TradeoffPolicy::default().with(&TradeoffOverride {
            risk: RiskOverride {
                review_at: Some(Level::High),
                refuse_at: Some(Level::High),
            },
            experiment_first: ExperimentFirstOverride {
                confidence_below: Some(Level::High),
                risk_at_least: Some(Level::Medium),
            },
            ..Default::default()
        });
        let medium = estimate(with(
            good(),
            json!({ "confidence": "medium", "risk": { "level": "medium" } }),
        ));
        let answer = evaluate(Some(&policy), Some(&medium)).unwrap();
        assert_eq!(answer.label, Label::ExperimentFirst);
        assert!(matches!(
            answer.reasons[0],
            Reason::ExperimentFirst {
                confidence: Level::Medium,
                risk: Level::Medium
            }
        ));
    }

    #[test]
    fn unknown_is_never_good_adev_273() {
        let mut no_risk = good();
        no_risk.as_object_mut().unwrap().remove("risk");
        assert_eq!(label(no_risk), Label::NeedsReview);

        let mut no_guardrails = good();
        no_guardrails
            .as_object_mut()
            .unwrap()
            .remove("guardrailImpacts");
        assert_eq!(
            label(with(
                no_guardrails.clone(),
                json!({ "guardrailsStated": true })
            )),
            Label::NeedsReview
        );
        // A project that states no guardrails has nothing to be unknown about.
        assert_eq!(label(no_guardrails), Label::DoNow);

        let mut no_goals = good();
        no_goals.as_object_mut().unwrap().remove("goalImpacts");
        assert_eq!(label(no_goals), Label::NeedsReview);
        let mut no_confidence = good();
        no_confidence.as_object_mut().unwrap().remove("confidence");
        assert_eq!(label(no_confidence), Label::ExperimentFirst);
    }

    #[test]
    fn a_goal_that_gets_worse_is_not_a_reason_to_build_adev_273() {
        let worse = with(
            good(),
            json!({ "goalImpacts": [{ "goalId": "g1", "delta": -1.0, "low": -2.0, "high": 0.0 }] }),
        );
        assert_eq!(label(worse), Label::NeedsReview);
        // A goal that improves downward is better when its delta is negative.
        let down = with(
            good(),
            json!({ "goalImpacts": [
                { "goalId": "g1", "delta": -1.0, "low": -2.0, "high": 0.0, "direction": "down" }
            ] }),
        );
        assert_eq!(label(down), Label::DoNow);
    }

    #[test]
    fn a_builtin_estimate_is_capped_at_experiment_first_adev_273() {
        let builtin = with(good(), json!({ "source": "builtin_llm" }));
        let answer = evaluate(Some(&policy()), Some(&estimate(builtin))).unwrap();
        assert_eq!(answer.label, Label::ExperimentFirst);
        assert_eq!(answer.reasons[0], Reason::BuiltinCapped);
        // It can still be refused outright.
        let risky = with(
            good(),
            json!({ "source": "builtin_llm", "risk": { "level": "high" } }),
        );
        assert_eq!(label(risky), Label::Dont);
    }

    #[test]
    fn ideas_order_by_label_then_the_goals_in_the_policys_order_adev_273() {
        let key = |value: serde_json::Value| {
            evaluate(Some(&policy()), Some(&estimate(value)))
                .unwrap()
                .sort_key
        };
        let gains_g2 = with(
            good(),
            json!({ "goalImpacts": [
                { "goalId": "g1", "delta": 1.0, "low": -1.0, "high": 2.0 },
                { "goalId": "g2", "delta": 3.0, "low": 1.0, "high": 5.0 }
            ] }),
        );
        let gains_g1 = with(
            good(),
            json!({ "goalImpacts": [
                { "goalId": "g1", "delta": 3.0, "low": 1.0, "high": 5.0 },
                { "goalId": "g2", "delta": 1.0, "low": -1.0, "high": 2.0 }
            ] }),
        );
        // goalOrder is [g2, g1]: the idea that surely gains g2 leads.
        assert!(key(gains_g2.clone()) < key(gains_g1));
        // A better label beats any goal band.
        let review = with(gains_g2, json!({ "risk": { "level": "medium" } }));
        assert!(key(good()) < key(review));
    }

    #[test]
    fn a_cheaper_or_newer_idea_breaks_the_last_ties_adev_273() {
        let key = |edit: serde_json::Value| {
            evaluate(Some(&policy()), Some(&estimate(with(good(), edit))))
                .unwrap()
                .sort_key
        };
        assert!(key(json!({ "costMicrocents": 10 })) < key(json!({ "costMicrocents": 20 })));
        assert!(
            key(json!({ "createdAt": "2026-11-25T00:00:00Z" }))
                < key(json!({ "createdAt": "2026-11-01T00:00:00Z" }))
        );
    }

    #[test]
    fn a_stored_value_outside_the_vocabulary_is_refused_adev_273() {
        assert!(serde_json::from_value::<TradeoffOverride>(
            json!({ "risk": { "reviewAt": "scary" } })
        )
        .is_err());
        assert!(serde_json::from_value::<TradeoffOverride>(
            json!({ "guardrail": { "refuseAtBreachProbability": 1.5 } })
        )
        .is_err());
    }

    #[test]
    fn a_layer_names_less_than_all_of_it_adev_273() {
        let layered = TradeoffPolicy::default()
            .with(&serde_json::from_value(json!({ "risk": { "reviewAt": "low" } })).unwrap());
        assert_eq!(layered.risk.review_at, Level::Low);
        assert_eq!(layered.risk.refuse_at, Level::High);
    }
}
