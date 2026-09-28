//! One task waiting on another (task 69a840a4).
//!
//! The spec of record is `astrid-web/docs/specs/TASK_BLOCKING_DEPENDENCIES.md`. This is the client
//! half: reading a task's dependencies, adding one, removing one, and offering the candidates a
//! picker can choose from.
//!
//! ## Why the projection is cached rather than derived
//!
//! `GET /api/v1/tasks/:id` carries `blockedBy` / `blocks` as **ids**, and the ids alone are not
//! enough to draw the row. A chip needs a title, and an id with no task behind it is ambiguous in
//! exactly the way that matters: it means *either* "a task you cannot see" *or* "a task that has
//! not reached this cache yet". Resolving titles from the local `tasks` table would render the
//! second as the first, and tell a reader their own blocker is invisible to them.
//!
//! So the server's own projection — which knows which of the two it is, and says `hidden: true`
//! when it is the first — is what gets cached, under one `metadata` key per task. `metadata` is
//! cleared on sign-out with every other table, so nothing outlives the account that could see it.
//!
//! ## A blocker you cannot see still blocks
//!
//! `hidden` chips are counted and drawn, never dropped. Treating an invisible blocker as satisfied
//! would promote work that is genuinely not ready, and do it *because* of a permission boundary —
//! the worst available reason. The count is not a leak: the reader already knows something holds
//! their task. A title would be.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Context, Result, ServiceError};
use crate::api::endpoints;
use crate::outbox::{self, entry::kind, journal};

/// One end of a dependency, as the server projects it.
///
/// `hidden` and the absent title are one fact told two ways, because that is how the wire says it
/// (`V1Blocker` in `astrid-web/lib/api-contracts/v1-ios-shapes.ts`): a blocker the reader cannot
/// see arrives as an id and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The short id a chip is prefixed with (`AWTD-1007`), where the task has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub hidden: bool,
}

impl Blocker {
    /// A chip for an id the reader cannot resolve to a task.
    pub fn hidden(id: impl Into<String>) -> Self {
        Blocker {
            id: id.into(),
            hidden: true,
            ..Blocker::default()
        }
    }
}

/// Both directions, plus what a picker must not offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Dependencies {
    /// Tasks this one is waiting on.
    #[serde(default)]
    pub blocked_by: Vec<Blocker>,
    /// Tasks waiting on this one.
    #[serde(default)]
    pub blocks: Vec<Blocker>,
    /// Every visible task that waits on this one, transitively — the ones a write would refuse
    /// with `409 dependency_cycle`. A picker drops them rather than offering a choice that will
    /// be refused.
    #[serde(default)]
    pub dependent_ids: Vec<String>,
}

/// The metadata key one task's projection is cached under.
pub fn cache_key(task_id: &str) -> String {
    format!("blockers.{task_id}")
}

/// Reading and changing what a task waits on.
pub struct TaskDependencyService {
    context: Context,
}

impl TaskDependencyService {
    pub fn new(context: Context) -> Self {
        TaskDependencyService { context }
    }

    /// What is cached for this task, or nothing.
    ///
    /// The row draws from this first so it appears at once and survives being offline. An
    /// unreadable cache entry reads as nothing cached rather than as an error: a projection
    /// written by a newer build is not worth failing a detail screen over.
    pub fn cached(&self, task_id: &str) -> Option<Dependencies> {
        self.context
            .store
            .metadata(&cache_key(task_id))
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
    }

    /// Fetch the projection and cache it.
    pub async fn refresh(&self, task_id: &str) -> Result<Dependencies> {
        let value = self
            .context
            .client
            .send(self.context.client.get(endpoints::task_blockers(task_id)))
            .await?;
        let dependencies: Dependencies = serde_json::from_value(value).map_err(|error| {
            ServiceError::Api(crate::api::ApiError::Decode(format!(
                "unreadable blockers: {error}"
            )))
        })?;
        self.store(task_id, &dependencies);
        Ok(dependencies)
    }

