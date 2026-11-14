//! The wire, as a JSON Schema a client's generator or gate can read (task 9ef7edb4).
//!
//! Until this existed, only the command **`kind`** names crossed the boundary checked — the list
//! `Command::kinds()` answers, which every shell's bindings gate reads. Everything else was
//! hand-written on each side and compared by nobody: about 6,000 lines of it, across Windows'
//! `Models.cs` and `Commands.cs`, Android's `@Serializable` DTOs, the iOS facade and web's
//! `lib/core-rules`. And the failure that produces is silent, because `serde` **ignores an unknown
//! request field**: rename `assigneeId` here and every client goes on sending the old name, which
//! arrives as `None` and writes nothing. No error, no log line, a button that half works.
//!
//! So the schema is derived from the types that ARE the wire, rather than written down beside
//! them. It cannot drift from them, because it is them.
//!
//! ## Three documents, and what each can honestly claim
//!
//! - [`command`] — the whole request vocabulary: every variant of [`crate::app::Command`], every
//!   field in each, by its wire spelling. **Complete.** This is the half the silent-default bug
//!   lives in, and a client's gate can hold every field it sends against it.
//! - [`rule`] — the same for the pure-rules door, `astrid_rules::rules::Rule`.
//! - [`wire`] — the response side: the envelope, and the models and projections a shell decodes an
//!   answer into. **Deliberately partial, and the gap is named** — see [`wire`].
//!
//! ## What is NOT in here
//!
//! `Response::value` is a `serde_json::Value`. The envelope is one shape for every command and the
//! payload differs per command, so a schema derived from `Response` describes `{ ok, value, error }`
//! and nothing about any actual answer. Where that answer is a model or a projection with a
//! `Serialize` impl, [`wire`] names the type and the shape is real. Where it is assembled by hand
//! with `serde_json::json!` — `app/dispatch/row_json.rs`, the list and board rows — there is no
//! type to derive from, and no amount of schema work here can invent one. Giving those projections
//! types is its own task (astrid-core's "Move list-row assembly into astrid-rules; return TaskRow
//! projections from both doors"); when it lands, those types join [`WireModels`] and the gap
//! closes.
//!
//! Stating that is the point. A schema that quietly covered half the wire while looking whole
//! would be worse than none, because a client's gate would pass and mean nothing.

use schemars::{schema_for, JsonSchema};
use serde_json::Value;

/// The request vocabulary: every command, every field, by its wire spelling.
pub fn command() -> Value {
    document::<crate::app::Command>()
}

/// The pure-rules door's vocabulary.
pub fn rule() -> Value {
    document::<astrid_rules::rules::Rule>()
}

/// The response side: the envelope, and every answer shape that HAS a type.
///
/// One document rather than one per type, because a client generating models from it wants the
/// whole set and the `$defs` are shared: `Task` appears inside half of them.
pub fn wire() -> Value {
    document::<WireModels>()
}

/// Every schema this core publishes, by the file name it is written to.
///
/// `cargo xtask gen-schema` writes these and `check-schema` compares them; a consumer's bindings
/// gate reads them from this function rather than from a vendored copy, so there is no second
/// artefact to go stale.
pub fn documents() -> Vec<(&'static str, Value)> {
    vec![
        ("command.json", command()),
        ("rule.json", rule()),
        ("wire.json", wire()),
    ]
}

/// One type's schema, as the JSON that is written out.
fn document<T: JsonSchema>() -> Value {
    schema_for!(T).to_value()
}

/// The answer shapes that have a Rust type, gathered so one schema document holds them all.
///
/// A field per shape, named as the schema names it. It is never built or serialised — it exists so
/// `schemars` walks all of these in one pass and emits one set of `$defs`, with `Task` and `User`
/// written once rather than once per document.
///
/// **Adding a type here is the deliberate act.** A response shape that is not listed is not in the
/// schema, and a consumer's gate will say so about every field it carries — which is the right
/// failure, because the alternative is a gate that passes on shapes nobody described.
#[derive(JsonSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct WireModels {
    // ── The envelope every answer travels in ─────────────────────────────────────────────────
    response: astrid_rules::envelope::Response,
    failure: astrid_rules::envelope::Failure,

    // ── The models, as the commands that answer "in the wire shape" return them ───────────────
    task: astrid_rules::model::Task,
    task_list: astrid_rules::model::TaskList,
    user: astrid_rules::model::User,
    comment: astrid_rules::model::Comment,
    project: astrid_rules::model::Project,
    chat_channel: astrid_rules::model::ChatChannel,
    chat_message: astrid_rules::model::ChatMessage,
    notification: astrid_rules::model::Notification,

    // ── The projections that carry a `Serialize` impl of their own ───────────────────────────
    //
    // The list and board ROWS are not here, and cannot be until they have types: see the module
    // note. These are the ones that already do.
    comment_row: astrid_rules::rows::comment::CommentRow,
    file_row: astrid_rules::rows::comment::FileRow,
    message_row: astrid_rules::rows::chat::MessageRow,
    agent_row: astrid_rules::rows::agent::AgentRow,
    credential_row: astrid_rules::rows::credential::CredentialRow,
    sidebar_sections: astrid_rules::rows::sidebar::Sections,
    repeat_summary_part: astrid_rules::rows::repeat::SummaryPart,
    repeat_preset: astrid_rules::rows::repeat::RepeatPreset,
    assignee_option: astrid_rules::rows::assignee::AssigneeOption,
    filter_group: astrid_rules::rows::filter_picks::FilterGroup,
    list_picks: astrid_rules::rows::list_picks::ListPicks,
    list_toggles: astrid_rules::rows::list_picks::ListToggles,
    list_default_picks: astrid_rules::rows::list_default_picks::ListDefaultPicks,
    reminder_pick: astrid_rules::rows::reminder_picks::ReminderPick,
    github_app: astrid_rules::rows::github_app::GithubApp,
}

