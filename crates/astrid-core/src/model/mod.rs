//! The wire shapes, exactly as the server sends them.
//!
//! Ported from `astrid-ios/Astrid App/Models/`. These types are shared by the API client, the
//! SQLite cache and the Outbox, and they are the only description of the wire contract this crate
//! has — so they follow it rather than improving on it.
//!
//! ## Why decoding is deliberately forgiving
//!
//! Three clients read the same JSON from a server whose schema is permissive, and a decode that
//! fails takes the whole response with it. Every outage in the sibling repos that looked like
//! "the app went offline with 0 lists" was one field on one row failing an otherwise healthy
//! response: a `priority: 4` the schema never capped, an `aiAgentsEnabled` that arrived as an
//! object.
//!
//! So there are two layers of tolerance, and both matter:
//!
//! 1. **Within a row** — optional everywhere, enums fall back rather than throw. Each case is
//!    documented where it lives, with the incident that bought it.
//! 2. **Across an array** — [`lenient`] decodes element by element, so one unreadable row costs
//!    that row and nothing else. The API client uses it for every collection endpoint.
//!
//! Leniency is not silence: [`Lenient::skipped`] carries what was dropped and why, and the client
//! logs it. A row that vanishes without a trace is a worse bug than a response that fails loudly.

pub mod chat;
pub mod date;
pub mod list;
pub mod notification;
pub mod project;
pub mod task;
pub mod user;

pub use chat::{is_temp_id, ChatChannel, ChatMessage, TEMP_ID_PREFIX};
pub use list::{
    DurationUnit, ListAgentConfig, ListInvite, ListMember, Privacy, RecentlyCompletedWindow,
    TaskList,
};
pub use notification::{Inbox, Notification, NotificationTask};
pub use project::{Project, ProjectMember};
pub use task::{
    Attachment, Comment, CommentType, CustomRepeatingPattern, MonthWeekday, Priority, ReminderType,
    RepeatFromMode, Repeating, SecureFile, Task,
};
pub use user::User;

use serde::de::DeserializeOwned;

/// One row an array decode had to drop, and what went wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Its position in the array as the server sent it.
    pub index: usize,
    /// The row's `id`, when it had one worth naming. Nothing else from the row is kept: the
    /// message ends up in logs, and a task title does not belong there.
    pub id: Option<String>,
    pub reason: String,
}

/// The result of decoding an array leniently: what came through, and what did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lenient<T> {
    pub items: Vec<T>,
    pub skipped: Vec<Skipped>,
}

impl<T> Lenient<T> {
    pub fn is_complete(&self) -> bool {
        self.skipped.is_empty()
    }

    /// Drop the diagnostics once they have been logged.
    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}

/// Decode an array element by element, keeping what decodes.
///
/// A non-array (an object, a null, a string) yields nothing and one [`Skipped`] naming the shape —
/// which is a real case: several endpoints return `{ "error": ... }` with a 200.
pub fn lenient<T: DeserializeOwned>(value: serde_json::Value) -> Lenient<T> {
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        other => {
            return Lenient {
                items: Vec::new(),
                skipped: vec![Skipped {
                    index: 0,
                    id: None,
                    reason: format!("expected an array, got {}", shape_of(&other)),
                }],
            }
        }
    };

    let mut items = Vec::with_capacity(rows.len());
    let mut skipped = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let id = row
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        match serde_json::from_value::<T>(row) {
            Ok(item) => items.push(item),
            Err(error) => skipped.push(Skipped {
                index,
                id,
                reason: error.to_string(),
            }),
        }
    }
    Lenient { items, skipped }
}

fn shape_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the 2026-08-29 outage: one row the client cannot read must cost that row, not
    /// the account's whole sidebar.
    #[test]
    fn one_unreadable_row_does_not_take_the_array_with_it() {
        let payload = serde_json::json!([
            { "id": "l1", "name": "Home" },
            { "name": "Nameless — no id, which TaskList requires" },
            { "id": "l3", "name": "Work" },
        ]);
        let decoded = lenient::<TaskList>(payload);
        let ids: Vec<&str> = decoded.items.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, vec!["l1", "l3"]);
        assert!(!decoded.is_complete());
        assert_eq!(decoded.skipped.len(), 1);
        assert_eq!(decoded.skipped[0].index, 1);
    }

    /// What was dropped has to be nameable, or the next outage is invisible instead of partial.
    #[test]
    fn a_skipped_row_is_reported_with_its_id() {
        let payload = serde_json::json!([{ "id": "t1", "title": 7 }]);
        let decoded = lenient::<Task>(payload);
        assert!(decoded.items.is_empty());
        assert_eq!(decoded.skipped[0].id.as_deref(), Some("t1"));
        assert!(!decoded.skipped[0].reason.is_empty());
    }

    /// Several endpoints answer 200 with an error object. Reading that as "no tasks" silently
    /// empties the list; it has to arrive as a diagnostic.
    #[test]
    fn a_response_that_is_not_an_array_is_reported_rather_than_read_as_empty() {
        let decoded = lenient::<Task>(serde_json::json!({ "error": "nope" }));
        assert!(decoded.items.is_empty());
        assert!(decoded.skipped[0].reason.contains("an object"));
    }

    #[test]
    fn a_clean_array_reports_nothing_skipped() {
        let decoded = lenient::<Task>(serde_json::json!([{ "id": "t1" }, { "id": "t2" }]));
        assert!(decoded.is_complete());
        assert_eq!(decoded.into_items().len(), 2);
    }
}