    /// Wait on another task — sent at once, queued only when the network failed (CONTRACTS D32).
    ///
    /// The server refuses a cycle (`409 dependency_cycle`), and that refusal has to reach the
    /// person who picked it rather than become a dead letter nobody sees; online, its answer is
    /// cached and returned. Offline the write is [`Self::add`]'s: optimistic and journalled, which
    /// is what makes blocking work on a plane.
    pub async fn add_now(&self, task_id: &str, blocking_task_id: &str) -> Result<Dependencies> {
        let request = self
            .context
            .client
            .post(endpoints::task_blockers(task_id))
            .value(json!({ "blockingTaskId": blocking_task_id }));
        match self.try_now(task_id, request).await? {
            Some(answer) => Ok(self.take_answer(task_id, &answer)),
            None => self.add(task_id, blocking_task_id),
        }
    }

    /// Stop waiting on another task — the same shape as [`Self::add_now`]. Removing a link that
    /// is already gone is the outcome asked for.
    pub async fn remove_now(&self, task_id: &str, blocking_task_id: &str) -> Result<Dependencies> {
        let request = self
            .context
            .client
            .delete(endpoints::task_blocker(task_id, blocking_task_id));
        match self.try_now(task_id, request).await {
            Ok(Some(answer)) => Ok(self.take_answer(task_id, &answer)),
            Ok(None) => self.remove(task_id, blocking_task_id),
            Err(ServiceError::Api(error)) if error.status() == Some(404) => {
                let mut dependencies = self.cached(task_id).unwrap_or_default();
                dependencies
                    .blocked_by
                    .retain(|blocker| blocker.id != blocking_task_id);
                self.store(task_id, &dependencies);
                self.mirror_onto_task(task_id, &dependencies);
                Ok(dependencies)
            }
            Err(error) => Err(error),
        }
    }

