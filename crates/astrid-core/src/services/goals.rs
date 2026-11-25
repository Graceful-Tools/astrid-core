//! A project's goals and guardrails, for the clients (ADEV-187; web ADEV-179, Android ADEV-184,
//! iOS ADEV-183).
//!
//! The contract is web's `docs/API_CONTRACT.md` → *GET|PUT `/api/v1/projects/{id}/goals` and GET
//! `/api/v1/lists/{id}/goals`*, and the shape and the rules are `lib/goals-guardrails.ts`.
//!
//! ## What is here, and what is not
//!
//! The **server normalises**: it trims every field, drops items with a blank statement, defaults a
//! guardrail's kind, assigns `g1…` / `r1…` to rows with no id, and enforces the limits. So nothing
//! here re-implements those rules, and the core keeps the server's echo of a write, never the
//! request. The one exception is the optimistic copy of a write made offline ([`optimistic`]),
//! which has to look like something until the journal delivers and the echo replaces it.
//!
//! ## Reads, writes and the cache
//!
//! Every answer is cached under a `metadata` key, so a board with no network still shows what it
//! last knew. A read that cannot reach the server answers with the cache; one the server
//! *refuses* (a project the caller cannot see) is an error, and the cache is left alone.
//!
//! A write is sent at once and journalled only when the network failed, like a project field
//! ([`super::project_fields`], CONTRACTS D56). Online, a refusal reaches the person who made the
//! edit: a `403` (an agent token, or not an owner or admin) or a `400` (a limit, a duplicate id,
//! an unknown kind) will say the same tomorrow, so it is a plain failure and never queued. Offline
//! the edit is optimistic and journalled, and a refusal on delivery is dead-lettered, not retried.
//!
//! Both lists empty means nothing is stated: a shell shows nothing. `can_edit == false` means
//! read-only: a shell hides the editor.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Context, Result, ServiceError};
use crate::api::{endpoints, ApiError};
use crate::outbox::{self, entry::kind, journal};
use crate::store::Store;

/// Whether a person judges a guardrail, or a program checks it (ADEV-182).
///
/// A kind this build does not know reads as `stated`: a guardrail shown as one a reader judges is
/// better than a guardrail that vanishes because a newer server added a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum GuardrailKind {
    Enforced,
    #[default]
    #[serde(other)]
    Stated,
}

/// Which way is better for a goal's metric (ADEV-275; web `GoalMetric.direction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum MetricDirection {
    #[default]
    Up,
    Down,
}

/// Where a goal's metric is read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MetricSource {
    pub connector_id: String,
    pub metric_key: String,
}

/// A goal's measure as numbers (ADEV-270 on web, ADEV-275 here). `current` and `measured_at` are the
/// server's: a write never changes them, but a client that holds the block sends it back whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GoalMetric {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub direction: MetricDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<f64>,
    /// ISO timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MetricSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after_hours: Option<f64>,
}

/// One goal, as the server answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub id: String,
    pub statement: String,
    /// What is measured, e.g. "IRR".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<String>,
    /// Where the measure should get to, e.g. "≥ 8%/yr".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Where it is now, e.g. "6.1%".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// The typed number behind the free text. Absent on a prose goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<GoalMetric>,
}

/// One guardrail, as the server answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Guardrail {
    pub id: String,
    pub statement: String,
    #[serde(default)]
    pub kind: GuardrailKind,
    /// What a program evaluates for an enforced guardrail (a `policy` or `metric` check). Kept as
    /// the server sent it: the core does not judge it, and a check type it does not know survives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Value>,
}

/// What `GetProjectGoals` and `GetListGoals` answer: the route's body as-is.
///
/// `list_id` is there for a list's answer only; `project_id` and `project_name` are `null` for a
/// list in no project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ProjectGoals {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub goals: Vec<Goal>,
    #[serde(default)]
    pub guardrails: Vec<Guardrail>,
    /// Owner or admin, and not an agent. `false` means read-only.
    #[serde(default)]
    pub can_edit: bool,
}

impl ProjectGoals {
    /// Nothing stated: a shell draws no control at all.
    pub fn is_empty(&self) -> bool {
        self.goals.is_empty() && self.guardrails.is_empty()
    }
}

/// A goal being written. A row with no `id` is new; the server assigns `g1`, `g2`…
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GoalDraft {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// Sent back whole by a client that holds it. Left out, the server keeps the stored block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<GoalMetric>,
}

