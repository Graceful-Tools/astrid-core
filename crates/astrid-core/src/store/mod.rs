//! The offline cache: everything the app knows, on disk, readable without a network.
//!
//! This is the read path. It never waits on a request, and it is authoritative for what the UI
//! draws — sync writes into it, the Outbox writes into it optimistically, and the shell renders
//! whatever it holds. That ordering is what makes the app work on a train, and it is the reason
//! rule 5 of `docs/ASTRID.md` §0 exists.
//!
//! Ported in spirit rather than in shape from `astrid-ios/Astrid App/Core/Persistence/`, which is
//! Core Data. The behaviour that matters — local-first reads, upsert-by-id, membership as a real
//! relation, temporary ids that survive until the server's arrives — is the same. See
//! [`schema`] for why each row keeps its original JSON beside the columns lifted out of it.
//!
//! ## Threading
//!
//! One connection behind a mutex, and every method is synchronous. SQLite calls here are
//! sub-millisecond; a pool would buy contention and an async API would buy await points, and
//! neither buys speed. The mutex is never held across anything that can block.

pub mod schema;

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::model::date;
use crate::model::{ChatChannel, ChatMessage, Comment, Priority, Project, Task, TaskList, User};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the cache could not be opened or read: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("a cached row could not be read back: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// What the list view needs, and nothing else.
///
/// Read straight from indexed columns, so drawing a list never decodes JSON. Anything a detail
/// view needs comes from [`Store::task`], which does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub due_date_time: Option<DateTime<Utc>>,
    pub is_all_day: bool,
    pub priority: Priority,
    pub repeating: Option<String>,
    pub parent_task_id: Option<String>,
    pub assignee_id: Option<String>,
    pub creator_id: Option<String>,
    pub status_role: Option<String>,
    pub is_private: bool,
    pub has_description: bool,
    pub timer_duration: Option<i64>,
    pub occurrence_count: Option<i64>,
    pub comment_count: i64,
    pub attachment_count: i64,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    /// Open (and migrate) the cache at `path`, creating it if it is not there.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        Self::prepare(connection)
    }

    /// A cache that lives only as long as the process. Used by every test, and by the shell's
    /// UI-test build, which must never touch the real user's data.
    pub fn in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut connection: Connection) -> Result<Self> {
        // WAL so a read never blocks behind a write: sync writes while the user is scrolling is
        // the normal case, not the exceptional one.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        // NORMAL rather than FULL. The cache is a cache: the Outbox is what makes a write durable,
        // and paying an fsync per statement to protect data the server already has is the wrong
        // trade on a laptop with a spinning disk.
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrate(&mut connection)?;
        Ok(Store {
            connection: Mutex::new(connection),
        })
    }

    /// Run `work` inside a transaction. Used by sync, which must never leave half a snapshot
    /// visible to a list that is being drawn at the time.
    pub fn transaction<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.connection.lock().expect("cache lock");
        let transaction = guard.transaction()?;
        let value = work(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    fn with<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.connection.lock().expect("cache lock");
        work(&guard)
    }

    // ─── Tasks ────────────────────────────────────────────────────────────────────────────────

    /// Insert or replace a task, and rewrite its list membership.
    ///
    /// Membership is replaced wholesale rather than merged: the server's answer is the truth about
    /// which lists a task is in, and merging would make removing a task from a list impossible to
    /// sync — which is exactly the bug shape "it came back after I moved it" describes.
    pub fn upsert_task(&self, task: &Task) -> Result<()> {
        self.with(|connection| upsert_task_in(connection, task))
    }

    pub fn upsert_tasks(&self, tasks: &[Task]) -> Result<()> {
        self.transaction(|connection| {
            for task in tasks {
                upsert_task_in(connection, task)?;
            }
            Ok(())
        })
    }

    pub fn task(&self, id: &str) -> Result<Option<Task>> {
        self.with(|connection| {
            let json: Option<String> = connection
                .query_row("SELECT json FROM tasks WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            json.map(|json| decode(&json)).transpose()
        })
    }

    pub fn tasks(&self) -> Result<Vec<Task>> {
        self.with(|connection| {
            let mut statement = connection.prepare("SELECT json FROM tasks")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn tasks_in_list(&self, list_id: &str) -> Result<Vec<Task>> {
        self.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT t.json FROM tasks t
                 JOIN task_lists_membership m ON m.task_id = t.id
                 WHERE m.list_id = ?1",
            )?;
            let rows = statement.query_map([list_id], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// The list view's read: columns only, no JSON.
    pub fn summaries_in_list(&self, list_id: &str) -> Result<Vec<TaskSummary>> {
        self.with(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {SUMMARY_COLUMNS} FROM tasks t
                 JOIN task_lists_membership m ON m.task_id = t.id
                 WHERE m.list_id = ?1"
            ))?;
            let rows = statement.query_map([list_id], read_summary)?;
            Ok(rows.collect::<SqlResult<Vec<_>>>()?)
        })
    }

    pub fn summaries(&self) -> Result<Vec<TaskSummary>> {
        self.with(|connection| {
            let mut statement =
                connection.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM tasks t"))?;
            let rows = statement.query_map([], read_summary)?;
            Ok(rows.collect::<SqlResult<Vec<_>>>()?)
        })
    }

    /// The ids of the lists a task belongs to.
    pub fn list_ids_for_task(&self, task_id: &str) -> Result<Vec<String>> {
        self.with(|connection| {
            let mut statement = connection
                .prepare("SELECT list_id FROM task_lists_membership WHERE task_id = ?1")?;
            let rows = statement.query_map([task_id], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<SqlResult<Vec<_>>>()?)
        })
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        self.with(|connection| {
            connection.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
            connection.execute("DELETE FROM task_lists_membership WHERE task_id = ?1", [id])?;
            connection.execute("DELETE FROM comments WHERE task_id = ?1", [id])?;
            Ok(())
        })
    }

    // ─── Lists ────────────────────────────────────────────────────────────────────────────────

    pub fn upsert_list(&self, list: &TaskList) -> Result<()> {
        self.with(|connection| upsert_list_in(connection, list))
    }

    pub fn upsert_lists(&self, lists: &[TaskList]) -> Result<()> {
        self.transaction(|connection| {
            for list in lists {
                upsert_list_in(connection, list)?;
            }
            Ok(())
        })
    }

    pub fn list(&self, id: &str) -> Result<Option<TaskList>> {
        self.with(|connection| {
            let json: Option<String> = connection
                .query_row("SELECT json FROM lists WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            json.map(|json| decode(&json)).transpose()
        })
    }

    pub fn lists(&self) -> Result<Vec<TaskList>> {
        self.with(|connection| {
            let mut statement = connection.prepare("SELECT json FROM lists")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn delete_list(&self, id: &str) -> Result<()> {
        self.with(|connection| {
            connection.execute("DELETE FROM lists WHERE id = ?1", [id])?;
            connection.execute("DELETE FROM task_lists_membership WHERE list_id = ?1", [id])?;
            Ok(())
        })
    }

    // ─── Everything else the cache holds ──────────────────────────────────────────────────────

    pub fn upsert_projects(&self, projects: &[Project]) -> Result<()> {
        self.transaction(|connection| {
            for project in projects {
                connection.execute(
                    "INSERT OR REPLACE INTO projects (id, name, owner_id, created_at, updated_at, json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        project.id,
                        project.name,
                        project.owner_id,
                        project.created_at.map(date::format),
                        project.updated_at.map(date::format),
                        encode(project)?,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.with(|connection| {
            let mut statement = connection.prepare("SELECT json FROM projects")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn upsert_users(&self, users: &[User]) -> Result<()> {
        self.transaction(|connection| {
            for user in users {
                connection.execute(
                    "INSERT OR REPLACE INTO users (id, name, email, json) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![user.id, user.name, user.email, encode(user)?],
                )?;
            }
            Ok(())
        })
    }

    pub fn user(&self, id: &str) -> Result<Option<User>> {
        self.with(|connection| {
            let json: Option<String> = connection
                .query_row("SELECT json FROM users WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            json.map(|json| decode(&json)).transpose()
        })
    }

    pub fn upsert_comments(&self, comments: &[Comment]) -> Result<()> {
        self.transaction(|connection| {
            for comment in comments {
                connection.execute(
                    "INSERT OR REPLACE INTO comments (id, task_id, author_id, created_at, json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        comment.id,
                        comment.task_id,
                        comment.author_id,
                        comment.created_at.map(date::format),
                        encode(comment)?,
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// A task's comments, oldest first — the order they are read in.
    pub fn comments_for_task(&self, task_id: &str) -> Result<Vec<Comment>> {
        self.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT json FROM comments WHERE task_id = ?1 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = statement.query_map([task_id], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn comment(&self, id: &str) -> Result<Option<Comment>> {
        self.with(|connection| {
            let json: Option<String> = connection
                .query_row("SELECT json FROM comments WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            json.map(|json| decode(&json)).transpose()
        })
    }

    pub fn delete_comment(&self, id: &str) -> Result<()> {
        self.with(|connection| {
            connection.execute("DELETE FROM comments WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    pub fn upsert_channels(&self, channels: &[ChatChannel]) -> Result<()> {
        self.transaction(|connection| {
            for channel in channels {
                connection.execute(
                    "INSERT OR REPLACE INTO chat_channels (id, list_id, virtual_key, json)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        channel.id,
                        channel.list_id,
                        channel.virtual_key,
                        encode(channel)?
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn channels(&self) -> Result<Vec<ChatChannel>> {
        self.with(|connection| {
            let mut statement = connection.prepare("SELECT json FROM chat_channels")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn upsert_messages(&self, messages: &[ChatMessage]) -> Result<()> {
        self.transaction(|connection| {
            for message in messages {
                connection.execute(
                    "INSERT OR REPLACE INTO chat_messages (id, channel_id, author_id, created_at, json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        message.id,
                        message.channel_id,
                        message.author_id,
                        message.created_at.map(date::format),
                        encode(message)?,
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// A channel's messages, oldest first.
    pub fn messages_in_channel(&self, channel_id: &str) -> Result<Vec<ChatMessage>> {
        self.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT json FROM chat_messages WHERE channel_id = ?1
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = statement.query_map([channel_id], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn delete_message(&self, id: &str) -> Result<()> {
        self.with(|connection| {
            connection.execute("DELETE FROM chat_messages WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    // ─── Temporary ids ────────────────────────────────────────────────────────────────────────

    /// Record that a `temp_` id this device minted is now known to the server as `server_id`.
    ///
    /// Kept after the Outbox entry that created it is gone: a deep link, a notification, or a
    /// queued edit written before the create was delivered can still be holding the temporary one,
    /// and resolving it late is the difference between an edit landing and an edit 404ing.
    pub fn record_id_mapping(
        &self,
        temp_id: &str,
        server_id: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.with(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO id_mappings (temp_id, server_id, created_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![temp_id, server_id, date::format(now)],
            )?;
            Ok(())
        })
    }

    /// The server id for a temporary one, if it has been delivered. Follows a chain, because a
    /// temporary id can in principle be remapped twice; a cycle stops rather than hangs.
    pub fn resolve_id(&self, id: &str) -> Result<String> {
        self.with(|connection| {
            let mut current = id.to_string();
            for _ in 0..8 {
                let next: Option<String> = connection
                    .query_row(
                        "SELECT server_id FROM id_mappings WHERE temp_id = ?1",
                        [&current],
                        |row| row.get(0),
                    )
                    .optional()?;
                match next {
                    Some(next) if next != current => current = next,
                    _ => break,
                }
            }
            Ok(current)
        })
    }

    // ─── Metadata ─────────────────────────────────────────────────────────────────────────────

    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.with(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
                [key, value],
            )?;
            Ok(())
        })
    }

    pub fn metadata(&self, key: &str) -> Result<Option<String>> {
        self.with(|connection| {
            Ok(connection
                .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .optional()?)
        })
    }

    /// Forget everything. Sign-out calls this: a cache that outlives its session is a cache that
    /// shows the previous user's tasks to the next one.
    pub fn clear(&self) -> Result<()> {
        self.transaction(|connection| {
            for table in [
                "tasks",
                "task_lists_membership",
                "lists",
                "projects",
                "users",
                "comments",
                "chat_channels",
                "chat_messages",
                "outbox",
                "id_mappings",
                "metadata",
            ] {
                connection.execute(&format!("DELETE FROM {table}"), [])?;
            }
            Ok(())
        })
    }
}

const SUMMARY_COLUMNS: &str = "t.id, t.title, t.completed, t.completed_at, t.due_date_time, \
     t.is_all_day, t.priority, t.repeating, t.parent_task_id, t.assignee_id, t.creator_id, \
     t.status_role, t.is_private, t.description, t.timer_duration, t.occurrence_count, \
     t.comment_count, t.attachment_count, t.created_at, t.updated_at";

fn read_summary(row: &rusqlite::Row<'_>) -> SqlResult<TaskSummary> {
    let instant = |value: Option<String>| value.as_deref().and_then(date::parse);
    Ok(TaskSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        completed: row.get::<_, i64>(2)? != 0,
        completed_at: instant(row.get(3)?),
        due_date_time: instant(row.get(4)?),
        is_all_day: row.get::<_, i64>(5)? != 0,
        priority: Priority::from_i64(row.get(6)?),
        repeating: row.get(7)?,
        parent_task_id: row.get(8)?,
        assignee_id: row.get(9)?,
        creator_id: row.get(10)?,
        status_role: row.get(11)?,
        is_private: row.get::<_, i64>(12)? != 0,
        has_description: !row.get::<_, String>(13)?.trim().is_empty(),
        timer_duration: row.get(14)?,
        occurrence_count: row.get(15)?,
        comment_count: row.get(16)?,
        attachment_count: row.get(17)?,
        created_at: instant(row.get(18)?),
        updated_at: instant(row.get(19)?),
    })
}

fn upsert_task_in(connection: &Connection, task: &Task) -> Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO tasks (
            id, title, completed, completed_at, due_date_time, is_all_day, priority, repeating,
            parent_task_id, assignee_id, creator_id, status_role, is_private, description,
            timer_duration, occurrence_count, comment_count, attachment_count,
            created_at, updated_at, json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                   ?18, ?19, ?20, ?21)",
        rusqlite::params![
            task.id,
            task.title,
            task.completed as i64,
            task.completed_at.map(date::format),
            task.due_date_time.map(date::format),
            task.is_all_day as i64,
            task.priority.as_i64(),
            task.repeating
                .map(|r| serde_json::to_value(r).ok())
                .and_then(|v| v.and_then(|v| v.as_str().map(str::to_string))),
            task.parent_task_id,
            task.assignee_id
                .clone()
                .or_else(|| task.assignee.as_ref().map(|u| u.id.clone())),
            task.effective_creator_id(),
            task.status_role,
            task.is_private as i64,
            task.description,
            task.timer_duration,
            task.occurrence_count,
            task.comments.as_ref().map(Vec::len).unwrap_or(0) as i64,
            task.all_secure_files().len() as i64,
            task.created_at.map(date::format),
            task.updated_at.map(date::format),
            encode(task)?,
        ],
    )?;

    connection.execute(
        "DELETE FROM task_lists_membership WHERE task_id = ?1",
        [&task.id],
    )?;
    for list_id in task.effective_list_ids() {
        connection.execute(
            "INSERT OR IGNORE INTO task_lists_membership (task_id, list_id) VALUES (?1, ?2)",
            [&task.id, &list_id],
        )?;
    }
    Ok(())
}

fn upsert_list_in(connection: &Connection, list: &TaskList) -> Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO lists (
            id, name, color, privacy, owner_id, project_id, list_type, status_role, status_order,
            is_favorite, favorite_order, is_virtual, task_count, created_at, updated_at, json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        rusqlite::params![
            list.id,
            list.name,
            list.color,
            list.privacy
                .map(|p| serde_json::to_value(p).ok())
                .and_then(|v| v.and_then(|v| v.as_str().map(str::to_string))),
            list.owner_id
                .clone()
                .or_else(|| list.owner.as_ref().map(|u| u.id.clone())),
            list.project_id,
            list.list_type,
            list.status_role,
            list.status_order,
            list.is_favorite.unwrap_or(false) as i64,
            list.favorite_order,
            list.is_virtual.unwrap_or(false) as i64,
            list.task_count,
            list.created_at.map(date::format),
            list.updated_at.map(date::format),
            encode(list)?,
        ],
    )?;
    Ok(())
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| StoreError::Corrupt(error.to_string()))
}

fn decode<T: DeserializeOwned>(json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|error| StoreError::Corrupt(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Repeating;

    fn task_in_lists(id: &str, lists: &[&str]) -> Task {
        let mut task = Task::new(id, format!("Task {id}"));
        task.list_ids = Some(lists.iter().map(|l| l.to_string()).collect());
        task
    }

    /// The cache stores what this build understood, not the bytes that arrived. Stated as a test
    /// because the alternative — keeping unknown fields and sending them back — is a tempting
    /// change to make, and it would let a stale value this client never read be written back over
    /// a newer one.
    #[test]
    fn what_is_cached_is_what_this_build_decoded() {
        let store = Store::in_memory().expect("opens");
        let task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "title": "Buy milk",
            "somethingNewOnTheServer": { "nested": true }
        }))
        .expect("decodes");
        store.upsert_task(&task).expect("stores");

        let raw: String = store
            .with(|connection| {
                Ok(
                    connection.query_row("SELECT json FROM tasks WHERE id = 't1'", [], |row| {
                        row.get::<_, String>(0)
                    })?,
                )
            })
            .expect("reads");
        assert!(!raw.contains("somethingNewOnTheServer"));
        assert_eq!(
            store.task("t1").expect("reads").expect("present").title,
            "Buy milk"
        );
    }

    #[test]
    fn upserting_replaces_rather_than_duplicates() {
        let store = Store::in_memory().expect("opens");
        store
            .upsert_task(&Task::new("t1", "First"))
            .expect("stores");
        store
            .upsert_task(&Task::new("t1", "Second"))
            .expect("stores");
        assert_eq!(store.tasks().expect("reads").len(), 1);
        assert_eq!(
            store.task("t1").expect("reads").expect("present").title,
            "Second"
        );
    }

    /// "It came back after I moved it" is what merging membership instead of replacing it looks
    /// like from the outside.
    #[test]
    fn membership_is_replaced_wholesale_so_a_task_can_leave_a_list() {
        let store = Store::in_memory().expect("opens");
        store
            .upsert_task(&task_in_lists("t1", &["l1", "l2"]))
            .expect("stores");
        assert_eq!(store.tasks_in_list("l1").expect("reads").len(), 1);

        store
            .upsert_task(&task_in_lists("t1", &["l2"]))
            .expect("stores");
        assert!(store.tasks_in_list("l1").expect("reads").is_empty());
        assert_eq!(store.tasks_in_list("l2").expect("reads").len(), 1);
    }

    #[test]
    fn membership_comes_from_embedded_lists_when_that_is_the_shape_that_arrived() {
        let store = Store::in_memory().expect("opens");
        let task: Task =
            serde_json::from_str(r#"{"id":"t1","title":"x","lists":[{"id":"l9","name":"Work"}]}"#)
                .expect("decodes");
        store.upsert_task(&task).expect("stores");
        assert_eq!(store.tasks_in_list("l9").expect("reads").len(), 1);
        assert_eq!(store.list_ids_for_task("t1").expect("reads"), vec!["l9"]);
    }

    /// The list view's read must not decode JSON — these are the columns it draws from.
    #[test]
    fn a_summary_carries_what_a_row_draws() {
        let store = Store::in_memory().expect("opens");
        let mut task = task_in_lists("t1", &["l1"]);
        task.title = "Buy milk".into();
        task.description = "two litres".into();
        task.priority = Priority::High;
        task.repeating = Some(Repeating::Weekly);
        task.due_date_time = date::parse("2026-09-07T12:00:00Z");
        task.assignee_id = Some("u1".into());
        store.upsert_task(&task).expect("stores");

        let summaries = store.summaries_in_list("l1").expect("reads");
        assert_eq!(summaries.len(), 1);
        let row = &summaries[0];
        assert_eq!(row.title, "Buy milk");
        assert_eq!(row.priority, Priority::High);
        assert_eq!(row.repeating.as_deref(), Some("weekly"));
        assert!(row.has_description);
        assert!(row.is_all_day);
        assert_eq!(row.assignee_id.as_deref(), Some("u1"));
        assert_eq!(
            row.due_date_time.map(date::format).as_deref(),
            Some("2026-09-07T12:00:00Z")
        );
    }

    /// The assignee can arrive as an id or as an embedded user. A row that reads only the id shows
    /// an unassigned task that is in fact assigned.
    #[test]
    fn an_embedded_assignee_still_fills_the_column() {
        let store = Store::in_memory().expect("opens");
        let task: Task = serde_json::from_str(r#"{"id":"t1","assignee":{"id":"u7","name":"Ada"}}"#)
            .expect("decodes");
        store.upsert_task(&task).expect("stores");
        assert_eq!(
            store.summaries().expect("reads")[0].assignee_id.as_deref(),
            Some("u7")
        );
    }

    #[test]
    fn deleting_a_task_takes_its_membership_and_comments_with_it() {
        let store = Store::in_memory().expect("opens");
        store
            .upsert_task(&task_in_lists("t1", &["l1"]))
            .expect("stores");
        let comment: Comment =
            serde_json::from_str(r#"{"id":"c1","taskId":"t1","content":"hi"}"#).expect("decodes");
        store.upsert_comments(&[comment]).expect("stores");

        store.delete_task("t1").expect("deletes");
        assert!(store.task("t1").expect("reads").is_none());
        assert!(store.tasks_in_list("l1").expect("reads").is_empty());
        assert!(store.comments_for_task("t1").expect("reads").is_empty());
    }

    #[test]
    fn comments_and_messages_come_back_oldest_first() {
        let store = Store::in_memory().expect("opens");
        for (id, at) in [
            ("c2", "2026-09-07T12:00:00Z"),
            ("c1", "2026-09-06T12:00:00Z"),
        ] {
            let comment: Comment = serde_json::from_value(serde_json::json!({
                "id": id, "taskId": "t1", "content": id, "createdAt": at
            }))
            .expect("decodes");
            store.upsert_comments(&[comment]).expect("stores");
        }
        let ids: Vec<String> = store
            .comments_for_task("t1")
            .expect("reads")
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec!["c1", "c2"]);
    }

    /// A queued edit written before the create was delivered still names the temporary id.
    #[test]
    fn a_temporary_id_resolves_to_the_one_the_server_gave_it() {
        let store = Store::in_memory().expect("opens");
        let now = date::parse("2026-09-07T12:00:00Z").expect("an instant");
        store
            .record_id_mapping("temp_abc", "cm3real", now)
            .expect("records");
        assert_eq!(store.resolve_id("temp_abc").expect("resolves"), "cm3real");
        // An id nobody remapped is itself.
        assert_eq!(store.resolve_id("cm3real").expect("resolves"), "cm3real");
    }

    /// A cache that outlives its session shows the previous user's tasks to the next one.
    #[test]
    fn signing_out_leaves_nothing_behind() {
        let store = Store::in_memory().expect("opens");
        store
            .upsert_task(&task_in_lists("t1", &["l1"]))
            .expect("stores");
        store
            .upsert_lists(&[TaskList::new("l1", "Home")])
            .expect("stores");
        store
            .set_metadata("cursor", "2026-09-07T12:00:00Z")
            .expect("stores");

        store.clear().expect("clears");
        assert!(store.tasks().expect("reads").is_empty());
        assert!(store.lists().expect("reads").is_empty());
        assert!(store.metadata("cursor").expect("reads").is_none());
    }

    #[test]
    fn metadata_is_a_place_to_keep_the_sync_cursor() {
        let store = Store::in_memory().expect("opens");
        assert!(store.metadata("cursor").expect("reads").is_none());
        store.set_metadata("cursor", "1").expect("stores");
        store.set_metadata("cursor", "2").expect("stores");
        assert_eq!(
            store.metadata("cursor").expect("reads").as_deref(),
            Some("2")
        );
    }
}
