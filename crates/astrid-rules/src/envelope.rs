//! The envelope every answer travels in — `{ ok, value?, error? }` — shared by both doors.
//!
//! [`crate::rules::run_json`] answers in it, and so does astrid-core's `App::run_json`, which
//! re-exports these types from `astrid_core::app`. One shape, so a shell reads both doors with one
//! decoder. Changing it is a wire change for every client.

use serde::Serialize;

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
