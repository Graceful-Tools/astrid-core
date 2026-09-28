//! The stateless door: the pure contracts, answered synchronously.
//!
//! [`crate::app::App::run_json`] is the door to a running client — a cache, an Outbox, a network —
//! and it is async because most of what goes through it waits on something. Some rules wait on
//! nothing: what completing a task does, how a description renders, who may edit a list. A view
//! asks those while it draws, and a shell that has to await a future to draw a row is a shell
//! that draws it late.
//!
//! So those rules have a second door, with the same shape as the first — one function, JSON in,
//! JSON out, the same [`Response`] envelope — and no state at all. Nothing here reads a cache or a
//! clock: whatever a rule depends on ("now", the device's offset from UTC) arrives in the request,
//! which is what makes every answer reproducible from the request alone.
//!
//! Inputs are the **API wire shape** (`/api/v1`), the one every client already speaks and this
//! crate's models already decode leniently. A shell that keeps its own models hands them over as
//! it would to the server; nothing is mirrored field by field across the boundary.

use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;

use crate::app::{Failure, Response};
use crate::model::{date, Task};

/// One question for a pure rule.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Rule {
    /// What marking `task` completed (or not) at `now` does to it — see
    /// [`crate::repeating::completion`]. `task` is the task as the person sees it.
    Completion {
        task: Box<Task>,
        completed: bool,
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The device's offset from UTC, in minutes east (California in summer is `-420`).
        utc_offset_minutes: i32,
    },
    /// A description, comment or chat message as blocks to draw — see [`crate::markdown`].
    RenderMarkdown { text: String },
}

/// Answer one rule.
pub fn run(rule: Rule) -> Response {
    match rule {
        Rule::Completion {
            task,
            completed,
            now,
            utc_offset_minutes,
        } => match FixedOffset::east_opt(utc_offset_minutes * 60) {
            Some(offset) => {
                Response::ok(crate::repeating::completion(&task, completed, now, offset))
            }
            None => Response::failed(Failure::bad_request(format!(
                "{utc_offset_minutes} minutes is not an offset from UTC"
            ))),
        },
        Rule::RenderMarkdown { text } => Response::ok(crate::markdown::render(&text)),
    }
}

/// Answer one rule given as JSON, as JSON. What a binding calls.
pub fn run_json(request: &str) -> String {
    match serde_json::from_str::<Rule>(request) {
        Ok(rule) => run(rule).to_json(),
        // A request the core cannot read is a bug in the shell, and it has to say so rather than
        // answer with a default that looks like a result.
        Err(error) => Response::failed(Failure::bad_request(error.to_string())).to_json(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn answer(request: Value) -> Value {
        serde_json::from_str(&run_json(&request.to_string())).expect("the door answers JSON")
    }

    #[test]
    fn completion_answers_in_the_wire_shape_the_shell_sent() {
        let reply = answer(json!({
            "kind": "completion",
            "task": {
                "id": "t1", "repeating": "weekly", "repeatFrom": "DUE_DATE",
                "dueDateTime": "2026-09-28T09:00:00Z", "isAllDay": false
            },
            "completed": true,
            "now": "2026-09-28T12:00:00Z",
            "utcOffsetMinutes": 0
        }));
        assert_eq!(
            reply,
            json!({ "ok": true, "value": {
                "outcome": "rollForward",
                "dueDateTime": "2026-10-05T09:00:00Z",
                "isAllDay": false,
                "occurrenceCount": 1
            }})
        );
    }

    #[test]
    fn markdown_renders_to_blocks() {
        let reply = answer(json!({ "kind": "renderMarkdown", "text": "**hi**" }));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["value"][0]["kind"], "paragraph");
        assert_eq!(reply["value"][0]["inlines"][0]["bold"], true);
    }

    #[test]
    fn a_rule_the_core_does_not_know_says_so() {
        let reply = answer(json!({ "kind": "somethingLater" }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "badRequest");
    }

    #[test]
    fn an_impossible_offset_is_refused_rather_than_read_as_utc() {
        let reply = answer(json!({
            "kind": "completion", "task": { "id": "t1" }, "completed": true,
            "now": "2026-09-28T12:00:00Z", "utcOffsetMinutes": 100_000
        }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "badRequest");
    }
}
