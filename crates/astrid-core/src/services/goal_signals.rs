//! What a task does for its project's goals: its goal links, its estimate and its experiment
//! results, read-only (ADEV-285; web ADEV-271, ADEV-278, ADEV-279; `docs/GOAL_SIGNALS.md` §6, §9
//! row 7).
//!
//! The contract is web's `docs/API_CONTRACT.md` → *GET `/api/v1/tasks/{id}/goals`*, *GET
//! `/api/v1/tasks/{id}/estimates`* and *GET `/api/v1/tasks/{id}/experiment`*.
//!
//! ## What is here, and what is not
//!
//! Three reads, each cached under a `metadata` key so a board with no network still draws the
//! label chip, the estimate summary and the Pending results card from what it last knew. A read
//! that cannot reach the server answers with the cache; one the server *refuses* (`404` where
//! the capability is off, `403 not_granted` without the flag, a task the caller cannot see) is an
//! error, and the cache is left alone: a shell draws no section for it.
//!
//! **Nothing here writes**, and nothing here computes. The label, its reasons and the experiment's
//! verdict are the server's, kept as sent. Sections whose shape the server owns (impacts, risk,
//! metrics, findings, reasons) are kept as [`Value`], so a field a newer server adds survives a
//! round trip through the cache. The order of ideas by the `policy` sort is
//! `astrid_rules::filters`, not here.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Context, Result, ServiceError};
use crate::api::{endpoints, ApiError};
use crate::store::Store;

/// Which of its project's goals a task serves, directly or through a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct GoalLink {
    pub project_id: String,
    #[serde(default)]
    pub project_name: Option<String>,
    pub goal_id: String,
    pub statement: String,
    /// `task` for the task's own link, `list` for one inherited from a list.
    #[serde(default)]
    pub via: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    /// `person`, `agent` or `report`.
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TaskGoalLinks {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub links: Vec<GoalLink>,
}

/// One estimate, as the server answers it. A missing section is "not estimated", never zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub id: String,
    /// `builtin_llm`, `service:<key>` or `human`.
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_impacts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardrail_impacts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microcents: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_hours: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Plain text: show it as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// The built-in estimator's answer: show ranges under a "rough estimate" tag.
    #[serde(default)]
    pub rough: bool,
    /// `do-now`, `experiment-first`, `needs-review` or `dont`. `None` where no `tradeoff` policy is
    /// stated (a label this build does not know is kept, and a shell draws no chip for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Each reason is `{ code, …values }`, worded by the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_reasons: Option<Value>,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TaskEstimates {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub latest: Option<Estimate>,
    /// Newest first, at most 20.
    #[serde(default)]
    pub history: Vec<Estimate>,
}

/// An experiment link or a post-deploy verification window. The service's numbers are kept as
/// given: Astrid computes no statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ExperimentLink {
    pub id: String,
    /// `experiment` or `verification`.
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `pending`, `running`, `concluded`, `stopped` or `unknown`.
    #[serde(default)]
    pub state: String,
    /// `improving`, `flat`, `regressing` or `inconclusive`; `None` until there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// `{ action, reason }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decide_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    /// No data since `fetched_at` for 48 hours.
    #[serde(default)]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TaskExperiments {
    /// Not on the server's answer: the core adds it so a cached answer says whose it is.
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub experiments: Vec<ExperimentLink>,
    /// The task sits in the Pending results column.
    #[serde(default)]
    pub in_results_column: bool,
}

pub fn links_key(task_id: &str) -> String {
    format!("taskGoalLinks:{task_id}")
}

pub fn estimates_key(task_id: &str) -> String {
    format!("taskEstimates:{task_id}")
}

pub fn experiments_key(task_id: &str) -> String {
    format!("taskExperiments:{task_id}")
}

/// An answer, and whether it moved what is cached (the shell is told only when it did).
pub struct Answer<T> {
    pub value: T,
    pub changed: bool,
}

