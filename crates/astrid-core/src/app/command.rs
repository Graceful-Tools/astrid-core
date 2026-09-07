//! The vocabulary the shell speaks.
//!
//! One enum in, one enum out. Both are `serde`-tagged on `kind`, so the wire form is readable in a
//! log and stable enough to write a C# type against by hand.
//!
//! ## What belongs here
//!
//! A command is something a person did — "complete this task", "show me this list". It is not a
//! step in doing it. `SaveTaskThenRefreshThenScroll` would be the shell deciding, which is the one
//! thing it may not do; the core decides what completing a task entails and the shell asks for the
//! result.
//!
//! ## Failures are values
//!
//! Every failure is a [`Failure`] with a machine-readable kind, because the shell has to tell three
//! situations apart and they look identical in a message string: **sign in again** (the session is
//! gone), **it will send later** (offline, and the Outbox has it), and **that did not work**
//! (the server refused). Showing the second as an error is how a working offline app comes to look
//! broken.

use serde::{Deserialize, Serialize};

use crate::rows::{DisplayMode, Surface};

#[derive(Debug, Clone, Deserialize)]
// `rename_all` names the variants, `rename_all_fields` names the fields inside them. Both are
// needed: without the second, `{"kind":"list","listId":"l1"}` reads as a command with no id and
// every screen that uses it fails as "bad request" with nothing to say why.
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Command {
    // ── Reads. These never touch the network. ────────────────────────────────────────────────
    /// Every list, for the sidebar.
    Lists,
    /// One list.
    List {
        list_id: String,
    },
    /// The rows to draw for a list, already filtered, sorted, spliced and projected.
    ///
    /// One command rather than "give me the tasks, and here is how I will filter them": the
    /// filtering, the sort, the subtask splice and the row projection are four contracts, and a
    /// shell that ran them itself would be four chances to disagree with web.
    RowsForList {
        list_id: String,
        #[serde(default)]
        display_mode: Option<String>,
        #[serde(default)]
        surface: Option<String>,
        /// Where to start, for a virtualised list. Absent means the beginning.
        #[serde(default)]
        offset: Option<usize>,
        /// How many rows are wanted. Absent means all of them — which is the right answer for a
        /// list of forty and the wrong one for a list of ten thousand.
        #[serde(default)]
        limit: Option<usize>,
    },
    /// One task in full, for the detail view.
    Task {
        task_id: String,
    },
    /// Everything one task's detail screen needs: the task, its comments, its subtasks, its list
    /// chips, and the order to lay the fields out in.
    ///
    /// One command rather than five, because the field order is a cross-platform product decision
    /// and a shell that assembled the screen itself would be the fifth place to get it wrong.
    TaskDetail {
        task_id: String,
        #[serde(default)]
        display_mode: Option<String>,
    },
    /// The quick date and time choices for a task, with the instant each one means and which
    /// one it is already set to.
    ///
    /// Resolved here rather than in the shell because the arithmetic is the part that goes wrong:
    /// a day is 23 or 25 hours across a daylight-saving boundary, an all-day date is stored
    /// differently from a timed one, and "morning" means 09:00 where the reader is rather than in
    /// UTC. Three clients read the same list in the same order — see `astrid_core::rows::due_picks`.
    DueDateOptions {
        task_id: String,
    },
    /// A task's comments.
    Comments {
        task_id: String,
    },
    /// The signed-in user.
    CurrentUser,
    /// Whether there is a stored session. Not whether it is still valid — only the server knows
    /// that, and it says so with a 401.
    IsSignedIn,
    /// The Outbox's state, for the "not synced yet" indicator.
    OutboxStats,
    /// What a pressed key means, given what is on screen.
    ///
    /// The shell asks rather than knowing, because the bare-key scheme is a cross-platform
    /// contract locked by `contracts/fixtures/shortcuts.json` — including the part that is easiest
    /// to get wrong, which is *when a key is allowed to fire at all*. A shell that dispatched from
    /// its own table would drift from web the first time somebody added a shortcut there.
    ///
    /// Pure: no cache, no network, no clock. It is safe to ask on the UI thread while a key is
    /// being handled.
    ResolveShortcut {
        key: String,
        #[serde(default)]
        has_selection: bool,
        #[serde(default)]
        is_text_field_focused: bool,
        #[serde(default)]
        is_modal_presented: bool,
    },
    /// The whole scheme, for a shortcuts sheet.
    Shortcuts,

    // ── Writes. These update the cache and journal the change. ───────────────────────────────
    CreateTask {
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        list_ids: Vec<String>,
        #[serde(default)]
        priority: Option<i64>,
        #[serde(default)]
        due_date_time: Option<String>,
        #[serde(default)]
        is_all_day: Option<bool>,
        #[serde(default)]
        assignee_id: Option<String>,
        #[serde(default)]
        parent_task_id: Option<String>,
    },
    /// Edit a task. The body is the same shape the API takes, so a field the shell learns about
    /// needs no change here — see [`crate::services::TaskChanges`] for how absent and null differ.
    UpdateTask {
        task_id: String,
        changes: serde_json::Value,
    },
    /// Complete or un-complete a task. **The only way to do either.**
    CompleteTask {
        task_id: String,
        completed: bool,
    },
    DeleteTask {
        task_id: String,
    },
    SetTaskLists {
        task_id: String,
        list_ids: Vec<String>,
    },
    SetTaskStatusRole {
        task_id: String,
        #[serde(default)]
        status_role: Option<String>,
    },
    CreateList {
        name: String,
        #[serde(default)]
        color: Option<String>,
    },
    UpdateList {
        list_id: String,
        changes: serde_json::Value,
    },
    DeleteList {
        list_id: String,
    },
    SetListFavorite {
        list_id: String,
        favorite: bool,
    },
    PostComment {
        task_id: String,
        content: String,
    },
    DeleteComment {
        comment_id: String,
    },

    // ── Things that need the network by their nature. ────────────────────────────────────────
    /// One sync pass: push, fetch, apply.
    Sync,
    /// Drain the Outbox without fetching. What a "retry now" button does.
    Drain,
    /// Refresh a task's comments from the server.
    RefreshComments {
        task_id: String,
    },
    /// Search for people to assign or invite.
    SearchUsers {
        query: String,
    },
    /// Fetch what the deployment supports.
    RefreshCapabilities,
    /// Start signing in. Answers with the URL for the shell to open in the browser.
    BeginSignIn,
    /// Finish signing in, from the URL Windows activated the app with.
    CompleteSignIn {
        callback_url: String,
    },
    /// Abandon the sign-in in progress — the user closed the browser prompt.
    CancelSignIn,
    /// Forget everything: the cache, the journal, the credential.
    SignOut,
}

