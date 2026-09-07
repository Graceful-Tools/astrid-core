//! Tasks: creating, editing, completing, deleting.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/TaskService.swift`.
//!
//! ## Completion is the rule this file exists to protect
//!
//! Rule 2 of `docs/ASTRID.md` §0: a task is completed **only** through [`TaskService::complete`].
//! An `update(completed: true)` writes the flag and skips the rollover, so a daily task completed
//! that way is simply done forever — and nothing about the code that did it looks wrong. That is
//! why the two paths are separate functions here and why the rollover is not an option on the
//! update path.
//!
//! Rule 3 is the other half: when the completion comes from a view that lets the user edit the due
//! date first, the rollover has to anchor on **what they are looking at**, not on what the cache
//! last heard from the server. [`TaskService::complete`] takes the on-screen task for that reason,
//! and the fields it copies across are the ones the next occurrence is computed from.
//!
//! ## Nothing here talks to the network
//!
//! Every write is: update the cache, journal the entry, return. The Outbox delivers it, whenever
//! that turns out to be. A service that awaited a response before returning would be a service
//! that fails when a train enters a tunnel.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{Context, Result, ServiceError};
use crate::model::{
    date, CustomRepeatingPattern, Priority, RepeatFromMode, Repeating, Task, TaskList,
};
use crate::outbox::{self, journal, kind};
use crate::repeating;

/// What a new task is made of.
///
/// A draft rather than a `Task`: the id does not exist yet, and half of `Task` is server-owned.
#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub list_ids: Vec<String>,
    pub priority: Priority,
    pub due_date_time: Option<DateTime<Utc>>,
    pub is_all_day: bool,
    pub assignee_id: Option<String>,
    pub repeating: Option<Repeating>,
    pub repeating_data: Option<CustomRepeatingPattern>,
    pub repeat_from: Option<RepeatFromMode>,
    pub is_private: bool,
    pub parent_task_id: Option<String>,
    pub status_role: Option<String>,
}

impl TaskDraft {
    pub fn new(title: impl Into<String>) -> Self {
        TaskDraft {
            title: title.into(),
            // All-day unless a time is chosen — the same default the model decodes with, so a task
            // created here and one fetched back read identically.
            is_all_day: true,
            ..Default::default()
        }
    }

    pub fn in_list(mut self, list_id: impl Into<String>) -> Self {
        self.list_ids.push(list_id.into());
        self
    }

    pub fn due(mut self, at: DateTime<Utc>, all_day: bool) -> Self {
        self.due_date_time = Some(at);
        self.is_all_day = all_day;
        self
    }
}

/// An edit.
///
/// Every field is doubly optional where clearing is possible, and the two layers mean different
/// things: the outer `None` is "do not touch this", `Some(None)` is "clear it". The distinction is
/// not pedantry — "remove the due date" and "leave the due date alone" are both common, they reach
/// the server as `null` and as absent, and a type that cannot tell them apart makes one of them
/// impossible to express.
#[derive(Debug, Clone, Default)]
pub struct TaskChanges {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<Priority>,
    pub due_date_time: Option<Option<DateTime<Utc>>>,
    /// When to be reminded. `Some(None)` clears it, which is how a reminder is turned off rather
    /// than left in the past.
    pub reminder_time: Option<Option<DateTime<Utc>>>,
    pub is_all_day: Option<bool>,
    pub completed: Option<bool>,
    pub completed_at: Option<Option<DateTime<Utc>>>,
    pub assignee_id: Option<Option<String>>,
    pub repeating: Option<Option<Repeating>>,
    pub repeating_data: Option<Option<CustomRepeatingPattern>>,
    pub repeat_from: Option<RepeatFromMode>,
    pub occurrence_count: Option<i64>,
    pub list_ids: Option<Vec<String>>,
    pub parent_task_id: Option<Option<String>>,
    pub status_role: Option<Option<String>>,
    pub is_private: Option<bool>,
    pub timer_duration: Option<Option<i64>>,
    pub last_timer_value: Option<Option<String>>,
}

