//! The offline cache: everything the app knows, on disk, readable without a network.
//!
//! This is the read path. It never waits on a request, and it is authoritative for what the UI
//! draws — sync writes into it, the Outbox writes into it optimistically, and the shell renders
//! whatever it holds. That ordering is what makes the app work on a train, and it is the reason
//! rule 5 of README.md exists.
//!
//! Ported in spirit rather than in shape from `astrid-ios/Astrid App/Core/Persistence/`, which is
//! Core Data. The behaviour that matters — local-first reads, upsert-by-id, membership as a real
//! relation, temporary ids that survive until the server's arrives — is the same. See
//! [`schema`] for why each row keeps its original JSON beside the columns lifted out of it.
//!
//! ## Threading
//!
//! Two connections, and every method is still synchronous.
//!
//! **A writer** behind a mutex, which every write and every transaction takes, and **a reader**
//! opened `SQLITE_OPEN_READ_ONLY` behind its own, which every read takes. The split is the point:
//! with WAL on, a reader on its own connection sees the last committed snapshot and never waits
//! for a writer, so a sync pass applying a fetched page inside one transaction
//! (`sync/mod.rs`, AITD-454) no longer stalls the list being drawn at the time. With one shared
//! connection it did, for as long as the pass took — task `bb20ca56`, and the reason WAL bought
//! nothing before.
//!
//! An in-memory cache has **no** reader: two connections to `:memory:` are two separate, empty
//! databases, so [`Store::in_memory`] leaves `reader` as `None` and reads fall back to the
//! writer. Every test that does not need the contention therefore behaves exactly as before.
//!
//! **Why the methods stay synchronous, and where `spawn_blocking` goes.** A cache read is
//! sub-millisecond and, now, uncontended; handing each one to `tokio::task::spawn_blocking` would
//! add a thread hop and a `Send` bound per call and make `rowsForList` slower, against the one
//! bound `tests/performance.rs` calls "not generous". So the store keeps a plain blocking API and
//! the *caller* decides: the long, genuinely blocking store work — sync's per-phase apply — is
//! what runs on `spawn_blocking`, at the one place that work is issued from async code. A
//! dedicated store thread fed by a channel was the alternative; it was rejected because every
//! call site here is synchronous, so a channel would make each one block on `recv` from a tokio
//! worker — the same stall with an extra hop — unless the whole API went async, which the FFI
//! door (`run_json`) is explicitly not allowed to change.
//!
//! The mutex is never held across anything that can block, `busy_timeout` covers the case where
//! another process has the write lock, and a poisoned mutex is [`StoreError::Lock`] rather than a
//! panic — this code runs inside an FFI call, and unwinding across that boundary takes the shell
//! down with nothing to report.

pub mod schema;
pub mod session;

use std::collections::HashMap;
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
    /// A write panicked while it held the connection, so the mutex is poisoned and nothing can be
    /// trusted to read it. An error rather than a panic of its own: this is reached from inside an
    /// FFI call, and unwinding across that boundary takes the shell down with no diagnosis
    /// (task bb20ca56).
    #[error("the cache is unusable because an earlier write failed part-way")]
    Lock,
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// What the list view needs, and nothing else — read straight from the indexed columns.
///
/// **Not yet what the list view reads**, though this said it was until task `71ddacb8`:
/// [`Store::summaries_in_list`] has only ever had test callers, because the filter and sort rules
/// the list goes through (`astrid_rules::filters`) take whole `Task`s. Anything a detail view
/// needs comes from [`Store::task`], which decodes.
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
    /// Every write and every transaction. One at a time, by the mutex.
    connection: Mutex<Connection>,
    /// Every read, on a connection that cannot write and so never waits for the writer.
    ///
    /// `None` for an in-memory cache, where a second connection would be a second database; reads
    /// then fall back to [`Store::connection`]. See the module header.
    reader: Option<Mutex<Connection>>,
    /// Who may write the signed-in account's data, and when sign-out has closed that — see
    /// [`session`].
    session: session::SessionGate,
}

impl Store {
    /// The account's session: passes enter it, sign-out closes it.
    pub fn session(&self) -> &session::SessionGate {
        &self.session
    }