/// Every property name the schema knows, anywhere in it — the question a client's bindings gate
/// asks of each name it sends or decodes.
///
/// Flat and unqualified on purpose. A gate holding `Commands.cs` against this is asking "is
/// `assigneeId` a name this core knows at all?", not "is it legal on command X" — the second needs
/// the shell's call sites mapped to variants, which is a C# parser nobody should write, and the
/// first already catches the whole class of bug this exists for: a field renamed here while a
/// client goes on sending the old spelling.
pub fn property_names(document: &Value) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    collect_properties(document, &mut names);
    names
}

fn collect_properties(node: &Value, names: &mut std::collections::BTreeSet<String>) {
    match node {
        Value::Object(fields) => {
            for (key, value) in fields {
                // `properties` and `patternProperties` are the two places a JSON Schema puts a
                // field NAME in a key position. Everything else that looks like an object here is
                // a sub-schema, and its keys are keywords.
                if key == "properties" {
                    if let Value::Object(properties) = value {
                        names.extend(properties.keys().cloned());
                    }
                }
                collect_properties(value, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_properties(item, names);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{command, documents, property_names, rule, wire};

    /// The command schema names every command, by the same wire spelling `Command::kinds()` gives.
    ///
    /// The two lists come from the same enum by different routes — `strum`'s variant names through
    /// `kinds()`, `serde`'s tag through the `schemars` derive — so agreeing is the claim. If they
    /// ever differ, one of them is not seeing a variant, and a shell's gate would be reading its
    /// vocabulary against a list the core does not accept.
    #[test]
    fn the_command_schema_names_every_kind_9ef7edb4() {
        let document = command();
        let text = document.to_string();
        for kind in crate::app::command_kinds() {
            assert!(
                text.contains(&format!("\"{kind}\"")),
                "the schema does not name the command {kind:?}"
            );
        }
    }

    /// A field's WIRE spelling is what the schema carries, not Rust's.
    ///
    /// The whole value of this document to a client is that `assignee_id` appears in it as
    /// `assigneeId` — the name a shell actually sends. `rename_all_fields = "camelCase"` on the
    /// enum is what does it, and a refactor that dropped it would leave a schema every client
    /// failed against for a reason nobody could see.
    #[test]
    fn fields_carry_their_wire_spelling_9ef7edb4() {
        let names = property_names(&command());
        assert!(names.contains("assigneeId"), "assigneeId should be a name");
        assert!(names.contains("listId"), "listId should be a name");
        assert!(
            !names.contains("assignee_id"),
            "the schema carries wire spellings, not Rust's: {:?}",
            names
                .iter()
                .filter(|name| name.contains('_'))
                .collect::<Vec<_>>()
        );
    }

    /// An instant is a string with a date-time format, not whatever `#[serde(with = "…")]`'s
    /// module path would have produced.
    ///
    /// `schemars` reads serde's attributes, and `with = "date::optional"` names a MODULE — handed
    /// to it as a type it does not compile. Every such field says what it is instead
    /// (`date::Timestamp`), and this is the half that proves the answer is right rather than
    /// merely compiling.
    #[test]
    fn an_instant_is_a_date_time_string_9ef7edb4() {
        let text = wire().to_string();
        assert!(
            text.contains("\"format\":\"date-time\""),
            "a wire instant should be a date-time string"
        );
    }

    /// The rules door has its own vocabulary, and it is not the command one.
    #[test]
    fn the_rule_schema_is_the_rules_door_9ef7edb4() {
        let text = rule().to_string();
        assert!(
            text.contains("\"completion\""),
            "a rule kind should be named"
        );
        assert!(
            text.contains("\"nextOccurrence\""),
            "and so should this one"
        );
    }

    /// Every document is non-trivial and has a `$defs` to generate from. A schema that quietly
    /// came out as `true` (which is a legal JSON Schema meaning "anything") would pass every gate
    /// built on it while checking nothing.
    #[test]
    fn every_document_describes_something_9ef7edb4() {
        for (name, document) in documents() {
            assert!(
                document.is_object(),
                "{name} is not an object, so it constrains nothing"
            );
            assert!(
                document.get("$defs").is_some(),
                "{name} has no $defs to generate types from"
            );
            assert!(
                property_names(&document).len() > 10,
                "{name} names only {} properties; the derive has stopped seeing fields",
                property_names(&document).len()
            );
        }
    }

    /// The gap is stated rather than papered over: `Response::value` is untyped, and the schema
    /// says so by describing it as anything at all.
    #[test]
    fn the_envelopes_payload_is_untyped_9ef7edb4() {
        let names = property_names(&wire());
        assert!(names.contains("ok") && names.contains("value") && names.contains("error"));
        // And the models the answer is decoded INTO are named, which is what makes the document
        // useful despite that.
        assert!(
            names.contains("dueDateTime"),
            "Task's fields should be in it"
        );
    }
}