impl TaskChanges {
    pub fn title(title: impl Into<String>) -> Self {
        TaskChanges {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    /// Fold the changes into a task, so the cache shows what the user just did.
    pub fn apply(&self, task: &mut Task) {
        if let Some(value) = &self.title {
            task.title = value.clone();
        }
        if let Some(value) = &self.description {
            task.description = value.clone();
        }
        if let Some(value) = self.priority {
            task.priority = value;
        }
        if let Some(value) = self.due_date_time {
            task.due_date_time = value;
        }
        if let Some(value) = self.reminder_time {
            task.reminder_time = value;
            // A moved reminder has not been sent yet, whatever the server last said about the one
            // before it.
            task.reminder_sent = Some(false);
        }
        if let Some(value) = self.is_all_day {
            task.is_all_day = value;
        }
        if let Some(value) = self.completed {
            task.completed = value;
        }
        if let Some(value) = self.completed_at {
            task.completed_at = value;
        }
        if let Some(value) = &self.assignee_id {
            task.assignee_id = value.clone();
            if value.is_none() {
                task.assignee = None;
            }
        }
        if let Some(value) = self.repeating {
            task.repeating = value;
        }
        if let Some(value) = &self.repeating_data {
            task.repeating_data = value.clone();
        }
        if let Some(value) = self.repeat_from {
            task.repeat_from = Some(value);
        }
        if let Some(value) = self.occurrence_count {
            task.occurrence_count = Some(value);
        }
        if let Some(value) = &self.list_ids {
            task.list_ids = Some(value.clone());
            // The embedded list objects are now stale, and a row that renders chips from them
            // would show the list the task just left.
            task.lists = None;
        }
        if let Some(value) = &self.parent_task_id {
            task.parent_task_id = value.clone();
        }
        if let Some(value) = &self.status_role {
            task.status_role = value.clone();
        }
        if let Some(value) = self.is_private {
            task.is_private = value;
        }
        if let Some(value) = self.timer_duration {
            task.timer_duration = value;
        }
        if let Some(value) = &self.last_timer_value {
            task.last_timer_value = value.clone();
        }
    }

    /// The request body. Only what was touched appears; a cleared field appears as `null`.
    pub fn to_body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        let mut set = |key: &str, value: serde_json::Value| {
            body.insert(key.to_string(), value);
        };
        if let Some(value) = &self.title {
            set("title", json!(value));
        }
        if let Some(value) = &self.description {
            set("description", json!(value));
        }
        if let Some(value) = self.priority {
            set("priority", json!(value.as_i64()));
        }
        if let Some(value) = self.due_date_time {
            set("dueDateTime", json!(value.map(date::format)));
        }
        if let Some(value) = self.reminder_time {
            set("reminderTime", json!(value.map(date::format)));
        }
        if let Some(value) = self.is_all_day {
            set("isAllDay", json!(value));
        }
        if let Some(value) = self.completed {
            set("completed", json!(value));
        }
        if let Some(value) = self.completed_at {
            set("completedAt", json!(value.map(date::format)));
        }
        if let Some(value) = &self.assignee_id {
            set("assigneeId", json!(value));
        }
        if let Some(value) = self.repeating {
            set("repeating", json!(value));
        }
        if let Some(value) = &self.repeating_data {
            set("repeatingData", json!(value));
        }
        if let Some(value) = self.repeat_from {
            set("repeatFrom", json!(value));
        }
        if let Some(value) = self.occurrence_count {
            set("occurrenceCount", json!(value));
        }
        if let Some(value) = &self.list_ids {
            set("listIds", json!(value));
        }
        if let Some(value) = &self.parent_task_id {
            set("parentTaskId", json!(value));
        }
        if let Some(value) = &self.status_role {
            set("statusRole", json!(value));
        }
        if let Some(value) = self.is_private {
            set("isPrivate", json!(value));
        }
        if let Some(value) = self.timer_duration {
            set("timerDuration", json!(value));
        }
        if let Some(value) = &self.last_timer_value {
            set("lastTimerValue", json!(value));
        }
        serde_json::Value::Object(body)
    }
}

pub struct TaskService {
    context: Context,
}

impl TaskService {
    pub fn new(context: Context) -> Self {
        TaskService { context }
    }

    // ─── Reads ────────────────────────────────────────────────────────────────────────────────

    pub fn task(&self, id: &str) -> Result<Option<Task>> {
        Ok(self.context.store.task(id)?)
    }

    pub fn tasks_in_list(&self, list_id: &str) -> Result<Vec<Task>> {
        Ok(self.context.store.tasks_in_list(list_id)?)
    }

    pub fn all_tasks(&self) -> Result<Vec<Task>> {
        Ok(self.context.store.tasks()?)
    }

    // ─── Writes ───────────────────────────────────────────────────────────────────────────────