    /// Send now: `Some(answer)` when the server took it, `None` when it must be queued (the
    /// network failed, or an earlier change to this task is still waiting and must go first).
    async fn try_now(
        &self,
        task_id: &str,
        request: crate::api::Request,
    ) -> Result<Option<serde_json::Value>> {
        let queued = journal::all(&self.context.store)?.iter().any(|entry| {
            matches!(
                entry.kind.as_str(),
                kind::ADD_TASK_BLOCKER | kind::REMOVE_TASK_BLOCKER
            ) && matches!(
                entry.status,
                crate::outbox::Status::Pending | crate::outbox::Status::Running
            ) && entry.payload.get("taskId").and_then(|id| id.as_str()) == Some(task_id)
        });
        if queued {
            return Ok(None);
        }
        match self.context.client.send(request).await {
            Ok(answer) => Ok(Some(answer)),
            Err(crate::api::ApiError::Transport(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// The server's answer to a change — `{ taskId, blockingTaskId, blockedBy }` — cached over
    /// what this task waits on; what waits on it is unchanged by an edge it did not touch.
    fn take_answer(&self, task_id: &str, answer: &serde_json::Value) -> Dependencies {
        let mut dependencies = self.cached(task_id).unwrap_or_default();
        if let Some(blocked_by) = answer
            .get("blockedBy")
            .and_then(|value| serde_json::from_value::<Vec<Blocker>>(value.clone()).ok())
        {
            dependencies.blocked_by = blocked_by;
        }
        self.store(task_id, &dependencies);
        self.mirror_onto_task(task_id, &dependencies);
        dependencies
    }

    /// Wait on another task, offline.
    ///
    /// Optimistic, then journalled. The chip appears immediately with whatever this cache knows
    /// about the blocker — a title if the task is local, `hidden` if it is not, which the refresh
    /// after the write corrects.
    pub fn add(&self, task_id: &str, blocking_task_id: &str) -> Result<Dependencies> {
        let mut dependencies = self.cached(task_id).unwrap_or_default();
        if !dependencies
            .blocked_by
            .iter()
            .any(|blocker| blocker.id == blocking_task_id)
        {
            dependencies
                .blocked_by
                .push(self.optimistic_chip(blocking_task_id));
        }
        self.store(task_id, &dependencies);
        self.mirror_onto_task(task_id, &dependencies);

        let entry = outbox::build(
            kind::ADD_TASK_BLOCKER,
            json!({
                "taskId": task_id,
                "blockingTaskId": blocking_task_id,
                "body": { "blockingTaskId": blocking_task_id },
            }),
            &outbox::new_temp_id(),
            self.context.clock.now(),
        );
        journal::enqueue(&self.context.store, &entry)?;
        Ok(dependencies)
    }

    /// Stop waiting on another task.
    pub fn remove(&self, task_id: &str, blocking_task_id: &str) -> Result<Dependencies> {
        let mut dependencies = self.cached(task_id).unwrap_or_default();
        dependencies
            .blocked_by
            .retain(|blocker| blocker.id != blocking_task_id);
        self.store(task_id, &dependencies);
        self.mirror_onto_task(task_id, &dependencies);

        let entry = outbox::build(
            kind::REMOVE_TASK_BLOCKER,
            json!({ "taskId": task_id, "blockingTaskId": blocking_task_id }),
            &outbox::new_temp_id(),
            self.context.clock.now(),
        );
        journal::enqueue(&self.context.store, &entry)?;
        Ok(dependencies)
    }

    /// The chip to draw the instant a blocker is picked, before the server has answered.
    ///
    /// The picker searched the server, so the task is one the reader can see — but this cache may
    /// not hold it, and a chip with no title is better than a wrong one. The refresh that follows
    /// the write replaces this either way.
    fn optimistic_chip(&self, blocking_task_id: &str) -> Blocker {
        match self.context.store.task(blocking_task_id) {
            Ok(Some(task)) => Blocker {
                id: task.id,
                title: Some(task.title),
                identifier: task.identifier,
                completed: task.completed,
                hidden: false,
            },
            _ => Blocker::hidden(blocking_task_id),
        }
    }

    fn store(&self, task_id: &str, dependencies: &Dependencies) {
        if let Ok(json) = serde_json::to_string(dependencies) {
            let _ = self.context.store.set_metadata(&cache_key(task_id), &json);
        }
    }

    /// Keep the task's own id array in step with the projection.
    ///
    /// Two places hold the same fact, because the wire puts it in two places: the ids ride on the
    /// task so a list view can know a task is blocked without a second request, and the projection
    /// holds the titles. Writing only the projection would leave a row whose chips disagree with
    /// the task the rest of the app is reading.
    fn mirror_onto_task(&self, task_id: &str, dependencies: &Dependencies) {
        if let Ok(Some(mut task)) = self.context.store.task(task_id) {
            task.blocked_by = Some(
                dependencies
                    .blocked_by
                    .iter()
                    .map(|blocker| blocker.id.clone())
                    .collect(),
            );
            let _ = self.context.store.upsert_task(&task);
        }
    }
}

/// What a picker may offer, out of what a search returned.
///
/// Three filters and nothing else, mirroring the spec: never the task itself, never something
/// already linked, and never anything that waits on this task transitively — offering one of those
/// is offering a choice the server will refuse with `409 dependency_cycle`.
///
/// Ranking, not filtering, puts the same board first: most blockers are neighbours, and a
/// cross-board blocker stays one search away. `sort_by_key` is stable, so within each group the
/// search's own relevance order survives.
pub fn pickable<'a>(
    results: &'a [crate::model::Task],
    task_id: &str,
    dependencies: &Dependencies,
    board_list_ids: &[String],
) -> Vec<&'a crate::model::Task> {
    let linked: std::collections::HashSet<&str> = dependencies
        .blocked_by
        .iter()
        .map(|blocker| blocker.id.as_str())
        .chain(dependencies.dependent_ids.iter().map(String::as_str))
        .collect();

    let mut offerable: Vec<&crate::model::Task> = results
        .iter()
        .filter(|candidate| candidate.id != task_id)
        .filter(|candidate| !linked.contains(candidate.id.as_str()))
        .collect();
    offerable.sort_by_key(|candidate| {
        let on_this_board = candidate
            .effective_list_ids()
            .iter()
            .any(|id| board_list_ids.contains(id));
        u8::from(!on_this_board)
    });
    offerable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Task;

    fn blocker(id: &str) -> Blocker {
        Blocker {
            id: id.into(),
            title: Some(id.into()),
            ..Blocker::default()
        }
    }

    fn task_on(id: &str, list_ids: &[&str]) -> Task {
        let mut task = Task::new(id, id);
        task.list_ids = Some(list_ids.iter().map(|id| id.to_string()).collect());
        task
    }

    /// A blocker the reader cannot see arrives as an id and nothing else, and is still a blocker.
    /// Read back off the wire rather than constructed, because "absent title" is the shape the
    /// server actually sends.
    #[test]
    fn a_hidden_blocker_survives_the_wire_and_still_counts_task_69a840a4() {
        let dependencies: Dependencies = serde_json::from_value(json!({
            "blockedBy": [{ "id": "t9", "hidden": true }],
            "blocks": [],
            "dependentIds": [],
        }))
        .expect("the wire shape");
        assert_eq!(dependencies.blocked_by.len(), 1);
        assert!(dependencies.blocked_by[0].hidden);
        assert_eq!(dependencies.blocked_by[0].title, None);
    }

    /// The projection tolerates a response with only the field it needs. `meta` rides along on
    /// every v1 envelope and is not this type's business.
    #[test]
    fn the_projection_ignores_the_envelope_it_does_not_need_task_69a840a4() {
        let dependencies: Dependencies = serde_json::from_value(json!({
            "blockedBy": [{ "id": "t1", "title": "Ship it", "identifier": "AWTD-1", "completed": true }],
            "blocks": [],
            "dependentIds": ["t4"],
            "meta": { "requestId": "abc" },
        }))
        .expect("the wire shape");
        assert_eq!(
            dependencies.blocked_by[0].identifier.as_deref(),
            Some("AWTD-1")
        );
        assert!(dependencies.blocked_by[0].completed);
        assert_eq!(dependencies.dependent_ids, vec!["t4".to_string()]);
    }

    /// The picker never offers the task itself, anything already linked, or anything that waits on
    /// this task — the last because the server would refuse the write as a cycle.
    #[test]
    fn the_picker_offers_neither_a_cycle_nor_a_duplicate_task_69a840a4() {
        let results = vec![
            task_on("me", &["l1"]),
            task_on("already", &["l1"]),
            task_on("would-cycle", &["l1"]),
            task_on("fine", &["l1"]),
        ];
        let dependencies = Dependencies {
            blocked_by: vec![blocker("already")],
            blocks: vec![],
            dependent_ids: vec!["would-cycle".into()],
        };
        let offered: Vec<&str> = pickable(&results, "me", &dependencies, &["l1".into()])
            .iter()
            .map(|task| task.id.as_str())
            .collect();
        assert_eq!(offered, vec!["fine"]);
    }

    /// Same board first, and that is a RANKING: a task from another board is still offered, just
    /// lower. Nothing here caps the list — no slice, no "+n more".
    #[test]
    fn the_same_board_ranks_first_without_excluding_the_rest_task_69a840a4() {
        let results = vec![
            task_on("elsewhere", &["l9"]),
            task_on("neighbour", &["l1"]),
            task_on("also-elsewhere", &["l8"]),
            task_on("also-neighbour", &["l1"]),
        ];
        let offered: Vec<&str> = pickable(&results, "me", &Dependencies::default(), &["l1".into()])
            .iter()
            .map(|task| task.id.as_str())
            .collect();
        assert_eq!(
            offered,
            vec!["neighbour", "also-neighbour", "elsewhere", "also-elsewhere"],
            "ranked, not filtered, and stable within each group"
        );
    }

    /// One key per task, so two detail screens open at once do not overwrite each other's cache.
    #[test]
    fn the_cache_key_is_per_task_task_69a840a4() {
        assert_ne!(cache_key("t1"), cache_key("t2"));
        assert!(cache_key("t1").contains("t1"));
    }
}
