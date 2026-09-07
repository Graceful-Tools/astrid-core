//! The cache's shape, and how it gets from one version to the next.
//!
//! Migrations are numbered and applied in order; `user_version` records where a database got to.
//! A migration is never edited once it has shipped — the next one fixes it — because the only
//! thing worse than a wrong schema is two machines disagreeing about which wrong schema they have.
//!
//! ## Why every table is "columns plus the JSON it came from"
//!
//! Each row keeps its whole model in `json`, and lifts out only the fields something queries,
//! sorts or draws. That is not redundancy, it buys two different things:
//!
//! - **Speed.** The list view reads its columns and never touches `json`, so drawing 10,000 rows
//!   costs one indexed scan rather than 10,000 JSON decodes. The M0 spike put a number on what
//!   that has to fit inside; see `docs/M0_NOTES.md`.
//! - **Room to move.** A field the models grow needs a model change and nothing else — it is
//!   already in `json`, and the cache only needs a migration when something has to be *queried*
//!   by it. Core Data made the opposite trade on Apple and every field cost a model version.
//!
//! What `json` holds is what this build decoded, not the bytes the server sent: unknown fields are
//! dropped at the model, on purpose, so a stale value can never be resurrected and sent back.
//!
//! The detail view, which shows one task, decodes `json` and gets everything.

use rusqlite::{Connection, Result};

/// Every migration, in order. The index in this array plus one is the `user_version` it produces.
const MIGRATIONS: &[&str] = &[
    // 1 — the cache as M1 leaves it.
    r#"
    CREATE TABLE tasks (
        id                TEXT PRIMARY KEY,
        title             TEXT NOT NULL DEFAULT '',
        completed         INTEGER NOT NULL DEFAULT 0,
        completed_at      TEXT,
        due_date_time     TEXT,
        is_all_day        INTEGER NOT NULL DEFAULT 1,
        priority          INTEGER NOT NULL DEFAULT 0,
        repeating         TEXT,
        parent_task_id    TEXT,
        assignee_id       TEXT,
        creator_id        TEXT,
        status_role       TEXT,
        is_private        INTEGER NOT NULL DEFAULT 0,
        description       TEXT NOT NULL DEFAULT '',
        timer_duration    INTEGER,
        occurrence_count  INTEGER,
        comment_count     INTEGER NOT NULL DEFAULT 0,
        attachment_count  INTEGER NOT NULL DEFAULT 0,
        created_at        TEXT,
        updated_at        TEXT,
        json              TEXT NOT NULL
    );
    CREATE INDEX tasks_by_parent   ON tasks(parent_task_id);
    CREATE INDEX tasks_by_due      ON tasks(due_date_time);
    CREATE INDEX tasks_by_assignee ON tasks(assignee_id);
    CREATE INDEX tasks_by_updated  ON tasks(updated_at);

    -- Membership is its own table rather than a JSON array on the task, because "the tasks in
    -- this list" is the single most common query in the app and it has to be an index lookup.
    CREATE TABLE task_lists_membership (
        task_id TEXT NOT NULL,
        list_id TEXT NOT NULL,
        PRIMARY KEY (task_id, list_id)
    );
    CREATE INDEX membership_by_list ON task_lists_membership(list_id);

    CREATE TABLE lists (
        id             TEXT PRIMARY KEY,
        name           TEXT NOT NULL DEFAULT '',
        color          TEXT,
        privacy        TEXT,
        owner_id       TEXT,
        project_id     TEXT,
        list_type      TEXT,
        status_role    TEXT,
        status_order   INTEGER,
        is_favorite    INTEGER NOT NULL DEFAULT 0,
        favorite_order INTEGER,
        is_virtual     INTEGER NOT NULL DEFAULT 0,
        task_count     INTEGER,
        created_at     TEXT,
        updated_at     TEXT,
        json           TEXT NOT NULL
    );
    CREATE INDEX lists_by_project ON lists(project_id);

    CREATE TABLE projects (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL DEFAULT '',
        owner_id   TEXT,
        created_at TEXT,
        updated_at TEXT,
        json       TEXT NOT NULL
    );

    CREATE TABLE users (
        id    TEXT PRIMARY KEY,
        name  TEXT,
        email TEXT,
        json  TEXT NOT NULL
    );

    CREATE TABLE comments (
        id         TEXT PRIMARY KEY,
        task_id    TEXT NOT NULL,
        author_id  TEXT,
        created_at TEXT,
        json       TEXT NOT NULL
    );
    CREATE INDEX comments_by_task ON comments(task_id);

    CREATE TABLE chat_channels (
        id          TEXT PRIMARY KEY,
        list_id     TEXT,
        virtual_key TEXT,
        json        TEXT NOT NULL
    );

    CREATE TABLE chat_messages (
        id         TEXT PRIMARY KEY,
        channel_id TEXT NOT NULL,
        author_id  TEXT,
        created_at TEXT,
        json       TEXT NOT NULL
    );
    CREATE INDEX chat_messages_by_channel ON chat_messages(channel_id, created_at);

    -- The write journal. Ordering is by `sequence`, which is why it is an explicit column and not
    -- a timestamp: two entries created in the same millisecond still have an order, and that order
    -- is the one the user performed them in.
    CREATE TABLE outbox (
        sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
        id                TEXT NOT NULL UNIQUE,
        kind              TEXT NOT NULL,
        payload           TEXT NOT NULL,
        client_request_id TEXT NOT NULL,
        depends_on        TEXT NOT NULL DEFAULT '[]',
        temp_id           TEXT,
        status            TEXT NOT NULL DEFAULT 'pending',
        attempts          INTEGER NOT NULL DEFAULT 0,
        next_attempt_at   TEXT NOT NULL,
        last_error        TEXT,
        result            TEXT,
        created_at        TEXT NOT NULL,
        updated_at        TEXT NOT NULL
    );
    CREATE INDEX outbox_by_status ON outbox(status, sequence);
    CREATE INDEX outbox_by_temp_id ON outbox(temp_id);

    -- The mapping from a temporary id this device minted to the id the server gave it. Kept after
    -- the entry that created it is gone, because a deep link, a notification or a queued edit can
    -- still be holding the temporary one.
    CREATE TABLE id_mappings (
        temp_id    TEXT PRIMARY KEY,
        server_id  TEXT NOT NULL,
        created_at TEXT NOT NULL
    );

    -- Sync bookkeeping and anything else that is one value under one name.
    CREATE TABLE metadata (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
];

/// The version a fully migrated database reports.
pub fn latest_version() -> i64 {
    MIGRATIONS.len() as i64
}

/// Bring `connection` up to [`latest_version`], applying only what it is missing.
///
/// Each migration runs inside a transaction with its `user_version` bump, so a machine that loses
/// power mid-migration comes back on the version it was on rather than on half of the next one.
pub fn migrate(connection: &mut Connection) -> Result<()> {
    let current: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (index, statements) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        let transaction = connection.transaction()?;
        transaction.execute_batch(statements)?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrating_an_empty_database_reaches_the_latest_version() {
        let mut connection = Connection::open_in_memory().expect("opens");
        migrate(&mut connection).expect("migrates");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("reads");
        assert_eq!(version, latest_version());
    }

    /// Launching twice is the normal case, and it must not try to create the tables again.
    #[test]
    fn migrating_twice_changes_nothing() {
        let mut connection = Connection::open_in_memory().expect("opens");
        migrate(&mut connection).expect("migrates");
        migrate(&mut connection).expect("migrates again");
        let tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
                [],
                |row| row.get(0),
            )
            .expect("reads");
        assert_eq!(tables, 1);
    }
}