    /// Create a task. It exists — in the cache, with a temporary id — before this returns.
    ///
    /// The temporary id doubles as the idempotency key, so a create that times out and is retried
    /// cannot produce two tasks.
    pub fn create(&self, draft: &TaskDraft) -> Result<Task> {
        let now = self.context.clock.now();
        let temp_id = outbox::new_temp_id();

        // A virtual list ("Today", "Not in a List") is a saved set of filters, and a board column
        // is a state. Neither is somewhere a task can be filed, and filing one there produces a
        // task that belongs to a view — invisible in every real list, and impossible to find
        // again. Dropping them here rather than at the call site means every path that creates a
        // task is covered, including the one where the open list IS a virtual list.
        let list_ids = self.filed_in(&draft.list_ids)?;

        let mut task = Task::new(temp_id.clone(), draft.title.clone());
        task.description = draft.description.clone();
        task.list_ids = Some(list_ids.clone());
        task.priority = draft.priority;
        task.due_date_time = draft.due_date_time;
        task.is_all_day = draft.is_all_day;
        task.assignee_id = draft.assignee_id.clone();
        task.repeating = draft.repeating;
        task.repeating_data = draft.repeating_data.clone();
        task.repeat_from = draft.repeat_from;
        task.is_private = draft.is_private;
        task.parent_task_id = draft.parent_task_id.clone();
        task.status_role = draft.status_role.clone();
        task.created_at = Some(now);
        task.updated_at = Some(now);
        task.client_request_id = Some(temp_id.clone());

        self.context.store.upsert_task(&task)?;

        let body = json!({
            "title": draft.title,
            "description": draft.description,
            "listIds": list_ids,
            "priority": draft.priority.as_i64(),
            "dueDateTime": draft.due_date_time.map(date::format),
            "isAllDay": draft.is_all_day,
            "assigneeId": draft.assignee_id,
            "repeating": draft.repeating,
            "repeatingData": draft.repeating_data,
            "repeatFrom": draft.repeat_from,
            "isPrivate": draft.is_private,
            "parentTaskId": draft.parent_task_id,
            "statusRole": draft.status_role,
        });
        let entry = outbox::build(kind::CREATE_TASK, json!({ "body": body }), &temp_id, now)
            .for_temp_id(&temp_id);
        journal::enqueue(&self.context.store, &entry)?;

        Ok(task)
    }

    /// Edit a task. See [`TaskChanges`] for how "clear it" is told apart from "leave it".
    ///
    /// **Not the way to complete one.** `completed: Some(true)` here writes the flag and skips the
    /// rollover; [`TaskService::complete`] is the only correct path.
    pub fn update(&self, id: &str, changes: &TaskChanges) -> Result<Task> {
        let now = self.context.clock.now();
        let mut task = self.require(id)?;
        changes.apply(&mut task);
        task.updated_at = Some(now);
        self.context.store.upsert_task(&task)?;

        let entry = outbox::build(
            kind::UPDATE_TASK,
            json!({ "taskId": id, "body": changes.to_body() }),
            &outbox::new_temp_id(),
            now,
        );
        let entry = match crate::model::is_temp_id(id) {
            // An edit to a task the server has not seen yet belongs in the create's lane, and has
            // to strand with it if the create is refused.
            true => entry.for_temp_id(id),
            false => entry,
        };
        journal::enqueue(&self.context.store, &entry)?;

        Ok(task)
    }