    /// How long a statement waits for another connection's write lock before giving up.
    ///
    /// Within this process the mutex already serialises writers, so this is for the case the
    /// mutex cannot see: a second process on the same cache file — the app and its UI-test build,
    /// or two copies started at once. Without it such a statement failed instantly with
    /// `SQLITE_BUSY` (task `bb20ca56`).
    const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    /// Open (and migrate) the cache at `path`, creating it if it is not there.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let connection = Connection::open(path)?;
        let mut store = Self::prepare(connection)?;
        // After `prepare`, so the file and its schema exist and WAL is on before anything tries
        // to read them. A read-only connection to a WAL database needs the `-shm` file, which the
        // writer above has now created and keeps alive.
        let reader = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        reader.busy_timeout(Self::BUSY_TIMEOUT)?;
        reader.pragma_update(None, "foreign_keys", "ON")?;
        store.reader = Some(Mutex::new(reader));
        Ok(store)
    }

    /// A cache that lives only as long as the process. Used by every test, and by the shell's
    /// UI-test build, which must never touch the real user's data.
    ///
    /// Single-connection by necessity: a second connection to `:memory:` is a second, empty
    /// database, so there is no reader to open and reads go to the writer.
    pub fn in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut connection: Connection) -> Result<Self> {
        // WAL so a read never blocks behind a write: sync writes while the user is scrolling is
        // the normal case, not the exceptional one. It buys that only in company with the
        // read-only connection `open` adds — on one shared connection the mutex serialised them
        // anyway (task `bb20ca56`).
        connection.pragma_update(None, "journal_mode", "WAL")?;
        // NORMAL rather than FULL. The cache is a cache: the Outbox is what makes a write durable,
        // and paying an fsync per statement to protect data the server already has is the wrong
        // trade on a laptop with a spinning disk.
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(Self::BUSY_TIMEOUT)?;
        schema::migrate(&mut connection)?;
        Ok(Store {
            connection: Mutex::new(connection),
            reader: None,
            session: session::SessionGate::default(),
        })
    }

    /// Run `work` inside a transaction. Used by sync, which must never leave half a snapshot
    /// visible to a list that is being drawn at the time.
    pub fn transaction<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        opened::record_transaction();
        let mut guard = self.connection.lock().map_err(|_| StoreError::Lock)?;
        let transaction = guard.transaction()?;
        let value = work(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    /// The writer itself, mutably, outside a transaction. rusqlite installs a hook only through
    /// `&mut Connection` (since 0.33), and a test that counts commits has to install one.
    #[cfg(test)]
    pub(crate) fn writer_mut<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.connection.lock().map_err(|_| StoreError::Lock)?;
        work(&mut guard)
    }

    /// Run `work` on the read connection.
    ///
    /// For the modules that own a table of their own — the Outbox's journal — so that a question
    /// about one row can be asked without [`Store::transaction`], which takes the writer's lock
    /// and opens a transaction to do it (task `71ddacb8`). Read-only by construction: on a real
    /// cache this connection is `SQLITE_OPEN_READ_ONLY` and a write through it fails.
    pub fn reading<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.read(work)
    }

    /// The writer. Every method that changes a row goes through here.
    fn with<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.connection.lock().map_err(|_| StoreError::Lock)?;
        work(&guard)
    }

    /// The reader, falling back to the writer when there is none (an in-memory cache).
    ///
    /// Reads must come through here rather than [`Store::with`] so that drawing a list does not
    /// wait for a sync pass to commit — see the module header.
    fn read<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        opened::record_read();
        match &self.reader {
            Some(reader) => {
                let guard = reader.lock().map_err(|_| StoreError::Lock)?;
                work(&guard)
            }
            None => self.with(work),
        }
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
        self.read(|connection| task_in(connection, id))
    }

    pub fn tasks(&self) -> Result<Vec<Task>> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT json FROM tasks")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// Every cached task that is not finished.
    ///
    /// For a reader that drops finished tasks as its *first* act, so the `completed` column
    /// answers precisely what the Rust matcher would keep rather than merely a superset of it —
    /// the palette, whose two passes over its slice both open with `!task.completed` (task
    /// `3845b8a6`). It still decodes a whole [`Task`] per surviving row, because what the palette
    /// then reads (`identifier`, `title`, `effective_list_ids()`) lives only in `json`; what is
    /// bounded is the row *count*, and on an account whose history dwarfs its live work that is
    /// the larger half.
    ///
    /// Deliberately **not** indexed on `completed`. A skipped row costs one integer column read
    /// against a `serde_json` parse saved, and an index on a two-valued column where most rows
    /// match is one SQLite would decline to use anyway.
    ///
    /// The three task reads that *cannot* be bounded — `services::search`, `parse::mentions` and
    /// `dispatch::tasks::cache_search` — and the fields that force it are written up in
    /// `docs/CONTRACTS.md`.
    pub fn tasks_incomplete(&self) -> Result<Vec<Task>> {
        self.read(|connection| {
            let mut statement =
                connection.prepare_cached("SELECT json FROM tasks WHERE completed = 0")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// Every cached task as `(id, updated_at)`.
    ///
    /// The whole of what a full pass's absence scan needs (task `c8457e0f`): it compares each
    /// cached id against what the listing returned and each stamp against the moment the pass
    /// began, and wants nothing else from the row. It asked [`Store::tasks`] for that — `SELECT
    /// json` plus a full decode per row — which is task `71ddacb8`'s shape in the one read that
    /// grows with how long the app has been installed. Both columns are their own, and
    /// `updated_at` is indexed.
    pub fn task_stamps(&self) -> Result<Vec<(String, Option<DateTime<Utc>>)>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached("SELECT id, updated_at FROM tasks")?;
            let rows = statement.query_map([], read_stamp)?;
            Ok(rows.collect::<SqlResult<Vec<_>>>()?)
        })
    }

    /// Whether the cache holds any task at all.
    ///
    /// `EXISTS`, so asking stops at the first row. The callers only ever wanted the question —
    /// "is this cache empty?" — and answered it by decoding the account to call `.is_empty()` on
    /// it (task `71ddacb8`).
    pub fn has_any_tasks(&self) -> Result<bool> {
        self.read(|connection| {
            Ok(
                connection.query_row("SELECT EXISTS (SELECT 1 FROM tasks)", [], |row| {
                    row.get::<_, i64>(0)
                })? != 0,
            )
        })
    }

    /// The tasks made here that the server has not answered for yet — a range on the primary
    /// key, so a pass does not decode every task to find the few it wants.
    pub fn temp_tasks(&self) -> Result<Vec<Task>> {
        self.read(temp_tasks_in)
    }

    /// The unfinished tasks whose reminder falls in `from..=to`.
    ///
    /// What a reminder tick asks, in place of the whole account (task `71ddacb8`). On the partial
    /// index from migration 2, which is why `IS NOT NULL` is spelled out: the planner only uses a
    /// partial index for a query whose own `WHERE` implies its predicate.
    ///
    /// Completed tasks are excluded here rather than afterwards because
    /// [`astrid_rules::reminders::due_now`] drops them anyway — the filter belongs wherever it can
    /// be applied before a row is decoded.
    pub fn tasks_with_reminder_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<Task>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT json FROM tasks
                 WHERE reminder_time IS NOT NULL AND completed = 0
                   AND reminder_time >= ?1 AND reminder_time <= ?2",
            )?;
            let rows = statement.query_map([date::format(from), date::format(to)], |row| {
                row.get::<_, String>(0)
            })?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// The unfinished tasks whose reminder is still to come after `after`.
    ///
    /// Unbounded above on purpose: the caller wants the earliest, and which row that is depends on
    /// a snooze and on what has already been shown — neither of which SQL can see — so a `LIMIT 1`
    /// here would answer with a row the caller then discards and no way to ask for the next.
    /// Bounding it to "has a future reminder" is what takes this off the size of the account.
    pub fn tasks_with_reminder_after(&self, after: DateTime<Utc>) -> Result<Vec<Task>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT json FROM tasks
                 WHERE reminder_time IS NOT NULL AND completed = 0 AND reminder_time > ?1
                 ORDER BY reminder_time",
            )?;
            let rows = statement.query_map([date::format(after)], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// The tasks with these ids, in no particular order; ids the cache does not hold are absent.
    ///
    /// For a caller holding a small known set — the tasks with a snooze mark on this device —
    /// which would otherwise read the account to find them.
    pub fn tasks_with_ids(&self, ids: &[String]) -> Result<Vec<Task>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.read(|connection| {
            let placeholders = vec!["?"; ids.len()].join(", ");
            let mut statement = connection.prepare(&format!(
                "SELECT json FROM tasks WHERE id IN ({placeholders})"
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(ids), |row| {
                row.get::<_, String>(0)
            })?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// Every task in any of `list_ids`, each task once however many of them it is in.
    ///
    /// On `membership_by_list`. For the reads whose scope is a known set of lists — a project's
    /// board, a provider's linked list — which read the whole account and filtered afterwards
    /// (task `71ddacb8`).
    pub fn tasks_in_lists(&self, list_ids: &[String]) -> Result<Vec<Task>> {
        if list_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.read(|connection| {
            let placeholders = vec!["?"; list_ids.len()].join(", ");
            let mut statement = connection.prepare(&format!(
                "SELECT t.json FROM tasks t
                 WHERE EXISTS (
                     SELECT 1 FROM task_lists_membership m
                     WHERE m.task_id = t.id AND m.list_id IN ({placeholders})
                 )"
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(list_ids), |row| {
                row.get::<_, String>(0)
            })?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn tasks_in_list(&self, list_id: &str) -> Result<Vec<Task>> {
        self.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT t.json FROM tasks t
                 JOIN task_lists_membership m ON m.task_id = t.id
                 WHERE m.list_id = ?1",
            )?;
            let rows = statement.query_map([list_id], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// Every cached task that has a parent — what a list splices under its rows. On the parent
    /// index, so a list does not decode the whole account to find a handful of subtasks.
    pub fn subtasks(&self) -> Result<Vec<Task>> {
        self.read(|connection| {
            let mut statement =
                connection.prepare("SELECT json FROM tasks WHERE parent_task_id IS NOT NULL")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// The list view's read: columns only, no JSON.
    pub fn summaries_in_list(&self, list_id: &str) -> Result<Vec<TaskSummary>> {
        self.read(|connection| {
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
        self.read(|connection| {
            let mut statement =
                connection.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM tasks t"))?;
            let rows = statement.query_map([], read_summary)?;
            Ok(rows.collect::<SqlResult<Vec<_>>>()?)
        })
    }

    /// The ids of the lists a task belongs to.
    pub fn list_ids_for_task(&self, task_id: &str) -> Result<Vec<String>> {
        self.read(|connection| {
            let mut statement = connection
                .prepare("SELECT list_id FROM task_lists_membership WHERE task_id = ?1")?;
            let rows = statement.query_map([task_id], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<SqlResult<Vec<_>>>()?)
        })
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        self.with(|connection| delete_task_in(connection, id))
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
        self.read(|connection| list_in(connection, id))
    }

    pub fn lists(&self) -> Result<Vec<TaskList>> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT json FROM lists")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// Every cached list a server could be asked about, as `(id, updated_at)`.
    ///
    /// [`Store::task_stamps`]'s counterpart for the same scan, and virtual lists are left out
    /// here rather than filtered afterwards: a virtual list is this client's own — no server has
    /// one, so no server can confirm it gone.
    pub fn real_list_stamps(&self) -> Result<Vec<(String, Option<DateTime<Utc>>)>> {
        self.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT id, updated_at FROM lists WHERE is_virtual = 0")?;
            let rows = statement.query_map([], read_stamp)?;
            Ok(rows.collect::<SqlResult<Vec<_>>>()?)
        })
    }

    pub fn delete_list(&self, id: &str) -> Result<()> {
        self.with(|connection| delete_list_in(connection, id))
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

    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.with(|connection| {
            connection.execute("DELETE FROM projects WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.read(|connection| {
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
        self.read(|connection| {
            let json: Option<String> = connection
                .query_row("SELECT json FROM users WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()?;
            json.map(|json| decode(&json)).transpose()
        })
    }

    /// Every user we hold. Small by construction — the people on your lists, not a directory.
    pub fn users(&self) -> Result<Vec<User>> {
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT json FROM users")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// Store comments. A server comment that echoes an optimistic one's `clientRequestId` takes its
    /// place, whichever path brought it first — see [`Store::upsert_messages`].
    pub fn upsert_comments(&self, comments: &[Comment]) -> Result<()> {
        self.transaction(|connection| {
            for comment in comments {
                replace_echoed(
                    connection,
                    "comments",
                    "task_id",
                    &comment.task_id,
                    &comment.id,
                    comment.client_request_id.as_deref(),
                    comment.created_at,
                )?;
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
        self.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT json FROM comments WHERE task_id = ?1 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = statement.query_map([task_id], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    pub fn comment(&self, id: &str) -> Result<Option<Comment>> {
        self.read(|connection| {
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
        self.read(|connection| {
            let mut statement = connection.prepare("SELECT json FROM chat_channels")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|json| decode(&json?)).collect()
        })
    }

    /// Store messages. A server message that echoes an optimistic one's `clientRequestId` takes
    /// its place — the stream can bring the server's copy before the send's own answer does — and
    /// the optimistic id is recorded as resolving to the real one, so a reply to it still lands.
    pub fn upsert_messages(&self, messages: &[ChatMessage]) -> Result<()> {
        self.transaction(|connection| {
            for message in messages {
                replace_echoed(
                    connection,
                    "chat_messages",
                    "channel_id",
                    &message.channel_id,
                    &message.id,
                    message.client_request_id.as_deref(),
                    message.created_at,
                )?;
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
        self.read(|connection| {
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
        self.with(|connection| record_id_mapping_in(connection, temp_id, server_id, now))
    }

    /// The server id for a temporary one, if it has been delivered. Follows a chain, because a
    /// temporary id can in principle be remapped twice; a cycle stops rather than hangs.
    pub fn resolve_id(&self, id: &str) -> Result<String> {
        self.read(|connection| resolve_in(connection, id))
    }

    /// Resolve many ids under ONE read.
    ///
    /// The loop that calls [`Store::resolve_id`] per item pays the read lock once per item, and
    /// `journal::ids_named` does exactly that over the whole journal, two to four times a sync
    /// (task `f7d45608`). Ids absent from the mapping table resolve to themselves, so the answer
    /// has an entry for every id asked about.
    pub fn resolve_ids<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<HashMap<String, String>> {
        let ids: Vec<&str> = ids.into_iter().collect();
        self.read(|connection| {
            let mut resolved = HashMap::with_capacity(ids.len());
            for id in ids {
                if resolved.contains_key(id) {
                    continue;
                }
                resolved.insert(id.to_string(), resolve_in(connection, id)?);
            }
            Ok(resolved)
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

    /// Read `keys`, let `work` change them, and write back what changed — all in one transaction.
    ///
    /// For a write that spans several entries and must land whole or not at all: a migration
    /// that fails half way is retried from the start, not resumed from wherever it stopped.
    pub fn update_metadata(
        &self,
        keys: &[String],
        work: impl FnOnce(&mut std::collections::HashMap<String, Option<String>>),
    ) -> Result<()> {
        self.transaction(|connection| {
            let mut values = std::collections::HashMap::new();
            for key in keys {
                let value: Option<String> = connection
                    .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                        row.get(0)
                    })
                    .optional()?;
                values.insert(key.clone(), value);
            }
            let before = values.clone();
            work(&mut values);
            for (key, value) in &values {
                if before.get(key) == Some(value) {
                    continue;
                }
                match value {
                    Some(value) => connection.execute(
                        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
                        [key.as_str(), value.as_str()],
                    )?,
                    None => connection.execute("DELETE FROM metadata WHERE key = ?1", [key])?,
                };
            }
            Ok(())
        })
    }

    pub fn metadata(&self, key: &str) -> Result<Option<String>> {
        self.read(|connection| {
            Ok(connection
                .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .optional()?)
        })
    }

    /// Every metadata entry whose key starts with `prefix`, as `(key, value)`.
    ///
    /// For per-task marks read in one pass rather than one query per task — the reminder loop
    /// reads the snooze marks this way on every tick.
    pub fn metadata_with_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        self.read(|connection| {
            let mut statement = connection
                .prepare("SELECT key, value FROM metadata WHERE substr(key, 1, length(?1)) = ?1")?;
            let rows = statement
                .query_map([prefix], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<std::result::Result<Vec<(String, String)>, _>>()?;
            Ok(rows)
        })
    }

    /// Forget everything. Sign-out calls this: a cache that outlives its session is a cache that
    /// shows the previous user's tasks to the next one.
    /// Forget the account's rows but keep what it has not sent and how to sign in: the next sync
    /// is a full pass that rebuilds them. For a cache that has been caught holding what is not
    /// this account's — a shell's isolation guard — where signing out would be the wrong answer to
    /// a guard that can be wrong.
    pub fn clear_rows(&self, sync_stamp_key: &str) -> Result<()> {
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
            ] {
                connection.execute(&format!("DELETE FROM {table}"), [])?;
            }
            connection.execute("DELETE FROM metadata WHERE key = ?1", [sync_stamp_key])?;
            Ok(())
        })
    }

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

/// An `(id, updated_at)` pair, for the reads that want a row's identity and age and nothing else.
/// See [`Store::task_stamps`].
fn read_stamp(row: &rusqlite::Row<'_>) -> SqlResult<(String, Option<DateTime<Utc>>)> {
    Ok((
        row.get(0)?,
        row.get::<_, Option<String>>(1)?
            .as_deref()
            .and_then(date::parse),
    ))
}

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
        // NULL since task ebe0966b: the column is written from `Task::description`, which is
        // `None` for a task whose description the response never sent. Nothing to draw either way.
        has_description: row
            .get::<_, Option<String>>(13)?
            .is_some_and(|text| !text.trim().is_empty()),
        timer_duration: row.get(14)?,
        occurrence_count: row.get(15)?,
        comment_count: row.get(16)?,
        attachment_count: row.get(17)?,
        created_at: instant(row.get(18)?),
        updated_at: instant(row.get(19)?),
    })
}

// ─── The same, on a connection a caller already holds ─────────────────────────────────────────
//
// What a sync pass folds its rows in with, inside one `Store::transaction` (AITD-454): per-row
// calls through `Store` each took the lock and auto-committed, a journal write per statement.
// Statements are `prepare_cached`, so a pass of a thousand rows compiles each one once.

pub(crate) fn task_in(connection: &Connection, id: &str) -> Result<Option<Task>> {
    let json: Option<String> = connection
        .prepare_cached("SELECT json FROM tasks WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
        .optional()?;
    json.map(|json| decode(&json)).transpose()
}

pub(crate) fn temp_tasks_in(connection: &Connection) -> Result<Vec<Task>> {
    let mut statement =
        connection.prepare_cached("SELECT json FROM tasks WHERE id >= 'temp_' AND id < 'temp`'")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|json| decode(&json?)).collect()
}

pub(crate) fn delete_task_in(connection: &Connection, id: &str) -> Result<()> {
    connection
        .prepare_cached("DELETE FROM tasks WHERE id = ?1")?
        .execute([id])?;
    connection
        .prepare_cached("DELETE FROM task_lists_membership WHERE task_id = ?1")?
        .execute([id])?;
    connection
        .prepare_cached("DELETE FROM comments WHERE task_id = ?1")?
        .execute([id])?;
    Ok(())
}

pub(crate) fn list_in(connection: &Connection, id: &str) -> Result<Option<TaskList>> {
    let json: Option<String> = connection
        .prepare_cached("SELECT json FROM lists WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
        .optional()?;
    json.map(|json| decode(&json)).transpose()
}

pub(crate) fn delete_list_in(connection: &Connection, id: &str) -> Result<()> {
    connection
        .prepare_cached("DELETE FROM lists WHERE id = ?1")?
        .execute([id])?;
    connection
        .prepare_cached("DELETE FROM task_lists_membership WHERE list_id = ?1")?
        .execute([id])?;
    Ok(())
}

pub(crate) fn record_id_mapping_in(
    connection: &Connection,
    temp_id: &str,
    server_id: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    connection
        .prepare_cached(
            "INSERT OR REPLACE INTO id_mappings (temp_id, server_id, created_at)
             VALUES (?1, ?2, ?3)",
        )?
        .execute(rusqlite::params![temp_id, server_id, date::format(now)])?;
    Ok(())
}

pub(crate) fn upsert_task_in(connection: &Connection, task: &Task) -> Result<()> {
    connection
        .prepare_cached(
            "INSERT OR REPLACE INTO tasks (
            id, title, completed, completed_at, due_date_time, is_all_day, priority, repeating,
            parent_task_id, assignee_id, creator_id, status_role, is_private, description,
            timer_duration, occurrence_count, comment_count, attachment_count,
            created_at, updated_at, reminder_time, json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                   ?18, ?19, ?20, ?21, ?22)",
        )?
        .execute(rusqlite::params![
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
            // Lifted out so a reminder tick can ask for a window rather than the account
            // (task 71ddacb8). Not the due date: the two move independently.
            task.reminder_time.map(date::format),
            encode(task)?,
        ])?;

    connection
        .prepare_cached("DELETE FROM task_lists_membership WHERE task_id = ?1")?
        .execute([&task.id])?;
    let mut member = connection.prepare_cached(
        "INSERT OR IGNORE INTO task_lists_membership (task_id, list_id) VALUES (?1, ?2)",
    )?;
    for list_id in task.effective_list_ids() {
        member.execute([&task.id, &list_id])?;
    }
    Ok(())
}

pub(crate) fn upsert_list_in(connection: &Connection, list: &TaskList) -> Result<()> {
    connection
        .prepare_cached(
            "INSERT OR REPLACE INTO lists (
            id, name, color, privacy, owner_id, project_id, list_type, status_role, status_order,
            is_favorite, favorite_order, is_virtual, task_count, created_at, updated_at, json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )?
        .execute(rusqlite::params![
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
        ])?;
    Ok(())
}

/// Follow the chain of id mappings from `id` to whatever it finally resolves to.
///
/// Bounded at eight hops: a cycle in the mapping table would otherwise spin here for ever, and
/// eight is more rewrites than any real id goes through.
fn resolve_in(connection: &Connection, id: &str) -> Result<String> {
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
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| StoreError::Corrupt(error.to_string()))
}

fn decode<T: DeserializeOwned>(json: &str) -> Result<T> {
    decodes::record();
    serde_json::from_str(json).map_err(|error| StoreError::Corrupt(error.to_string()))
}

/// How many cached rows this thread has decoded from `json`.
///
/// The cache's one cost that grows with the size of the account: a column read is an indexed
/// lookup whatever the account holds, and a `json` decode is a full parse per row. So "how many
/// rows did that cost" is the number worth asserting on, and task `71ddacb8` — a reminder tick
/// decoding the whole table every thirty seconds — is what it was added for. A read bounded in SQL
/// shows up here as a smaller number; one bounded in Rust after the fact does not.
///
/// **Per thread, and deliberately.** A process-wide counter would make any two tests that assert
/// on it flaky the moment `cargo test` ran them on different threads at the same time, and a
/// counter nobody can trust is worse than none. Each test therefore sees only its own decodes; a
/// read handed to `spawn_blocking` is counted on the thread that runs it, not on the one that
/// asked.
pub mod decodes {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }

    /// The count so far on this thread.
    pub fn count() -> u64 {
        COUNT.with(Cell::get)
    }

    /// Start counting again from zero — what a test calls before the read it is bounding.
    pub fn reset() {
        COUNT.with(|count| count.set(0));
    }

    pub(super) fn record() {
        COUNT.with(|count| count.set(count.get().saturating_add(1)));
    }
}

/// How many connections this thread has taken: `transactions` is [`Store::transaction`] — the
/// writer's lock plus a BEGIN/COMMIT — and `reads` is the read connection.
///
/// The cost [`decodes`] cannot see. A question answered by one indexed lookup still costs the
/// writer's lock and a transaction if it is asked through [`Store::transaction`], and a loop that
/// asks a one-row question per item costs that lock once per item however cheap each answer is.
/// Both were true of the Outbox (task `f7d45608`): `has_pending` opened a transaction after every
/// command, a drain pass read the journal twice, and `journal::ids_named` took the read lock once
/// per journalled entry. A millisecond budget cannot tell those from a fast laptop; a count can.
///
/// **Per thread**, for the reason [`decodes`] gives: a process-wide counter would make any two
/// tests that assert on it flaky as soon as `cargo test` ran them at the same time.
pub mod opened {
    use std::cell::Cell;

    thread_local! {
        static TRANSACTIONS: Cell<u64> = const { Cell::new(0) };
        static READS: Cell<u64> = const { Cell::new(0) };
    }

    /// Write transactions opened on this thread so far.
    pub fn transactions() -> u64 {
        TRANSACTIONS.with(Cell::get)
    }

    /// Read-connection passes taken on this thread so far.
    pub fn reads() -> u64 {
        READS.with(Cell::get)
    }

    /// Start counting both again from zero — what a test calls before the work it is bounding.
    pub fn reset() {
        TRANSACTIONS.with(|count| count.set(0));
        READS.with(|count| count.set(0));
    }

    pub(super) fn record_transaction() {
        TRANSACTIONS.with(|count| count.set(count.get().saturating_add(1)));
    }

    pub(super) fn record_read() {
        READS.with(|count| count.set(count.get().saturating_add(1)));
    }
}

/// Drop the optimistic rows (`temp_` ids) in `table` whose `clientRequestId` the server's row
/// `id` echoes, recording each as resolving to it — so a reply or an edit written against the
/// optimistic id still lands. Scoped to the row's owner (`owner_column` = `owner`).
fn replace_echoed(
    connection: &rusqlite::Connection,
    table: &str,
    owner_column: &str,
    owner: &str,
    id: &str,
    client_request_id: Option<&str>,
    created_at: Option<DateTime<Utc>>,
) -> Result<()> {
    let Some(request_id) = client_request_id else {
        return Ok(());
    };
    if crate::model::is_temp_id(id) {
        return Ok(());
    }
    let echoed: Vec<String> = {
        let mut statement = connection.prepare(&format!(
            // A range on the primary key rather than LIKE, which no index can serve: only this
            // machine's few optimistic rows are ever looked at, however long the thread.
            "SELECT id FROM {table} WHERE id >= 'temp_' AND id < 'temp`' AND id != ?1
             AND {owner_column} = ?2 AND json_extract(json, '$.clientRequestId') = ?3"
        ))?;
        let rows = statement.query_map(rusqlite::params![id, owner, request_id], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for temp_id in echoed {
        connection.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [&temp_id])?;
        connection.execute(
            "INSERT OR REPLACE INTO id_mappings (temp_id, server_id, created_at)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![
                temp_id,
                id,
                date::format(created_at.unwrap_or_else(Utc::now))
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Repeating;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn task_in_lists(id: &str, lists: &[&str]) -> Task {
        let mut task = Task::new(id, format!("Task {id}"));
        task.list_ids = Some(lists.iter().map(|l| l.to_string()).collect());
        task
    }

    /// A directory for a test that needs the cache to be a real file.
    ///
    /// The contention these tests measure cannot exist on `:memory:`, where a second connection is
    /// a second, empty database. Hand-rolled rather than `tempfile`, which would be a dependency
    /// (and a `cargo deny` entry) bought for four lines.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn named(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock set after 1970")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("astrid-store-{label}-{nanos}"));
            std::fs::create_dir_all(&path).expect("a temporary directory");
            TempDir(path)
        }

        fn cache(&self) -> std::path::PathBuf {
            self.0.join("cache.sqlite")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Best effort: on Windows the WAL sidecars may still be held for an instant after the
            // connections close, and a test that failed to clean its temp directory has not found
            // a bug worth failing for.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn list_named(id: &str, name: &str) -> TaskList {
        serde_json::from_value(serde_json::json!({ "id": id, "name": name })).expect("a list")
    }

    /// Task `bb20ca56`: every write still goes to the writer, and the reader sees what it commits.
    ///
    /// This is the guard the rest of the suite cannot be: every other test opens
    /// [`Store::in_memory`], which has no reader and sends reads back to the writer, so a write
    /// method accidentally pointed at the read-only connection would pass all 800 of them and
    /// fail only on a real user's disk — with `SQLITE_READONLY` on something as ordinary as
    /// ticking a task off. It walks the single-statement writers, which are the ones at risk,
    /// and reads each back through the reader.
    #[test]
    fn bb20ca56_every_write_reaches_the_writer_on_a_real_file() {
        let directory = TempDir::named("bb20ca56-writes");
        let store = Store::open(directory.cache()).expect("opens");
        assert!(store.reader.is_some(), "an on-disk cache has a reader");

        // Writes through `with`, one statement each.
        store
            .upsert_list(&list_named("l1", "Everything"))
            .expect("stores a list");
        store
            .upsert_task(&task_in_lists("t1", &["l1"]))
            .expect("stores a task");
        store.set_metadata("cursor", "1").expect("stores metadata");
        store
            .record_id_mapping(
                "temp_1",
                "t1",
                date::parse("2026-09-07T12:00:00Z").expect("a date"),
            )
            .expect("stores a mapping");
        // And a write through `transaction`.
        store
            .upsert_tasks(&[task_in_lists("t2", &["l1"]), task_in_lists("t3", &["l1"])])
            .expect("stores tasks in a transaction");

        // Read back through the reader: a committed write has to be visible on the other
        // connection, which is the half of WAL that a single connection never exercised.
        assert_eq!(
            store.metadata("cursor").expect("reads").as_deref(),
            Some("1")
        );
        assert_eq!(store.resolve_id("temp_1").expect("reads"), "t1");
        assert!(store.list("l1").expect("reads").is_some());
        let mut ids: Vec<String> = store
            .summaries_in_list("l1")
            .expect("reads")
            .into_iter()
            .map(|summary| summary.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["t1", "t2", "t3"]);

        // The deletes are writes too, and the reader must see them go.
        store.delete_task("t2").expect("deletes a task");
        store.delete_list("l1").expect("deletes a list");
        assert!(store.task("t2").expect("reads").is_none());
        assert!(store.list("l1").expect("reads").is_none());
    }

    /// How long a read may take while a thousand-row apply holds the write transaction open.
    ///
    /// The bound task `bb20ca56` names. Generous for a read that does not wait at all — this one
    /// is sub-millisecond on any machine — and impossible for one that does.
    const READ_BUDGET: Duration = Duration::from_millis(50);

    /// Task `bb20ca56`: drawing a list must not wait for a sync pass to finish writing.
    ///
    /// Sync applies a whole fetched page inside one transaction (`sync/mod.rs`, AITD-454). With a
    /// single connection behind one mutex that meant `rowsForList` queued behind every row of it,
    /// so a pass arriving while somebody scrolled stalled the list for as long as the pass took.
    ///
    /// The apply here holds its transaction open deliberately. A real one is slow because it is
    /// slow; a test that raced a genuinely fast apply would pass whether or not the reader waits.
    #[test]
    fn bb20ca56_a_read_does_not_wait_for_a_sync_apply() {
        const ROWS: usize = 1_000;
        /// Long enough that a read which queues behind the apply cannot come in under the budget.
        const HELD: Duration = Duration::from_millis(400);

        let directory = TempDir::named("bb20ca56-apply");
        let store = Arc::new(Store::open(directory.cache()).expect("opens"));
        store
            .upsert_list(&list_named("l1", "Everything"))
            .expect("stores");

        let (applying, apply_started) = std::sync::mpsc::channel();
        let apply = {
            let store = store.clone();
            std::thread::spawn(move || {
                let tasks: Vec<Task> = (0..ROWS)
                    .map(|index| task_in_lists(&format!("t{index}"), &["l1"]))
                    .collect();
                store
                    .transaction(|connection| {
                        for task in &tasks {
                            upsert_task_in(connection, task)?;
                        }
                        // Every row is in and the transaction is still open: this is the instant
                        // at which somebody scrolling used to be stalled.
                        applying.send(()).expect("a reader waiting to be let go");
                        std::thread::sleep(HELD);
                        Ok(())
                    })
                    .expect("applies");
            })
        };

        apply_started.recv().expect("the apply to reach its rows");
        let at = Instant::now();
        let rows = store.summaries_in_list("l1").expect("reads");
        let elapsed = at.elapsed();
        apply.join().expect("the apply to finish");

        // Printed like the performance suite's numbers, so `--nocapture` says what this machine
        // actually does.
        println!("summaries_in_list during a {ROWS}-row apply: {elapsed:?}");
        assert!(
            elapsed < READ_BUDGET,
            "a read took {elapsed:?} while a {ROWS}-row apply held the write transaction, \
             which is more than the {READ_BUDGET:?} budget"
        );
        // The apply had not committed, so a reader correctly sees the account as it was. What is
        // being asserted is that it got an answer at all, not which answer.
        assert!(
            rows.is_empty(),
            "an uncommitted apply is not visible to a reader"
        );
    }

    /// Task `bb20ca56`: a poisoned cache lock is an error, not a panic inside the FFI call.
    ///
    /// `lock().expect("cache lock")` unwound across the language boundary, which is undefined
    /// behaviour on the shell's side of it and takes the app down with no diagnosis. One write
    /// that panicked used to make every later read do the same.
    #[test]
    fn bb20ca56_a_poisoned_cache_lock_is_an_error_not_a_panic() {
        let store = Arc::new(Store::in_memory().expect("opens"));

        let poisoner = {
            let store = store.clone();
            std::thread::spawn(move || {
                store.transaction(|_| -> Result<()> { panic!("a write that panics mid-statement") })
            })
        };
        // The panic is this test's own doing; the default hook would print its backtrace and read
        // like a failure.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = poisoner.join().is_err();
        std::panic::set_hook(hook);
        assert!(
            panicked,
            "the writing thread panicked and poisoned the lock"
        );

        match store.tasks() {
            Err(StoreError::Lock) => {}
            other => panic!("a read after a poisoned lock answered {other:?}"),
        }
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
        task.description = Some("two litres".into());
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