/// A guardrail being written. No `id` is new (`r1`, `r2`…); no `kind` is `stated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GuardrailDraft {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<GuardrailKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Value>,
}

pub fn project_key(project_id: &str) -> String {
    format!("projectGoals:{project_id}")
}

pub fn list_key(list_id: &str) -> String {
    format!("listGoals:{list_id}")
}

/// Keep the server's answer for a project, and say whether it moved the cached value.
pub fn store_project(store: &Store, project_id: &str, value: &ProjectGoals) -> bool {
    write(store, &project_key(project_id), value)
}

fn write(store: &Store, key: &str, value: &ProjectGoals) -> bool {
    let before: Option<ProjectGoals> = read(store, key);
    if before.as_ref() == Some(value) {
        return false;
    }
    if let Ok(json) = serde_json::to_string(value) {
        let _ = store.set_metadata(key, &json);
    }
    true
}

fn read(store: &Store, key: &str) -> Option<ProjectGoals> {
    store
        .metadata(key)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Fill missing ids as `{prefix}1`, `{prefix}2`…, skipping any id the writer already used — the
/// server's rule, so an offline copy reads as the echo will.
fn assign_ids(given: &[Option<String>], prefix: &str) -> Vec<String> {
    let mut taken: Vec<String> = given.iter().flatten().cloned().collect();
    let mut n = 0;
    given
        .iter()
        .map(|id| match id {
            Some(id) => id.clone(),
            None => loop {
                n += 1;
                let candidate = format!("{prefix}{n}");
                if !taken.contains(&candidate) {
                    taken.push(candidate.clone());
                    break candidate;
                }
            },
        })
        .collect()
}

/// What an edit made offline shows until the journal delivers and the server's echo replaces it.
pub fn optimistic(
    base: &ProjectGoals,
    goals: &[GoalDraft],
    guardrails: &[GuardrailDraft],
) -> ProjectGoals {
    let goals: Vec<&GoalDraft> = goals
        .iter()
        .filter(|g| !g.statement.trim().is_empty())
        .collect();
    let guardrails: Vec<&GuardrailDraft> = guardrails
        .iter()
        .filter(|r| !r.statement.trim().is_empty())
        .collect();
    let goal_ids = assign_ids(
        &goals
            .iter()
            .map(|g| trimmed(g.id.clone()))
            .collect::<Vec<_>>(),
        "g",
    );
    let guardrail_ids = assign_ids(
        &guardrails
            .iter()
            .map(|r| trimmed(r.id.clone()))
            .collect::<Vec<_>>(),
        "r",
    );
    ProjectGoals {
        goals: goals
            .iter()
            .zip(goal_ids)
            .map(|(g, id)| Goal {
                id,
                statement: g.statement.trim().to_string(),
                measure: trimmed(g.measure.clone()),
                target: trimmed(g.target.clone()),
                current: trimmed(g.current.clone()),
                metric: g.metric.clone(),
            })
            .collect(),
        guardrails: guardrails
            .iter()
            .zip(guardrail_ids)
            .map(|(r, id)| Guardrail {
                id,
                statement: r.statement.trim().to_string(),
                kind: r.kind.unwrap_or_default(),
                check: r.check.clone(),
            })
            .collect(),
        ..base.clone()
    }
}

/// The server's refusal code, rather than the whole body, so a shell can pick its words.
fn refusal(error: ApiError) -> ServiceError {
    match error {
        ApiError::Http { status, message } => {
            let code = serde_json::from_str::<Value>(&message)
                .ok()
                .and_then(|body| {
                    body.get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or(message);
            ServiceError::Api(ApiError::Http {
                status,
                message: code,
            })
        }
        error => ServiceError::Api(error),
    }
}

/// An answer, and whether it moved what is cached (the shell is told only when it did).
pub struct Answer {
    pub goals: ProjectGoals,
    pub changed: bool,
}

pub struct GoalsService {
    context: Context,
}

impl GoalsService {
    pub fn new(context: Context) -> Self {
        GoalsService { context }
    }

    /// What is cached for this project. Draws at once, and with no network.
    pub fn cached_project(&self, project_id: &str) -> ProjectGoals {
        read(&self.context.store, &project_key(project_id)).unwrap_or_else(|| ProjectGoals {
            project_id: Some(project_id.to_string()),
            ..ProjectGoals::default()
        })
    }

    fn decode(answer: &Value) -> Result<ProjectGoals> {
        serde_json::from_value(answer.clone())
            .map_err(|error| ServiceError::Api(ApiError::Decode(error.to_string())))
    }

    /// Fetch a project's goals and cache the answer. Offline, the cache is the answer.
    pub async fn refresh_project(&self, project_id: &str) -> Result<Answer> {
        let client = &self.context.client;
        match client
            .send(client.get(endpoints::project_goals(project_id)))
            .await
        {
            Ok(answer) => {
                let goals = Self::decode(&answer)?;
                let changed = store_project(&self.context.store, project_id, &goals);
                Ok(Answer { goals, changed })
            }
            Err(ApiError::Transport(_)) => Ok(Answer {
                goals: self.cached_project(project_id),
                changed: false,
            }),
            Err(error) => Err(refusal(error)),
        }
    }

    /// Fetch a list's goals, which are its project's. Cached per list; offline, the cache.
    pub async fn refresh_list(&self, list_id: &str) -> Result<Answer> {
        let client = &self.context.client;
        let key = list_key(list_id);
        match client
            .send(client.get(endpoints::list_goals(list_id)))
            .await
        {
            Ok(answer) => {
                let mut goals = Self::decode(&answer)?;
                goals.list_id = Some(list_id.to_string());
                let changed = write(&self.context.store, &key, &goals);
                Ok(Answer { goals, changed })
            }
            Err(ApiError::Transport(_)) => Ok(Answer {
                goals: read(&self.context.store, &key).unwrap_or_else(|| ProjectGoals {
                    list_id: Some(list_id.to_string()),
                    ..ProjectGoals::default()
                }),
                changed: false,
            }),
            Err(error) => Err(refusal(error)),
        }
    }

    /// Is a goals write for this project still waiting in the journal? Then a fetch would answer
    /// with the value from before it, and a later write must queue behind it.
    fn queued(&self, project_id: &str) -> Result<bool> {
        Ok(journal::all(&self.context.store)?.iter().any(|entry| {
            entry.kind == kind::SET_PROJECT_GOALS
                && matches!(
                    entry.status,
                    crate::outbox::Status::Pending | crate::outbox::Status::Running
                )
                && entry.payload.get("projectId").and_then(Value::as_str) == Some(project_id)
        }))
    }

    /// Replace a project's goals and guardrails — sent at once, queued only when the network
    /// failed. A refusal is a plain failure.
    pub async fn set_now(
        &self,
        project_id: &str,
        goals: Vec<GoalDraft>,
        guardrails: Vec<GuardrailDraft>,
    ) -> Result<Answer> {
        let body = json!({ "goals": goals, "guardrails": guardrails });
        if self.queued(project_id)? {
            return self.enqueue(project_id, &goals, &guardrails, body);
        }
        let request = self
            .context
            .client
            .put(endpoints::project_goals(project_id))
            .value(body.clone());
        match self.context.client.send(request).await {
            Ok(answer) => {
                let stored = Self::decode(&answer)?;
                let changed = store_project(&self.context.store, project_id, &stored);
                Ok(Answer {
                    goals: stored,
                    changed,
                })
            }
            Err(ApiError::Transport(_)) => self.enqueue(project_id, &goals, &guardrails, body),
            Err(error) => Err(refusal(error)),
        }
    }

    fn enqueue(
        &self,
        project_id: &str,
        goals: &[GoalDraft],
        guardrails: &[GuardrailDraft],
        body: Value,
    ) -> Result<Answer> {
        let base = self.cached_project(project_id);
        let shown = optimistic(&base, goals, guardrails);
        let changed = store_project(&self.context.store, project_id, &shown);
        let entry = outbox::build(
            kind::SET_PROJECT_GOALS,
            json!({ "projectId": project_id, "body": body }),
            &outbox::new_temp_id(),
            self.context.clock.now(),
        );
        journal::enqueue(&self.context.store, &entry)?;
        Ok(Answer {
            goals: shown,
            changed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(id: Option<&str>, statement: &str) -> GoalDraft {
        GoalDraft {
            id: id.map(str::to_string),
            statement: statement.into(),
            ..GoalDraft::default()
        }
    }

    /// ADEV-187: web's rule — a row with no id gets the first free `g1, g2…`, and one the writer
    /// already named is never reused.
    #[test]
    fn adev_187_new_rows_take_the_first_free_id() {
        let shown = optimistic(
            &ProjectGoals::default(),
            &[
                goal(Some("g1"), "kept"),
                goal(None, "new"),
                goal(None, "newer"),
            ],
            &[],
        );
        let ids: Vec<&str> = shown.goals.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["g1", "g2", "g3"]);
    }

    /// ADEV-187: blank statements drop, optional fields trim away, a guardrail defaults to stated.
    #[test]
    fn adev_187_blank_rows_drop_and_kind_defaults_to_stated() {
        let shown = optimistic(
            &ProjectGoals::default(),
            &[
                GoalDraft {
                    measure: Some("  ".into()),
                    target: Some(" 8% ".into()),
                    ..goal(None, "  Grow  ")
                },
                goal(None, "   "),
            ],
            &[GuardrailDraft {
                statement: "No laws broken".into(),
                ..GuardrailDraft::default()
            }],
        );
        assert_eq!(shown.goals.len(), 1);
        assert_eq!(shown.goals[0].statement, "Grow");
        assert_eq!(shown.goals[0].measure, None);
        assert_eq!(shown.goals[0].target.as_deref(), Some("8%"));
        assert_eq!(shown.guardrails[0].kind, GuardrailKind::Stated);
        assert_eq!(shown.guardrails[0].id, "r1");
    }

    /// ADEV-275: a goal's `metric` block survives a read, and a write of the same goal sends it back.
    #[test]
    fn adev_275_a_goals_metric_round_trips() {
        let wire = json!({
            "id": "g1", "statement": "Grow", "target": "8", "current": "6",
            "metric": {
                "unit": "%", "direction": "down", "target": 8.0, "baseline": 12.5, "current": 6.1,
                "measuredAt": "2026-11-25T10:00:00.000Z",
                "source": { "connectorId": "c1", "metricKey": "irr" },
                "staleAfterHours": 24.0,
                "futureField": true
            }
        });
        let goal: Goal = serde_json::from_value(wire).expect("decodes");
        let metric = goal.metric.as_ref().expect("metric kept");
        assert_eq!(metric.direction, MetricDirection::Down);
        assert_eq!(metric.target, Some(8.0));
        assert_eq!(metric.current, Some(6.1));
        assert_eq!(
            metric.source.as_ref().map(|s| s.metric_key.as_str()),
            Some("irr")
        );
        let back = serde_json::to_value(&goal).expect("encodes");
        assert_eq!(back["metric"]["direction"], "down");
        assert_eq!(back["metric"]["staleAfterHours"], 24.0);
        assert_eq!(back["metric"]["source"]["connectorId"], "c1");
    }

    /// ADEV-275: a goal with no metric reads and writes exactly as before.
    #[test]
    fn adev_275_a_prose_goal_has_no_metric_key() {
        let goal: Goal = serde_json::from_value(json!({ "id": "g1", "statement": "x" })).unwrap();
        assert!(goal.metric.is_none());
        assert!(serde_json::to_value(&goal).unwrap().get("metric").is_none());
    }

    /// ADEV-275: an offline edit keeps the metric the editor still holds, and a guardrail keeps its
    /// check, whatever kind of check a newer server sent.
    #[test]
    fn adev_275_an_offline_edit_keeps_metric_and_check() {
        let metric = GoalMetric {
            direction: MetricDirection::Up,
            target: Some(10.0),
            ..GoalMetric::default()
        };
        let check = json!({ "type": "metric", "goalId": "g1", "atLeast": 5 });
        let shown = optimistic(
            &ProjectGoals::default(),
            &[GoalDraft {
                metric: Some(metric.clone()),
                ..goal(None, "Grow")
            }],
            &[GuardrailDraft {
                statement: "Floor".into(),
                kind: Some(GuardrailKind::Enforced),
                check: Some(check.clone()),
                ..GuardrailDraft::default()
            }],
        );
        assert_eq!(shown.goals[0].metric, Some(metric));
        assert_eq!(shown.guardrails[0].check, Some(check));
        let sent = serde_json::to_value(GuardrailDraft::default()).unwrap();
        assert!(sent.get("check").is_none());
    }

    /// ADEV-187: a kind from a newer server still shows its guardrail.
    #[test]
    fn adev_187_an_unknown_guardrail_kind_reads_as_stated() {
        let guardrail: Guardrail =
            serde_json::from_value(json!({ "id": "r1", "statement": "x", "kind": "audited" }))
                .expect("decodes");
        assert_eq!(guardrail.kind, GuardrailKind::Stated);
    }
}