    /// Complete (or un-complete) a task — the only correct way to do either.
    ///
    /// `on_screen` is the task as the user is looking at it, when a view let them edit fields
    /// before completing. The due date, all-day flag, repeat settings and repeat-from are taken
    /// from it, because those are what the next occurrence is computed from and the cache may be a
    /// sync behind (rule 3 of `docs/ASTRID.md` §0).
    ///
    /// A repeating task that is completed does not become completed: it rolls forward to its next
    /// occurrence and stays open. A series that has reached its end condition stays completed with
    /// its repeat cleared, so it does not look like it will come back.
    pub fn complete(
        &self,
        id: &str,
        completed: bool,
        on_screen: Option<&Task>,
        timer: Option<TimerResult>,
    ) -> Result<Task> {
        let now = self.context.clock.now();
        let mut current = self.require(id)?;
        if let Some(edited) = on_screen {
            // Only the fields the rollover is computed from. Copying the whole task would let a
            // stale view overwrite everything else about it.
            current.due_date_time = edited.due_date_time;
            current.is_all_day = edited.is_all_day;
            current.repeating = edited.repeating;
            current.repeating_data = edited.repeating_data.clone();
            current.repeat_from = edited.repeat_from;
            current.occurrence_count = edited.occurrence_count;
        }

        let mut changes = TaskChanges::default();
        if let Some(timer) = &timer {
            changes.timer_duration = Some(timer.duration_seconds);
            changes.last_timer_value = Some(timer.last_value.clone());
        }

        // Un-completing, or a task that does not repeat: the flag is the whole operation.
        if !completed || current.completed || !current.is_repeating() {
            changes.completed = Some(completed);
            changes.completed_at = Some(completed.then_some(now));
            return self.update(id, &changes);
        }

        let outcome = next_occurrence(&current, now, self.context.clock.utc_offset());
        match outcome.next_due_date {
            Some(next_due) => {
                // It rolls forward: still open, due next time round.
                changes.completed = Some(false);
                changes.completed_at = Some(None);
                changes.due_date_time = Some(Some(next_due));
                changes.is_all_day = Some(current.is_all_day);
                changes.occurrence_count = Some(outcome.new_occurrence_count as i64);
                self.update(id, &changes)
            }
            None => {
                // The series is over. Completed, and no longer repeating — a task that showed a
                // repeat chip after its last occurrence would look like it was coming back.
                changes.completed = Some(true);
                changes.completed_at = Some(Some(now));
                changes.repeating = Some(Some(Repeating::Never));
                changes.repeating_data = Some(None);
                self.update(id, &changes)
            }
        }
    }

    /// Delete a task. Gone from the cache immediately; the server hears about it when it can.
    pub fn delete(&self, id: &str) -> Result<()> {
        let now = self.context.clock.now();
        self.context.store.delete_task(id)?;

        let entry = outbox::build(
            kind::DELETE_TASK,
            json!({ "taskId": id }),
            &outbox::new_temp_id(),
            now,
        );
        let entry = match crate::model::is_temp_id(id) {
            true => entry.for_temp_id(id),
            false => entry,
        };
        journal::enqueue(&self.context.store, &entry)?;
        Ok(())
    }

    /// Move a task between lists.
    pub fn set_lists(&self, id: &str, list_ids: Vec<String>) -> Result<Task> {
        self.update(
            id,
            &TaskChanges {
                list_ids: Some(list_ids),
                ..Default::default()
            },
        )
    }

    /// Set the board column a task sits in. `None` is the Inbox column, which is a state rather
    /// than a stored list.
    pub fn set_status_role(&self, id: &str, status_role: Option<String>) -> Result<Task> {
        self.update(
            id,
            &TaskChanges {
                status_role: Some(status_role),
                ..Default::default()
            },
        )
    }

    /// The subset of `list_ids` a task can actually be filed in.
    ///
    /// An id the cache has never heard of is kept: it may be a list this device has not synced
    /// yet, and dropping it would silently lose the filing. Only lists we know to be views or
    /// states are removed.
    fn filed_in(&self, list_ids: &[String]) -> Result<Vec<String>> {
        let mut kept = Vec::with_capacity(list_ids.len());
        for id in list_ids {
            match self.context.store.list(id)? {
                Some(list) if list.is_virtual.unwrap_or(false) || list.is_status_list() => {}
                _ => kept.push(id.clone()),
            }
        }
        Ok(kept)
    }

