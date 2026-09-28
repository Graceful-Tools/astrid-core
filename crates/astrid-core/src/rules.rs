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

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
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
        /// The person's zone, by IANA name (`America/Los_Angeles`).
        time_zone: String,
    },
    /// The calculator on its own: where a series goes next from these fields — see
    /// [`crate::repeating::next_occurrence`]. `completion` is used as given; the all-day
    /// adjustment to the person's own day is [`Rule::Completion`]'s, not this rule's.
    NextOccurrence {
        /// `daily`, `weekly`, `monthly`, `yearly` or `custom`, as the wire spells it.
        repeating: String,
        /// The custom pattern, or a simple pattern's end condition, as the wire stores it.
        #[serde(default)]
        pattern: Option<serde_json::Value>,
        #[serde(default, with = "date::optional")]
        current_due_date: Option<DateTime<Utc>>,
        #[serde(with = "date::required")]
        completion: DateTime<Utc>,
        /// `DUE_DATE` or `COMPLETION_DATE`; the wire default is the latter.
        #[serde(default)]
        repeat_from: Option<String>,
        #[serde(default)]
        occurrence_count: i64,
        /// The zone whose calendar the steps are taken on, by IANA name. UTC when absent — the
        /// web's own answer.
        #[serde(default)]
        time_zone: Option<String>,
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
            time_zone,
        } => match zone(&time_zone) {
            Ok(zone) => Response::ok(crate::repeating::completion(&task, completed, now, zone)),
            Err(failure) => Response::failed(failure),
        },
        Rule::NextOccurrence {
            repeating,
            pattern,
            current_due_date,
            completion,
            repeat_from,
            occurrence_count,
            time_zone,
        } => {
            let zone = match zone(time_zone.as_deref().unwrap_or("UTC")) {
                Ok(zone) => zone,
                Err(failure) => return Response::failed(failure),
            };
            // The same fields a task carries, read the way a task is read — so this rule and
            // `Completion` cannot interpret a pattern differently.
            let task = serde_json::from_value::<Task>(serde_json::json!({
                "id": "rule",
                "repeating": repeating,
                "repeatingData": pattern,
                "dueDateTime": current_due_date.map(date::format),
                "isAllDay": false,
                "repeatFrom": repeat_from,
                "occurrenceCount": occurrence_count,
            }));
            match task {
                Ok(task) => {
                    Response::ok(crate::repeating::next_occurrence(&task, completion, zone))
                }
                Err(error) => Response::failed(Failure::bad_request(error.to_string())),
            }
        }
        Rule::RenderMarkdown { text } => Response::ok(crate::markdown::render(&text)),
    }
}

/// A zone by its IANA name. An unknown name is refused rather than read as UTC: a person in Tokyo
/// told their Tuesday task is due on Monday is a worse failure than an error in a log.
fn zone(name: &str) -> Result<Tz, Failure> {
    name.parse::<Tz>()
        .map_err(|_| Failure::bad_request(format!("{name} is not a time zone this build knows")))
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
            "timeZone": "UTC"
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
    fn the_calculator_answers_on_its_own() {
        let reply = answer(json!({
            "kind": "nextOccurrence",
            "repeating": "custom",
            "pattern": { "type": "custom", "unit": "weeks", "interval": 1,
                         "weekdays": ["monday", "wednesday", "friday"] },
            "currentDueDate": "2026-09-28T09:00:00Z",
            "completion": "2026-09-28T12:00:00Z",
            "repeatFrom": "DUE_DATE",
            "occurrenceCount": 4
        }));
        assert_eq!(
            reply,
            json!({ "ok": true, "value": {
                "nextDueDate": "2026-09-30T09:00:00Z",
                "shouldTerminate": false,
                "newOccurrenceCount": 5
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
    fn an_unknown_zone_is_refused_rather_than_read_as_utc() {
        let reply = answer(json!({
            "kind": "completion", "task": { "id": "t1" }, "completed": true,
            "now": "2026-09-28T12:00:00Z", "timeZone": "Mars/Olympus_Mons"
        }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "badRequest");
    }
}