fn read<T: for<'de> Deserialize<'de>>(store: &Store, key: &str) -> Option<T> {
    store
        .metadata(key)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
}

fn write<T: Serialize + for<'de> Deserialize<'de> + PartialEq>(
    store: &Store,
    key: &str,
    value: &T,
) -> bool {
    if read::<T>(store, key).as_ref() == Some(value) {
        return false;
    }
    if let Ok(json) = serde_json::to_string(value) {
        let _ = store.set_metadata(key, &json);
    }
    true
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

pub struct GoalSignalsService {
    context: Context,
}

impl GoalSignalsService {
    pub fn new(context: Context) -> Self {
        GoalSignalsService { context }
    }

    pub fn cached_links(&self, task_id: &str) -> TaskGoalLinks {
        read(&self.context.store, &links_key(task_id)).unwrap_or_else(|| TaskGoalLinks {
            task_id: task_id.to_string(),
            ..TaskGoalLinks::default()
        })
    }

    pub fn cached_estimates(&self, task_id: &str) -> TaskEstimates {
        read(&self.context.store, &estimates_key(task_id)).unwrap_or_else(|| TaskEstimates {
            task_id: task_id.to_string(),
            ..TaskEstimates::default()
        })
    }

    pub fn cached_experiments(&self, task_id: &str) -> TaskExperiments {
        read(&self.context.store, &experiments_key(task_id)).unwrap_or_else(|| TaskExperiments {
            task_id: task_id.to_string(),
            ..TaskExperiments::default()
        })
    }

    fn decode<T: for<'de> Deserialize<'de>>(answer: &Value) -> Result<T> {
        serde_json::from_value(answer.clone())
            .map_err(|error| ServiceError::Api(ApiError::Decode(error.to_string())))
    }

    /// Fetch the task's goal links and cache the answer. Offline, the cache is the answer.
    pub async fn refresh_links(&self, task_id: &str) -> Result<Answer<TaskGoalLinks>> {
        let client = &self.context.client;
        match client
            .send(client.get(endpoints::task_goals(task_id)))
            .await
        {
            Ok(answer) => {
                let mut value: TaskGoalLinks = Self::decode(&answer)?;
                value.task_id = task_id.to_string();
                let changed = write(&self.context.store, &links_key(task_id), &value);
                Ok(Answer { value, changed })
            }
            Err(ApiError::Transport(_)) => Ok(Answer {
                value: self.cached_links(task_id),
                changed: false,
            }),
            Err(error) => Err(refusal(error)),
        }
    }

    /// Fetch the task's latest estimate and history. Offline, the cache is the answer.
    pub async fn refresh_estimates(&self, task_id: &str) -> Result<Answer<TaskEstimates>> {
        let client = &self.context.client;
        match client
            .send(client.get(endpoints::task_estimates(task_id)))
            .await
        {
            Ok(answer) => {
                let mut value: TaskEstimates = Self::decode(&answer)?;
                value.task_id = task_id.to_string();
                let changed = write(&self.context.store, &estimates_key(task_id), &value);
                Ok(Answer { value, changed })
            }
            Err(ApiError::Transport(_)) => Ok(Answer {
                value: self.cached_estimates(task_id),
                changed: false,
            }),
            Err(error) => Err(refusal(error)),
        }
    }

    /// Fetch the task's experiment links and verification windows. Offline, the cache is the answer.
    pub async fn refresh_experiments(&self, task_id: &str) -> Result<Answer<TaskExperiments>> {
        let client = &self.context.client;
        match client
            .send(client.get(endpoints::task_experiment(task_id)))
            .await
        {
            Ok(answer) => {
                let mut value: TaskExperiments = Self::decode(&answer)?;
                value.task_id = task_id.to_string();
                let changed = write(&self.context.store, &experiments_key(task_id), &value);
                Ok(Answer { value, changed })
            }
            Err(ApiError::Transport(_)) => Ok(Answer {
                value: self.cached_experiments(task_id),
                changed: false,
            }),
            Err(error) => Err(refusal(error)),
        }
    }
}