    fn require(&self, id: &str) -> Result<Task> {
        self.context
            .store
            .task(id)?
            .ok_or_else(|| ServiceError::NotFound {
                kind: "task",
                id: id.to_string(),
            })
    }
}

/// What a timer produced, when a task was completed from one.
#[derive(Debug, Clone)]
pub struct TimerResult {
    pub duration_seconds: Option<i64>,
    pub last_value: Option<String>,
}

/// Work out where a repeating task goes next.
///
/// Delegates to [`crate::repeating`] and does no pattern math of its own. An inline copy in the
/// iOS service once ignored `weekdays`, so a Mon/Wed/Fri task jumped a whole week instead of
/// moving to the next selected day — rule 4 of `docs/ASTRID.md` §0 exists because of it.
fn next_occurrence(
    task: &Task,
    now: DateTime<Utc>,
    offset: chrono::FixedOffset,
) -> repeating::NextOccurrence {
    let repeat_from = match task.repeat_from.unwrap_or(RepeatFromMode::CompletionDate) {
        RepeatFromMode::DueDate => repeating::RepeatFrom::DueDate,
        RepeatFromMode::CompletionDate => repeating::RepeatFrom::CompletionDate,
    };
    let completion = effective_completion_date(task, repeat_from, now, offset);
    let occurrence_count = task.occurrence_count.unwrap_or(0) as i32;

    if task.repeating == Some(Repeating::Custom) {
        if let Some(pattern) = &task.repeating_data {
            return repeating::calculate_custom_next_occurrence(
                &repeating::pattern_from_wire(pattern),
                task.due_date_time,
                completion,
                repeat_from,
                occurrence_count,
            );
        }
    }

    // A simple pattern may still carry an end condition, piggybacked on the same column.
    let end_data = task.repeating_data.as_ref().and_then(|pattern| {
        let wire = repeating::pattern_from_wire(pattern);
        wire.end_condition
            .map(|end_condition| repeating::SimplePatternEndCondition {
                end_condition,
                end_after_occurrences: wire.end_after_occurrences,
                end_until_date: wire.end_until_date,
            })
    });

    let simple = match task.repeating {
        Some(Repeating::Daily) => repeating::Repeating::Daily,
        Some(Repeating::Weekly) => repeating::Repeating::Weekly,
        Some(Repeating::Monthly) => repeating::Repeating::Monthly,
        Some(Repeating::Yearly) => repeating::Repeating::Yearly,
        _ => repeating::Repeating::Never,
    };
    repeating::calculate_simple_next_occurrence(
        simple,
        task.due_date_time,
        completion,
        repeat_from,
        occurrence_count,
        end_data.as_ref(),
    )
}

/// The instant a completion anchors on.
///
/// For an all-day task repeating from its completion, the anchor is the calendar day the person
/// completed it on — **their** day, stored the way all-day dates are stored. Ticking one off at
/// 21:00 in California is 04:00 UTC the next day; anchoring on that instant would move every
/// evening completion a day past their own calendar, and the next occurrence with it.
fn effective_completion_date(
    task: &Task,
    repeat_from: repeating::RepeatFrom,
    now: DateTime<Utc>,
    offset: chrono::FixedOffset,
) -> DateTime<Utc> {
    if task.is_all_day && repeat_from == repeating::RepeatFrom::CompletionDate {
        return date::all_day_today(now, offset);
    }
    now
}

/// The lists a task is in, resolved against the cache. Used by the row projection and by anything
/// that renders list chips.
pub fn lists_for(task: &Task, all_lists: &[TaskList]) -> Vec<TaskList> {
    let ids = task.effective_list_ids();
    all_lists
        .iter()
        .filter(|list| ids.contains(&list.id))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport};
    use crate::model::CustomRepeatingPattern;
    use crate::outbox::Status;
    use crate::platform::{FixedClock, MemorySecureStore};
    use crate::store::Store;
    use std::sync::Arc;

    fn at(instant: &str) -> DateTime<Utc> {
        date::parse(instant).expect("an instant")
    }

    struct Fixture {
        service: TaskService,
        store: Arc<Store>,
    }

    fn fixture(now: &str) -> Fixture {
        fixture_in_zone(now, 0)
    }