impl Command {
    /// The display mode a row command asked for.
    pub fn display_mode(stored: Option<&str>) -> DisplayMode {
        DisplayMode::from_stored(stored)
    }

    /// Which surface a row command is drawing.
    ///
    /// Unrecognised means a list row, the commonest surface — and the one whose behaviour is least
    /// surprising if a newer shell asks for something this build has not heard of.
    pub fn surface(named: Option<&str>) -> Surface {
        match named {
            Some("boardCard") => Surface::BoardCard,
            Some("detail") => Surface::Detail,
            _ => Surface::ListRow,
        }
    }
}

/// What kind of failure it was. The part the shell branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// The shell sent something this build cannot read.
    BadRequest,
    /// The session is gone. Sign in again — retrying will not help.
    Unauthorized,
    /// The server considered it and said no.
    Refused,
    /// It did not reach the server. **Not necessarily a failure**: a write is already journalled
    /// and will go when the network does. The shell shows this as "offline", not as an error.
    Offline,
    /// The thing being acted on is not here.
    NotFound,
    /// The cache could not be read or written.
    Cache,
}

/// Why a command did not work.
///
/// One shape for every failure — a kind, a message, and the two optional details that some kinds
/// carry — rather than a tagged union whose payload differs per case. The shell reads this in C#,
/// where a shape that changes per variant is a `switch` over `JsonElement` at every call site.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub kind: FailureKind,
    /// For a person to read, and for a log. Never the thing to branch on.
    pub message: String,
    /// The HTTP status, when the server gave one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// What was not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl Failure {
    fn of(kind: FailureKind, message: impl Into<String>) -> Self {
        Failure {
            kind,
            message: message.into(),
            status: None,
            id: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::of(FailureKind::BadRequest, message)
    }

    pub fn unauthorized() -> Self {
        Self::of(FailureKind::Unauthorized, "the session is not valid")
    }

    pub fn refused(status: u16, message: impl Into<String>) -> Self {
        Failure {
            status: Some(status),
            ..Self::of(FailureKind::Refused, message)
        }
    }

    pub fn offline(message: impl Into<String>) -> Self {
        Self::of(FailureKind::Offline, message)
    }

    pub fn not_found(what: &str, id: impl Into<String>) -> Self {
        let id = id.into();
        Failure {
            id: Some(id.clone()),
            ..Self::of(FailureKind::NotFound, format!("no {what} with id {id}"))
        }
    }

    pub fn cache(message: impl Into<String>) -> Self {
        Self::of(FailureKind::Cache, message)
    }

    /// Whether this means "sign in again".
    pub fn needs_sign_in(&self) -> bool {
        self.kind == FailureKind::Unauthorized
    }

    /// Whether the work is still going to happen. An offline write is in the journal; showing it
    /// as a failure is how a working offline app comes to look broken.
    pub fn is_still_pending(&self) -> bool {
        self.kind == FailureKind::Offline
    }
}

