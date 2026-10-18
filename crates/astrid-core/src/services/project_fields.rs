//! A GitHub board's project fields in task detail (AITD-491; web AWTD-1208, AWTD-1209, AWTD-1210).
//!
//! The spec of record is `astrid-web/docs/specs/GITHUB_PROJECTS_WHITELABEL.md` §9.4. Fields are
//! DEFINED in GitHub — no client offers a way to create one — and on a GitHub-backed board every
//! one of them that is not mapped onto a native column is shown and editable in task detail.
//!
//! ## Why neither half rides the task cache
//!
//! The **schema** — names, types, options, iterations — is served once per project by
//! `GET /api/v1/projects/:id/fields`, so a task's values can stay a compact
//! `{ "<fieldId>": <scalar> }` map. The **values** ride `GET /api/v1/tasks/:id` but not the list
//! route, which carries them only on `?include=fields` because a thousand tasks' worth would break
//! its payload budget (web `lib/tasks/v1-task-fields.ts`). So a `fields` key on the cached `Task`
//! would be filled for the tasks someone happened to open and absent for the rest, and absent
//! would read as "no values". Both halves are cached here instead, under `metadata` keys, the way
//! `dependency` caches its projection: the screen draws from the cache at once, works offline,
//! and the round trip corrects it.
//!
//! ## Which fields show
//!
//! The same two rules as web `lib/project-field-display.ts`, which is the reference:
//!
//! - **A mapped field never shows.** Status, priority and due are native columns with their own
//!   rows already; a second control would be two ways to set one value, and the server refuses
//!   the write (`field_is_mapped`) anyway.
//! - **An unknown type is dropped, not drawn blank.** A newer server talking to this build is
//!   exactly when a row with no editor would appear.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use super::{Context, Result, ServiceError};
use crate::api::{endpoints, ApiError};
use crate::outbox::{self, entry::kind, journal};
use crate::store::Store;

/// The five types the server normalises. Anything else has no editor.
pub const EDITABLE_TYPES: [&str; 5] = ["text", "number", "date", "single_select", "iteration"];

/// One field as `GET /api/v1/projects/:id/fields` serves it (web `ProjectFieldSchemaEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectField {
    pub id: String,
    #[serde(default)]
    pub remote_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub data_type: String,
    /// A single-select's choices. The server stores this column as JSON, so anything that is not
    /// an array of options reads as none rather than failing the whole schema.
    #[serde(default, deserialize_with = "lenient_list")]
    pub options: Vec<FieldOption>,
    #[serde(default, deserialize_with = "lenient_list")]
    pub iterations: Vec<FieldIteration>,
    /// `status`, `priority` or `due` when the field is edited through its native column.
    #[serde(default)]
    pub mapped_to: Option<String>,
    #[serde(default)]
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FieldOption {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FieldIteration {
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub start_date: String,
    /// In days.
    #[serde(default)]
    pub duration: i64,
    /// Over. Not on the wire: worked out here, by [`order_iterations`].
    #[serde(default)]
    pub completed: bool,
}

/// One row of task detail: a field, and this task's value for it (`None` is no value).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldRow {
    #[serde(flatten)]
    pub field: ProjectField,
    pub value: Option<Value>,
}

/// What task detail draws. No rows on a task that is not on a GitHub board, which is every task
/// on a classic list — so their detail screens are unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskProjectFields {
    pub project_id: Option<String>,
    pub rows: Vec<FieldRow>,
}

fn lenient_list<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        _ => Vec::new(),
    })
}

pub fn schema_key(project_id: &str) -> String {
    format!("projectFields:{project_id}")
}

pub fn values_key(task_id: &str) -> String {
    format!("taskFieldValues:{task_id}")
}

/// The fields to draw, in `position` order: never a mapped one, never an unknown type.
pub fn visible(fields: &[ProjectField]) -> Vec<ProjectField> {
    let mut shown: Vec<ProjectField> = fields
        .iter()
        .filter(|field| {
            field.mapped_to.is_none() && EDITABLE_TYPES.contains(&field.data_type.as_str())
        })
        .cloned()
        .collect();
    shown.sort_by_key(|field| field.position);
    shown
}