    /// The same, somewhere other than UTC. Several rules here are about the reader's calendar day
    /// rather than the instant, and a test that never leaves UTC cannot tell the two apart.
    fn fixture_in_zone(now: &str, offset_hours: i32) -> Fixture {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                Arc::new(StubTransport::new()),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            Arc::new(FixedClock::at(at(now)).in_zone(offset_hours)),
        );
        Fixture {
            service: context.tasks(),
            store,
        }
    }

    fn entries(store: &Store) -> Vec<crate::outbox::Entry> {
        journal::all(store).expect("reads")
    }

    // ── Creating ─────────────────────────────────────────────────────────────────────────────

    /// The offline story in one test: it exists before anything is sent, and what will be sent is
    /// already written down.
    #[test]
    fn a_created_task_is_in_the_cache_and_in_the_journal_before_anything_is_sent() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let created = fixture
            .service
            .create(&TaskDraft::new("Buy milk").in_list("l1"))
            .expect("creates");

        assert!(crate::model::is_temp_id(&created.id));
        assert_eq!(
            fixture.store.tasks_in_list("l1").expect("reads").len(),
            1,
            "it has to be visible in its list immediately"
        );

        let entries = entries(&fixture.store);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, kind::CREATE_TASK);
        assert_eq!(entries[0].temp_id.as_deref(), Some(created.id.as_str()));
        assert_eq!(entries[0].payload["body"]["title"], "Buy milk");
    }

    /// The temp id IS the idempotency key. A create that times out and is retried under a
    /// different key makes a second task, and the user deletes one of them wondering what happened.
    #[test]
    fn the_temporary_id_is_the_idempotency_key() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let created = fixture
            .service
            .create(&TaskDraft::new("Buy milk"))
            .expect("creates");
        assert_eq!(entries(&fixture.store)[0].client_request_id, created.id);
        assert_eq!(
            created.client_request_id.as_deref(),
            Some(created.id.as_str())
        );
    }

    // ── Editing ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn an_edit_shows_immediately_and_sends_only_what_changed() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        fixture
            .store
            .upsert_task(&Task::new("t1", "Buy milk"))
            .expect("stores");

        let updated = fixture
            .service
            .update("t1", &TaskChanges::title("Buy oat milk"))
            .expect("updates");
        assert_eq!(updated.title, "Buy oat milk");
        assert_eq!(
            fixture
                .store
                .task("t1")
                .expect("reads")
                .expect("present")
                .title,
            "Buy oat milk"
        );

        let body = &entries(&fixture.store)[0].payload["body"];
        assert_eq!(body["title"], "Buy oat milk");
        assert_eq!(
            body.as_object().expect("an object").len(),
            1,
            "an edit sends what was edited, not the whole task: {body}"
        );
    }

    /// "Remove the due date" and "leave the due date alone" are both ordinary operations. A body
    /// that cannot say `null` can only express one of them.
    #[test]
    fn clearing_a_field_is_told_apart_from_leaving_it_alone() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut task = Task::new("t1", "Buy milk");
        task.due_date_time = Some(at("2026-09-08T12:00:00Z"));
        fixture.store.upsert_task(&task).expect("stores");

        // Leave it alone.
        fixture
            .service
            .update("t1", &TaskChanges::title("x"))
            .expect("updates");
        assert!(entries(&fixture.store)[0].payload["body"]
            .get("dueDateTime")
            .is_none());
        assert!(fixture
            .store
            .task("t1")
            .expect("reads")
            .expect("present")
            .due_date_time
            .is_some());

        // Clear it.
        fixture
            .service
            .update(
                "t1",
                &TaskChanges {
                    due_date_time: Some(None),
                    ..Default::default()
                },
            )
            .expect("updates");
        assert!(entries(&fixture.store)[1].payload["body"]["dueDateTime"].is_null());
        assert!(fixture
            .store
            .task("t1")
            .expect("reads")
            .expect("present")
            .due_date_time
            .is_none());
    }

    /// An edit to a task the server has never seen has to travel in the create's lane and strand
    /// with it, or it becomes an edit to an id that does not exist.
    #[test]
    fn an_edit_to_a_not_yet_created_task_carries_its_temporary_id() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let created = fixture
            .service
            .create(&TaskDraft::new("Buy milk"))
            .expect("creates");
        fixture
            .service
            .update(&created.id, &TaskChanges::title("Buy oat milk"))
            .expect("updates");

        let entries = entries(&fixture.store);
        assert_eq!(entries[1].kind, kind::UPDATE_TASK);
        assert_eq!(entries[1].temp_id.as_deref(), Some(created.id.as_str()));
        assert_eq!(
            entries[0].serialization_key(),
            entries[1].serialization_key(),
            "the create and the edit must not be in flight at once"
        );
    }

    // ── Completing ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn completing_a_one_off_task_marks_it_done_with_the_time_it_happened() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        fixture
            .store
            .upsert_task(&Task::new("t1", "Buy milk"))
            .expect("stores");

        let done = fixture
            .service
            .complete("t1", true, None, None)
            .expect("completes");
        assert!(done.completed);
        assert_eq!(done.completed_at, Some(at("2026-09-07T12:00:00Z")));
    }

    #[test]
    fn un_completing_clears_the_completion_time() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut task = Task::new("t1", "Buy milk");
        task.completed = true;
        task.completed_at = Some(at("2026-09-06T12:00:00Z"));
        fixture.store.upsert_task(&task).expect("stores");

        let reopened = fixture
            .service
            .complete("t1", false, None, None)
            .expect("completes");
        assert!(!reopened.completed);
        assert_eq!(reopened.completed_at, None);
    }

    /// The rule this file exists for. A daily task that is completed is not done — it is due
    /// tomorrow.
    #[test]
    fn completing_a_repeating_task_rolls_it_forward_instead_of_finishing_it() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut task = Task::new("t1", "Water the plants");
        task.repeating = Some(Repeating::Daily);
        task.repeat_from = Some(RepeatFromMode::DueDate);
        task.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        fixture.store.upsert_task(&task).expect("stores");

        let rolled = fixture
            .service
            .complete("t1", true, None, None)
            .expect("completes");
        assert!(
            !rolled.completed,
            "a repeating task does not stay completed"
        );
        assert_eq!(rolled.due_date_time, Some(at("2026-09-08T12:00:00Z")));
        assert_eq!(rolled.occurrence_count, Some(1));
    }

    /// Rule 3: the rollover anchors on what the user is looking at. The cache says the 7th; the
    /// screen says the 10th because they just changed it; tomorrow is the 11th, not the 8th.
    #[test]
    fn a_completion_from_an_edited_view_anchors_on_what_is_on_screen() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut cached = Task::new("t1", "Water the plants");
        cached.repeating = Some(Repeating::Daily);
        cached.repeat_from = Some(RepeatFromMode::DueDate);
        cached.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        fixture.store.upsert_task(&cached).expect("stores");

        let mut on_screen = cached.clone();
        on_screen.due_date_time = Some(at("2026-09-10T12:00:00Z"));

        let rolled = fixture
            .service
            .complete("t1", true, Some(&on_screen), None)
            .expect("completes");
        assert_eq!(rolled.due_date_time, Some(at("2026-09-11T12:00:00Z")));
    }

    /// A weekly Mon/Wed/Fri task moves to the next selected day, not seven days on. The inline
    /// copy that got this wrong is why the math lives in one module.
    #[test]
    fn a_custom_weekly_pattern_picks_the_next_selected_day() {
        // 2026-09-07 is a Monday.
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut task = Task::new("t1", "Gym");
        task.repeating = Some(Repeating::Custom);
        task.repeat_from = Some(RepeatFromMode::DueDate);
        task.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        task.repeating_data = Some(CustomRepeatingPattern {
            r#type: Some("custom".into()),
            unit: Some("weeks".into()),
            interval: Some(1),
            weekdays: Some(vec!["monday".into(), "wednesday".into(), "friday".into()]),
            ..Default::default()
        });
        fixture.store.upsert_task(&task).expect("stores");

        let rolled = fixture
            .service
            .complete("t1", true, None, None)
            .expect("completes");
        assert_eq!(
            rolled.due_date_time,
            Some(at("2026-09-09T12:00:00Z")),
            "Wednesday, not next Monday"
        );
    }

    /// The last occurrence of a series stays completed AND stops looking like it repeats.
    #[test]
    fn the_end_of_a_series_stays_completed_with_its_repeat_cleared() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut task = Task::new("t1", "Take the course");
        task.repeating = Some(Repeating::Daily);
        task.repeat_from = Some(RepeatFromMode::DueDate);
        task.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        task.occurrence_count = Some(2);
        task.repeating_data = Some(CustomRepeatingPattern {
            end_condition: Some("after_occurrences".into()),
            end_after_occurrences: Some(3),
            ..Default::default()
        });
        fixture.store.upsert_task(&task).expect("stores");

        let finished = fixture
            .service
            .complete("t1", true, None, None)
            .expect("completes");
        assert!(finished.completed);
        assert_eq!(finished.repeating, Some(Repeating::Never));
        assert_eq!(finished.repeating_data, None);
    }

    /// Completing an all-day task at 21:00 local is 05:00 UTC the next day. Anchoring on that
    /// moves every evening completion a day past the person's own calendar.
    #[test]
    fn an_all_day_completion_anchors_on_the_day_not_the_moment() {
        // 22:00 on the 7th in California, which is 05:00 on the 8th in UTC.
        let fixture = fixture_in_zone("2026-09-08T05:00:00Z", -7);
        let mut task = Task::new("t1", "Water the plants");
        task.repeating = Some(Repeating::Daily);
        task.repeat_from = Some(RepeatFromMode::CompletionDate);
        task.is_all_day = true;
        task.due_date_time = Some(at("2026-09-07T00:00:00Z"));
        fixture.store.upsert_task(&task).expect("stores");

        let rolled = fixture
            .service
            .complete("t1", true, None, None)
            .expect("completes");
        assert_eq!(
            rolled.due_date_time,
            Some(at("2026-09-08T00:00:00Z")),
            "completed on the 7th where the person is, so it is next due on the 8th — anchoring \
             on the UTC instant would have said the 9th, skipping a day every evening"
        );
    }

    /// Completing something that is already completed must not roll it forward again — a double
    /// tap on a checkbox would otherwise skip an occurrence.
    #[test]
    fn completing_an_already_completed_repeating_task_does_not_roll_it_again() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut task = Task::new("t1", "Water the plants");
        task.repeating = Some(Repeating::Daily);
        task.completed = true;
        task.due_date_time = Some(at("2026-09-07T12:00:00Z"));
        fixture.store.upsert_task(&task).expect("stores");

        let again = fixture
            .service
            .complete("t1", true, None, None)
            .expect("completes");
        assert_eq!(again.due_date_time, Some(at("2026-09-07T12:00:00Z")));
        assert!(again.completed);
    }

    #[test]
    fn a_timer_result_travels_with_the_completion() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        fixture
            .store
            .upsert_task(&Task::new("t1", "Focus"))
            .expect("stores");

        let done = fixture
            .service
            .complete(
                "t1",
                true,
                None,
                Some(TimerResult {
                    duration_seconds: Some(1500),
                    last_value: Some("25:00".into()),
                }),
            )
            .expect("completes");
        assert_eq!(done.timer_duration, Some(1500));
        assert_eq!(done.last_timer_value.as_deref(), Some("25:00"));
    }

    // ── Deleting ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_deleted_task_is_gone_at_once_and_the_delete_is_journalled() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        fixture
            .store
            .upsert_task(&Task::new("t1", "Buy milk"))
            .expect("stores");

        fixture.service.delete("t1").expect("deletes");
        assert!(fixture.store.task("t1").expect("reads").is_none());

        let entries = entries(&fixture.store);
        assert_eq!(entries[0].kind, kind::DELETE_TASK);
        assert_eq!(entries[0].payload["taskId"], "t1");
        assert_eq!(entries[0].status, Status::Pending);
    }

    /// A task filed into a view belongs nowhere: it is invisible in every real list and there is
    /// no way to find it again.
    #[test]
    fn a_task_is_not_filed_into_a_virtual_list_or_a_board_column() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let virtual_list: TaskList =
            serde_json::from_str(r#"{"id":"v1","name":"Today","isVirtual":true}"#)
                .expect("decodes");
        let column: TaskList =
            serde_json::from_str(r#"{"id":"s1","name":"Doing","listType":"status"}"#)
                .expect("decodes");
        fixture
            .store
            .upsert_lists(&[TaskList::new("l1", "Home"), virtual_list, column])
            .expect("stores");

        let created = fixture
            .service
            .create(
                &TaskDraft::new("Buy milk")
                    .in_list("l1")
                    .in_list("v1")
                    .in_list("s1"),
            )
            .expect("creates");

        assert_eq!(created.effective_list_ids(), vec!["l1"]);
        assert_eq!(
            entries(&fixture.store)[0].payload["body"]["listIds"],
            serde_json::json!(["l1"])
        );
    }

    /// A list this device has not synced yet is not a view — dropping it would silently lose the
    /// filing on the one device that had not caught up.
    #[test]
    fn a_list_the_cache_has_never_heard_of_is_kept() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let created = fixture
            .service
            .create(&TaskDraft::new("Buy milk").in_list("not-synced-yet"))
            .expect("creates");
        assert_eq!(created.effective_list_ids(), vec!["not-synced-yet"]);
    }

    #[test]
    fn editing_something_that_is_not_there_says_so() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let error = fixture
            .service
            .update("nope", &TaskChanges::title("x"))
            .expect_err("no such task");
        assert!(matches!(error, ServiceError::NotFound { kind: "task", .. }));
    }

    #[test]
    fn moving_a_task_between_lists_replaces_its_membership() {
        let fixture = fixture("2026-09-07T12:00:00Z");
        let mut task = Task::new("t1", "Buy milk");
        task.list_ids = Some(vec!["l1".into()]);
        fixture.store.upsert_task(&task).expect("stores");

        fixture
            .service
            .set_lists("t1", vec!["l2".into()])
            .expect("moves");
        assert!(fixture.store.tasks_in_list("l1").expect("reads").is_empty());
        assert_eq!(fixture.store.tasks_in_list("l2").expect("reads").len(), 1);
    }
}