impl From<crate::services::ServiceError> for Failure {
    fn from(error: crate::services::ServiceError) -> Self {
        use crate::api::ApiError;
        use crate::services::ServiceError;
        match error {
            ServiceError::Api(ApiError::Unauthorized) => Failure::unauthorized(),
            ServiceError::Api(ApiError::Http { status, message }) => {
                Failure::refused(status, message)
            }
            ServiceError::Api(ApiError::Transport(error)) => Failure::offline(error.to_string()),
            // A decode failure or a refused path: the server was reached, or would have been, and
            // trying again unchanged will not help — which is what `refused` means to the shell.
            ServiceError::Api(error) => Failure::bad_request(error.to_string()),
            ServiceError::Store(error) => Failure::cache(error.to_string()),
            ServiceError::NotFound { kind, id } => Failure::not_found(kind, id),
        }
    }
}

impl From<crate::store::StoreError> for Failure {
    fn from(error: crate::store::StoreError) -> Self {
        Failure::cache(error.to_string())
    }
}

/// What a command answered.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
}

impl Response {
    pub fn ok(value: impl Serialize) -> Self {
        Response {
            ok: true,
            value: serde_json::to_value(value).ok(),
            error: None,
        }
    }

    pub fn done() -> Self {
        Response {
            ok: true,
            value: None,
            error: None,
        }
    }

    pub fn failed(failure: Failure) -> Self {
        Response {
            ok: false,
            value: None,
            error: Some(failure),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|error| {
            // Serialising a response cannot normally fail. If it somehow does, the shell still has
            // to get an answer it can read, or it waits forever on a call that already finished.
            format!(
                "{{\"ok\":false,\"error\":{{\"kind\":\"cache\",\"0\":{}}}}}",
                serde_json::Value::String(error.to_string())
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_reads_from_the_shape_the_shell_sends() {
        let command: Command =
            serde_json::from_str(r#"{"kind":"completeTask","taskId":"t1","completed":true}"#)
                .expect("decodes");
        assert!(matches!(
            command,
            Command::CompleteTask {
                completed: true,
                ..
            }
        ));
    }

    /// The three situations the shell has to tell apart, and which look identical in a message.
    #[test]
    fn the_failures_that_mean_different_things_can_be_told_apart() {
        assert!(Failure::unauthorized().needs_sign_in());
        assert!(!Failure::unauthorized().is_still_pending());

        let offline = Failure::offline("dns");
        assert!(offline.is_still_pending());
        assert!(!offline.needs_sign_in());

        let refused = Failure::refused(422, "no");
        assert!(!refused.is_still_pending());
        assert!(!refused.needs_sign_in());
        assert_eq!(refused.status, Some(422));
    }

    #[test]
    fn a_failure_carries_its_kind_where_the_shell_can_read_it() {
        let json = Response::failed(Failure::unauthorized()).to_json();
        assert!(json.contains(r#""ok":false"#));
        assert!(json.contains(r#""kind":"unauthorized""#));
    }

    /// What was not found is named, so the shell can say which thing rather than "something".
    #[test]
    fn a_not_found_failure_names_the_thing() {
        let failure = Failure::not_found("list", "l9");
        assert_eq!(failure.id.as_deref(), Some("l9"));
        assert!(failure.message.contains("list"));
    }

    #[test]
    fn a_successful_command_with_nothing_to_return_still_says_ok() {
        let json = Response::done().to_json();
        assert_eq!(json, r#"{"ok":true}"#);
    }

    /// A surface a newer shell knows about must not turn every row into a board card.
    #[test]
    fn an_unknown_surface_is_a_list_row() {
        assert_eq!(Command::surface(None), Surface::ListRow);
        assert_eq!(Command::surface(Some("somethingLater")), Surface::ListRow);
        assert_eq!(Command::surface(Some("boardCard")), Surface::BoardCard);
        assert_eq!(Command::surface(Some("detail")), Surface::Detail);
    }
}
