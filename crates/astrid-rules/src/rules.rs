//! The stateless door: the pure contracts, answered synchronously.
//!
//! `astrid_core::app::App::run_json` is the door to a running client — a cache, an Outbox, a network —
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
use serde::{Deserialize, Serialize};

use crate::envelope::{Failure, Response};
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
    /// [`crate::repeating::next_occurrence`], which this answers exactly, all-day handling
    /// included.
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
        /// An all-day series steps in UTC whatever the zone: its dates are UTC midnights.
        #[serde(default)]
        is_all_day: bool,
    },
    /// Everything one person may do with one list (and with a task in it by `task_creator_id`) —
    /// see [`crate::permissions`]. `list` is the list's wire shape; nobody signed in has no access.
    ListAccess {
        list: crate::permissions::ListAccess,
        #[serde(default)]
        user_id: Option<String>,
        #[serde(default)]
        task_creator_id: Option<String>,
    },
    /// What a quick-add line says — its title, `#lists`, date word, repeat and priority — see
    /// [`crate::parse::smart`]. `today` is the person's calendar day; a date word answers as a
    /// calendar day too (`dueDay`), which a shell stores the way it stores any all-day date.
    SmartParse {
        text: String,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        /// The app's language (`en`, `pt-BR`, …): its keyword tables. English when unknown.
        #[serde(default)]
        locale: Option<String>,
        today: chrono::NaiveDate,
    },
    /// What a search box asks for — `assignee:me priority:high overdue rollover` as a structured
    /// filter plus the free text — see [`crate::parse::search`]. Answers the parse in the web's
    /// field names with `isEmpty` beside it: whether the query asks for anything at all.
    SearchParse { query: String },
    /// A description, comment or chat message as blocks to draw — see [`crate::markdown`].
    RenderMarkdown { text: String },
}

/// The answer to [`Rule::SearchParse`]: the parse, and whether it asks for anything.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParse {
    #[serde(flatten)]
    pub query: crate::parse::search::SearchQuery,
    pub is_empty: bool,
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
            is_all_day,
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
                "isAllDay": is_all_day,
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
        Rule::ListAccess {
            list,
            user_id,
            task_creator_id,
        } => Response::ok(match user_id {
            Some(user) => crate::permissions::Access::of(&user, task_creator_id.as_deref(), &list),
            None => crate::permissions::Access::NONE,
        }),
        Rule::SmartParse {
            text,
            lists,
            locale,
            today,
        } => Response::ok(crate::parse::smart::parse(
            &text,
            &lists,
            crate::parse::smart::Keywords::for_locale(locale.as_deref().unwrap_or("en")),
            today,
        )),
        Rule::SearchParse { query } => {
            let query = crate::parse::search::parse(&query);
            let is_empty = query.is_empty();
            Response::ok(SearchParse { query, is_empty })
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
            "occurrenceCount": 4,
            "timeZone": "America/Los_Angeles"
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

    /// An all-day series steps on UTC midnights even when the person is in California, where the
    /// completion instant is already the next UTC day.
    #[test]
    fn an_all_day_series_steps_in_utc_whatever_the_zone() {
        let reply = answer(json!({
            "kind": "nextOccurrence",
            "repeating": "daily",
            "currentDueDate": "2026-01-06T00:00:00Z",
            "completion": "2026-01-06T14:42:00Z",
            "repeatFrom": "COMPLETION_DATE",
            "timeZone": "America/Los_Angeles",
            "isAllDay": true
        }));
        assert_eq!(reply["value"]["nextDueDate"], "2026-01-07T00:00:00Z");
    }

    /// D6: an uppercase role written by an old endpoint is still that role.
    #[test]
    fn list_access_answers_every_question_at_once() {
        let reply = answer(json!({
            "kind": "listAccess",
            "list": { "id": "l1", "ownerId": "owner", "privacy": "PRIVATE",
                      "listMembers": [{ "userId": "u2", "role": "ADMIN" }] },
            "userId": "u2"
        }));
        assert_eq!(reply["value"]["role"], "admin");
        assert_eq!(reply["value"]["canManage"], true);
        assert_eq!(reply["value"]["canDelete"], false);
    }

    /// AWTD-1061: the server sends the project; the core resolves the role from it.
    #[test]
    fn list_access_reads_the_project_when_the_server_sends_it() {
        let reply = answer(json!({
            "kind": "listAccess",
            "list": { "ownerId": "owner", "privacy": "PRIVATE", "listType": "status",
                      "project": { "ownerId": "boss", "members": [],
                                   "lists": [{ "id": "d", "listMembers": [{ "userId": "u2" }] }] } },
            "userId": "u2"
        }));
        assert_eq!(reply["value"]["role"], "member");
        assert_eq!(reply["value"]["canEditTasks"], true);
        assert_eq!(reply["value"]["canManage"], false);
    }

    #[test]
    fn nobody_signed_in_has_no_access() {
        let reply = answer(json!({
            "kind": "listAccess",
            "list": { "id": "l1", "ownerId": "owner", "privacy": "PUBLIC" }
        }));
        assert_eq!(reply["value"]["role"], serde_json::Value::Null);
        assert_eq!(reply["value"]["canView"], false);
    }

    #[test]
    fn a_search_box_query_parses_to_its_filter_and_text() {
        let reply = answer(json!({
            "kind": "searchParse",
            "query": "assignee:@sam priority:urgent list:\"Bugs and Polish\" overdue AST-142"
        }));
        assert_eq!(reply["ok"], true);
        let value = &reply["value"];
        assert_eq!(value["text"], "overdue");
        assert_eq!(value["assignee"], "sam");
        assert_eq!(value["priorities"], json!(["high"]));
        assert_eq!(value["listNames"], json!(["Bugs and Polish"]));
        assert_eq!(value["identifier"], "AST-142");
        assert_eq!(value["due"], serde_json::Value::Null);
        assert_eq!(value["isEmpty"], false);
    }

    #[test]
    fn an_empty_search_asks_for_nothing() {
        let reply = answer(json!({ "kind": "searchParse", "query": "   " }));
        assert_eq!(reply["value"]["text"], "");
        assert_eq!(reply["value"]["isEmpty"], true);
    }

    #[test]
    fn a_quick_add_line_parses_to_its_parts() {
        let reply = answer(json!({
            "kind": "smartParse",
            "text": "Call mum tomorrow #family urgent",
            "lists": [{ "id": "f", "name": "Family" }],
            "today": "2026-09-28"
        }));
        assert_eq!(reply["value"]["title"], "Call mum");
        assert_eq!(reply["value"]["listIds"], json!(["f"]));
        assert_eq!(reply["value"]["dueDay"], "2026-09-29");
        assert_eq!(reply["value"]["priority"], 3);
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