/// An iteration field's choices: current and upcoming first, completed after, each by start date.
///
/// GitHub keeps completed iterations in the same array as the live ones, so start date alone
/// would put a year-old sprint above this week's. Completed ones are KEPT, not dropped: a task
/// still pointing at one must draw its title, not a bare id. An iteration runs `duration` days
/// from `startDate` inclusive, so one ending today is current and is completed the day after —
/// web `orderIterationChoices`, and measured in UTC as it is.
pub fn order_iterations(iterations: &[FieldIteration], now: DateTime<Utc>) -> Vec<FieldIteration> {
    let mut marked: Vec<FieldIteration> = iterations
        .iter()
        .map(|iteration| {
            let completed = NaiveDate::parse_from_str(&iteration.start_date, "%Y-%m-%d")
                .ok()
                .and_then(|day| day.and_hms_opt(0, 0, 0))
                .map(|start| now.naive_utc() >= start + Duration::days(iteration.duration))
                .unwrap_or(false);
            FieldIteration {
                completed,
                ..iteration.clone()
            }
        })
        .collect();
    marked.sort_by(|a, b| (a.completed, &a.start_date).cmp(&(b.completed, &b.start_date)));
    marked
}

/// Is `value` one the server can take for a field of this type? `None` (clear) always is.
///
/// The server checks again — and also that an option or iteration exists — but a value of the
/// wrong JSON type is caught here, before it is journalled to be refused later, unseen.
pub fn accepts(data_type: &str, value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(Value::String(_)) => data_type != "number",
        Some(Value::Number(_)) => data_type == "number",
        Some(_) => false,
    }
}

/// Put one value into the cached map, or take it out for `None`.
pub fn store_value(store: &Store, task_id: &str, field_id: &str, value: Option<Value>) {
    let key = values_key(task_id);
    let mut values: BTreeMap<String, Value> = store
        .metadata(&key)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    match value {
        Some(value) => {
            values.insert(field_id.to_string(), value);
        }
        None => {
            values.remove(field_id);
        }
    }
    if let Ok(json) = serde_json::to_string(&values) {
        let _ = store.set_metadata(&key, &json);
    }
}

/// The server's refusal code — `auth_required`, `sso_required`, `field_is_mapped`… — rather than
/// the whole body, so the shell can pick its words from it.
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

pub struct ProjectFieldService {
    context: Context,
}

impl ProjectFieldService {
    pub fn new(context: Context) -> Self {
        ProjectFieldService { context }
    }

    /// The GitHub board a task is on, if any — a task is on at most one.
    fn board_of(&self, task_id: &str) -> Result<Option<String>> {
        let Some(task) = self.context.store.task(task_id)? else {
            return Ok(None);
        };
        for list_id in task.effective_list_ids() {
            if let Some(list) = self.context.store.list(&list_id)? {
                if list.is_github_backed() {
                    if let Some(project_id) = list.project_id {
                        return Ok(Some(project_id));
                    }
                }
            }
        }
        Ok(None)
    }

    fn read<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.context
            .store
            .metadata(key)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
    }

    fn write<T: Serialize>(&self, key: &str, value: &T) {
        if let Ok(json) = serde_json::to_string(value) {
            let _ = self.context.store.set_metadata(key, &json);
        }
    }

    fn rows(&self, project_id: Option<String>, task_id: &str) -> TaskProjectFields {
        let Some(project_id) = project_id else {
            return TaskProjectFields::default();
        };
        let schema: Vec<ProjectField> = self.read(&schema_key(&project_id)).unwrap_or_default();
        let values: BTreeMap<String, Value> = self.read(&values_key(task_id)).unwrap_or_default();
        let now = self.context.clock.now();
        let rows = visible(&schema)
            .into_iter()
            .map(|mut field| {
                field.iterations = order_iterations(&field.iterations, now);
                let value = values.get(&field.id).cloned();
                FieldRow { field, value }
            })
            .collect();
        TaskProjectFields {
            project_id: Some(project_id),
            rows,
        }
    }

    /// What is cached for this task. Draws at once, and with no network.
    pub fn cached(&self, task_id: &str) -> Result<TaskProjectFields> {
        let task_id = self.context.store.resolve_id(task_id)?;
        let project_id = self.board_of(&task_id)?;
        Ok(self.rows(project_id, &task_id))
    }

    /// Fetch the schema and this task's values, cache both, and answer with the rows.
    ///
    /// Offline, or on a server without the capability (`404`), the cache is the answer: neither
    /// is worth an error state, and the pane simply shows what it last knew — or nothing.
    pub async fn refresh(&self, task_id: &str) -> Result<TaskProjectFields> {
        let task_id = self.context.store.resolve_id(task_id)?;
        let Some(project_id) = self.board_of(&task_id)? else {
            return Ok(TaskProjectFields::default());
        };
        let client = &self.context.client;
        match client
            .send(client.get(endpoints::project_fields(&project_id)))
            .await
        {
            Ok(answer) => {
                let fields: Vec<ProjectField> = answer
                    .get("fields")
                    .cloned()
                    .and_then(|fields| serde_json::from_value(fields).ok())
                    .unwrap_or_default();
                self.write(&schema_key(&project_id), &fields);
            }
            Err(error) if error.status() == Some(404) => {
                self.write(&schema_key(&project_id), &Vec::<ProjectField>::new());
            }
            Err(_) => return Ok(self.rows(Some(project_id), &task_id)),
        }
        if !crate::model::is_temp_id(&task_id) && !self.queued(&task_id)? {
            if let Ok(answer) = client.send(client.get(endpoints::task(&task_id))).await {
                let values: BTreeMap<String, Value> = answer
                    .get("task")
                    .and_then(|task| task.get("fields"))
                    .cloned()
                    .and_then(|fields| serde_json::from_value(fields).ok())
                    .unwrap_or_default();
                self.write(&values_key(&task_id), &values);
            }
        }
        Ok(self.rows(Some(project_id), &task_id))
    }

    /// Is an edit to this task's fields still waiting in the journal? Then a fetch would answer
    /// with the value from before it, and a later edit must queue behind it.
    fn queued(&self, task_id: &str) -> Result<bool> {
        Ok(journal::all(&self.context.store)?.iter().any(|entry| {
            entry.kind == kind::SET_TASK_FIELD
                && matches!(
                    entry.status,
                    crate::outbox::Status::Pending | crate::outbox::Status::Running
                )
                && entry.payload.get("taskId").and_then(Value::as_str) == Some(task_id)
        }))
    }

    /// Set one field — sent at once, queued only when the network failed (CONTRACTS D56).
    ///
    /// Online, a refusal reaches the person who made the edit: `auth_required` and `sso_required`
    /// are things they can act on, and a dead letter would be seen by nobody. Offline the edit is
    /// optimistic and journalled, as every other write is.
    pub async fn set_now(
        &self,
        task_id: &str,
        field_id: &str,
        value: Option<Value>,
    ) -> Result<TaskProjectFields> {
        let task_id = self.context.store.resolve_id(task_id)?;
        let project_id = self.board_of(&task_id)?;
        let fields = self.rows(project_id.clone(), &task_id);
        let Some(row) = fields.rows.iter().find(|row| row.field.id == field_id) else {
            return Err(ServiceError::NotFound {
                kind: "project field",
                id: field_id.to_string(),
            });
        };
        if !accepts(&row.field.data_type, value.as_ref()) {
            return Err(ServiceError::Api(ApiError::Http {
                status: 400,
                message: "field_type_mismatch".into(),
            }));
        }
        if crate::model::is_temp_id(&task_id) || self.queued(&task_id)? {
            return self.set(&task_id, field_id, value, project_id);
        }
        let request = self
            .context
            .client
            .put(endpoints::task_field(&task_id, field_id))
            .value(json!({ "value": value }));
        match self.context.client.send(request).await {
            Ok(answer) => {
                let kept = answer
                    .get("field")
                    .and_then(|field| field.get("value"))
                    .cloned()
                    .filter(|value| !value.is_null());
                store_value(&self.context.store, &task_id, field_id, kept);
                Ok(self.rows(project_id, &task_id))
            }
            Err(ApiError::Transport(_)) => self.set(&task_id, field_id, value, project_id),
            Err(error) => Err(refusal(error)),
        }
    }

    fn set(
        &self,
        task_id: &str,
        field_id: &str,
        value: Option<Value>,
        project_id: Option<String>,
    ) -> Result<TaskProjectFields> {
        store_value(&self.context.store, task_id, field_id, value.clone());
        let entry = outbox::build(
            kind::SET_TASK_FIELD,
            json!({ "taskId": task_id, "fieldId": field_id, "body": { "value": value } }),
            &outbox::new_temp_id(),
            self.context.clock.now(),
        );
        journal::enqueue(&self.context.store, &entry)?;
        Ok(self.rows(project_id, task_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(json: Value) -> ProjectField {
        serde_json::from_value(json).expect("a field")
    }

    /// Web `visibleProjectFields`: mapped fields and unknown types never show; order is `position`.
    #[test]
    fn aitd_491_only_unmapped_editable_fields_show_in_position_order() {
        let shown = visible(&[
            field(json!({ "id": "f3", "name": "Size", "dataType": "number", "position": 3 })),
            field(
                json!({ "id": "f1", "name": "Status", "dataType": "single_select", "mappedTo": "status", "position": 1 }),
            ),
            field(json!({ "id": "f2", "name": "Notes", "dataType": "text", "position": 2 })),
            field(json!({ "id": "f4", "name": "Labels", "dataType": "labels", "position": 0 })),
        ]);
        let ids: Vec<&str> = shown.iter().map(|field| field.id.as_str()).collect();
        assert_eq!(ids, ["f2", "f3"]);
    }

    /// The options column is JSON on the server; a shape this build cannot read is no options,
    /// not an unreadable schema.
    #[test]
    fn aitd_491_options_of_an_unexpected_shape_read_as_none() {
        let odd = field(
            json!({ "id": "f1", "dataType": "single_select", "options": { "x": 1 }, "iterations": null }),
        );
        assert!(odd.options.is_empty());
        assert!(odd.iterations.is_empty());
        let fine = field(
            json!({ "id": "f1", "dataType": "single_select", "options": [{ "id": "o1", "name": "Low", "color": "GREEN" }] }),
        );
        assert_eq!(fine.options[0].name, "Low");
    }

    /// Web `orderIterationChoices`: current and upcoming by start date, then completed; one that
    /// ends today is still current.
    #[test]
    fn aitd_491_completed_iterations_sort_after_the_live_ones_and_are_kept() {
        let iteration = |id: &str, start: &str, duration: i64| FieldIteration {
            id: id.into(),
            title: id.into(),
            start_date: start.into(),
            duration,
            completed: false,
        };
        let now = "2026-10-17T12:00:00Z".parse().expect("a time");
        let ordered = order_iterations(
            &[
                iteration("next", "2026-10-20", 14),
                iteration("old", "2025-01-01", 14),
                iteration("now", "2026-10-06", 14),
                iteration("ended-yesterday", "2026-10-03", 14),
            ],
            now,
        );
        let ids: Vec<&str> = ordered.iter().map(|it| it.id.as_str()).collect();
        assert_eq!(ids, ["now", "next", "old", "ended-yesterday"]);
        assert!(!ordered[0].completed);
        assert!(ordered[3].completed);
    }

    #[test]
    fn aitd_491_a_value_of_the_wrong_json_type_is_caught_before_it_is_sent() {
        assert!(accepts("number", Some(&json!(3))));
        assert!(!accepts("number", Some(&json!("3"))));
        assert!(accepts("text", Some(&json!("hi"))));
        assert!(!accepts("single_select", Some(&json!(1))));
        assert!(accepts("date", None));
        assert!(!accepts("text", Some(&json!(true))));
    }
}
