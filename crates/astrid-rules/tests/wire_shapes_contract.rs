//! Web's own payloads decode through this crate's models with no key lost and none invented.
//!
//! `contracts/fixtures/wire-shapes.json` is not a description of the wire: astrid-web's
//! `scripts/contract-fixtures/drivers/wire-shapes.mjs` runs fixed records through each `/api/v1`
//! route's own Prisma select/include and its own serializer, so the `body` of a surface is what the
//! server actually sends. Six surfaces cover the shapes that differ: a list page, the same page
//! with `?omit=`, a task read, a task SSE event, a list read, a list SSE event. The two event
//! surfaces are GitHub-backed, which is the only place `backend`, `remote`, `supports`,
//! `syncState` and `fields` appear at all.
//!
//! What this test asserts, per surface and per object, is narrow and mechanical:
//!
//! 1. **Nothing lost.** Every key present on the wire is present after a decode-and-re-encode
//!    round trip, with an equal value.
//! 2. **Nothing invented.** No key absent on the wire appears afterwards.
//!
//! That is the failure class behind D53–D55 and AITD-499, and each of those got through because
//! the only check was that decoding did not throw: `#[serde(default)] description: String` turns an
//! omitted description into `""`, which is indistinguishable from a cleared one; `syncState`
//! absent and `syncState: null` mean different things to a shell drawing a pending badge; and
//! `backend`/`supports` were dropped by the cache shape, so no client could gate on them.
//!
//! A key the models deliberately do not carry goes in [`ALLOWLIST`] **with its reason**. An entry
//! without a reason it can defend is a loosened test, and the point of this one is that it is not.

mod common;

use std::collections::BTreeSet;

use astrid_rules::model::{Task, TaskList};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/wire-shapes.json");

/// Which kind of object a surface carries, and where in `body` it sits.
enum Shape {
    /// `body` → the pointer → one object.
    One(&'static str),
    /// `body` → the pointer → an array of objects.
    Many(&'static str),
}

enum Kind {
    Task,
    List,
}

/// The surfaces, in the fixture's own order. A surface the fixture grows and this table does not
/// know is a failure of its own, below: a payload nobody round-trips is how D55 survived.
const SURFACES: &[(&str, Kind, Shape)] = &[
    ("task.listPage", Kind::Task, Shape::Many("/tasks")),
    ("task.listPage.omitted", Kind::Task, Shape::Many("/tasks")),
    ("task.read", Kind::Task, Shape::One("/task")),
    ("task.event", Kind::Task, Shape::One("/v1Task")),
    ("list.read", Kind::List, Shape::One("/list")),
    ("list.event", Kind::List, Shape::One("/v1List")),
];

/// A key on the wire this crate deliberately does not carry, scoped to the surface that sends it.
///
/// Paths are JSON pointers with every array index written `[]`, so one entry covers every element
/// of a collection. A prefix match is deliberate: an entry for `/lists[]/listMembers` also excuses
/// everything under it, because a key the model does not carry cannot have readable children.
/// `"*"` as the surface means every surface.
///
/// **Adding to this list is a decision, not a formality.** The reason has to say why a native shell
/// never needs the key, or name the task that will carry it — not that the round trip was
/// inconvenient. Scoping by surface is what keeps that honest: `description` is excused on the
/// `?omit=` surface only, so the surfaces that DO send it are still checked.
const ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        "*",
        "/sequence",
        "The server's optimistic-concurrency counter. Astrid's clients reconcile through the \
         Outbox and `updatedAt`, never by comparing sequences, so no shell has anything to do \
         with it. Modelling it would put a number in the cache that nothing reads.",
    ),
    (
        "task.event",
        "/statusRoleBeforeDone",
        "Which column an un-completed task returns to. The server decides that and sends the \
         result as `statusRole`, which IS modelled and IS what the board draws; a client that \
         kept its own copy would be a second opinion about a move it does not make.",
    ),
    (
        "task.listPage.omitted",
        "/description",
        "D55, and the finding this test was written to produce: `Task::description` is \
         `#[serde(default)] String`, so a description the server OMITTED (`?omit=description`) \
         decodes to `\"\"` — indistinguishable from one a person cleared. Writing it back would \
         erase the real description. The fix is `Option<String>` and it reaches every reader of \
         the field, so it is its own task: ebe0966b on the Astrid Windows board. Excused HERE \
         ONLY: the surfaces that do send a description still assert it survives.",
    ),
    (
        "list.event",
        "/recentlyCompletedWindow",
        "This surface sends `{ \"amount\": 3, \"unit\": \"days\" }` — no `kind`, and a unit no \
         version of `lib/recently-completed-window.ts` describes. The column is unvalidated JSON \
         that `v1-list-shape.ts` passes straight through, so reading an unreadable value as \"the \
         legacy 24-hour default\" is the right answer and dropping the key is that answer being \
         honest. It stopped being a decode FAILURE in this task; see \
         `deserialize_recently_completed_window`.",
    ),
    (
        "list.event",
        "/remote",
        "Where a GitHub-backed list lives (`url`, `owner`, `number`). `model::Task` has \
         `TaskRemote`; `TaskList` has no counterpart yet, so the chip that would show `acme/7` \
         has no data. Needed by the Windows supports work (93ffa9ea) and filed with `fields` as \
         task faa32e20.",
    ),
    (
        "*",
        "/fields",
        "A GitHub Project's own field values, keyed by `ProjectField.id` (web AWTD-1208). \
         Modelling them needs the schema route `GET /api/v1/projects/:id/fields` as well as the \
         values, which is more than a struct. Filed as task faa32e20 with `/remote`.",
    ),
];

fn main_fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("wire-shapes.json parses")
}

fn allowed(surface: &str, path: &str) -> bool {
    ALLOWLIST.iter().any(|(scope, prefix, _)| {
        (*scope == "*" || *scope == surface)
            && (path == *prefix || path.starts_with(&format!("{prefix}/")))
    })
}

/// Re-encode what decoded, so the two sides of the comparison are both `serde_json::Value`.
fn round_trip<T: serde::de::DeserializeOwned + Serialize>(
    wire: &Value,
    where_: &str,
    failures: &mut Vec<String>,
) -> Option<Value> {
    match serde_json::from_value::<T>(wire.clone()) {
        Ok(decoded) => match serde_json::to_value(&decoded) {
            Ok(value) => Some(value),
            Err(error) => {
                failures.push(format!("{where_}: re-encoding failed: {error}"));
                None
            }
        },
        Err(error) => {
            // A row this crate cannot read at all is the worst case, not an excused one: the API
            // client decodes collections leniently, so in production this row simply vanishes.
            failures.push(format!("{where_}: did not decode: {error}"));
            None
        }
    }
}

/// Two ways the round trip rewrites a value without losing anything, and why neither is a finding.
///
/// Everything else is. These two are stated here rather than in [`ALLOWLIST`] because they are
/// properties of the encoding, not keys the models decline to carry — allowlisting a path would
/// stop checking that path's *value*, which is the opposite of what is wanted.
///
/// 1. **`null` on the wire ↔ absent after the round trip.** Every optional field is
///    `skip_serializing_if = "Option::is_none"`, and the server itself uses the two
///    interchangeably — `lib/lists/v1-list-shape.ts` writes `?? null` where the task shapers omit.
///    `None` carries both, so nothing is lost. What IS a finding, and still fails below, is a
///    non-null value becoming absent.
/// 2. **An RFC 3339 instant re-serialised without its milliseconds.** `"…T09:00:00.000Z"` decodes
///    to a `DateTime<Utc>` and comes back `"…T09:00:00Z"`. The same instant, so compared as
///    instants. A timestamp that moves fails.
fn equivalent(wire: &Value, ours: &Value) -> bool {
    if wire == ours {
        return true;
    }
    match (wire.as_str(), ours.as_str()) {
        (Some(left), Some(right)) => match (instant(left), instant(right)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        },
        _ => false,
    }
}

fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Walk the wire value and the round-tripped value together, reporting by path.
fn compare(
    wire: &Value,
    ours: &Value,
    surface: &str,
    path: &str,
    where_: &str,
    failures: &mut Vec<String>,
) {
    if allowed(surface, path) {
        return;
    }
    match (wire, ours) {
        (Value::Object(wire_keys), Value::Object(our_keys)) => {
            let names: BTreeSet<&String> = wire_keys.keys().chain(our_keys.keys()).collect();
            for name in names {
                let child = format!("{path}/{name}");
                match (wire_keys.get(name), our_keys.get(name)) {
                    (Some(wire_child), Some(our_child)) => {
                        compare(wire_child, our_child, surface, &child, where_, failures)
                    }
                    (Some(wire_child), None) => {
                        if !allowed(surface, &child) && !wire_child.is_null() {
                            failures.push(format!(
                                "{where_}: LOST {child} — on the wire as {}, absent after the \
                                 round trip",
                                summarise(wire_child)
                            ));
                        }
                    }
                    (None, Some(our_child)) => {
                        if !allowed(surface, &child) && !our_child.is_null() {
                            failures.push(format!(
                                "{where_}: INVENTED {child} — absent on the wire, {} after the \
                                 round trip",
                                summarise(our_child)
                            ));
                        }
                    }
                    (None, None) => unreachable!("the name came from one of the two maps"),
                }
            }
        }
        (Value::Array(wire_items), Value::Array(our_items)) => {
            if wire_items.len() != our_items.len() {
                failures.push(format!(
                    "{where_}: {path} has {} element(s) on the wire and {} after the round trip",
                    wire_items.len(),
                    our_items.len()
                ));
                return;
            }
            for (wire_item, our_item) in wire_items.iter().zip(our_items) {
                compare(
                    wire_item,
                    our_item,
                    surface,
                    &format!("{path}[]"),
                    where_,
                    failures,
                );
            }
        }
        (wire_leaf, our_leaf) if !equivalent(wire_leaf, our_leaf) => failures.push(format!(
            "{where_}: CHANGED {path} — {} on the wire, {} after the round trip",
            summarise(wire_leaf),
            summarise(our_leaf)
        )),
        _ => {}
    }
}

/// Enough of a value to recognise it in a failure, without pasting a task body into a test report.
fn summarise(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Object(map) => format!("an object of {} key(s)", map.len()),
        Value::Array(items) => format!("an array of {} element(s)", items.len()),
        Value::String(text) if text.chars().count() > 40 => {
            format!("{:?}…", text.chars().take(40).collect::<String>())
        }
        other => other.to_string(),
    }
}

fn objects<'a>(body: &'a Value, shape: &Shape, name: &str) -> Vec<(String, &'a Value)> {
    match shape {
        Shape::One(pointer) => {
            let value = body
                .pointer(pointer)
                .unwrap_or_else(|| panic!("{name}: body{pointer} is missing"));
            vec![(format!("{name} body{pointer}"), value)]
        }
        Shape::Many(pointer) => {
            let array = body
                .pointer(pointer)
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("{name}: body{pointer} is not an array"));
            assert!(!array.is_empty(), "{name}: body{pointer} is empty");
            array
                .iter()
                .enumerate()
                .map(|(index, value)| (format!("{name} body{pointer}[{index}]"), value))
                .collect()
        }
    }
}

/// AWTD-1263 step 2, task f8dfc84e: every surface web records round-trips through this crate's
/// models without losing a key or inventing one.
#[test]
fn f8dfc84e_every_wire_surface_round_trips_with_no_key_lost_and_none_invented() {
    let fixture = main_fixture();
    let surfaces = fixture["surfaces"]
        .as_array()
        .expect("surfaces is an array");
    let mut failures = Vec::new();

    for (name, kind, shape) in SURFACES {
        let surface = surfaces
            .iter()
            .find(|surface| surface["name"] == Value::String((*name).to_string()))
            .unwrap_or_else(|| panic!("the fixture has no surface named {name}"));
        for (where_, wire) in objects(&surface["body"], shape, name) {
            let ours = match kind {
                Kind::Task => round_trip::<Task>(wire, &where_, &mut failures),
                Kind::List => round_trip::<TaskList>(wire, &where_, &mut failures),
            };
            if let Some(ours) = ours {
                compare(wire, &ours, name, "", &where_, &mut failures);
            }
        }
    }

    common::no_failures("wire-shapes.json", &failures);
}

/// A surface the fixture gains and [`SURFACES`] does not know would be silently unchecked, which
/// is how D55 reached three clients: the payload existed, nothing decoded it.
#[test]
fn f8dfc84e_every_surface_in_the_fixture_is_covered_by_this_test() {
    let fixture = main_fixture();
    let known: BTreeSet<&str> = SURFACES.iter().map(|(name, _, _)| *name).collect();
    let present: BTreeSet<&str> = fixture["surfaces"]
        .as_array()
        .expect("surfaces is an array")
        .iter()
        .map(|surface| surface["name"].as_str().expect("a surface name"))
        .collect();
    assert_eq!(
        present, known,
        "wire-shapes.json and this test's SURFACES table disagree: add the new surface to the \
         table (and say where its model object sits in `body`), or take the stale one out"
    );
}

/// Every allowlist entry carries a reason, and no entry outlives the key it excuses: a path the
/// fixture no longer sends is an exclusion nobody is checking.
#[test]
fn f8dfc84e_every_allowlisted_path_is_explained_and_still_on_the_wire() {
    let fixture = main_fixture();
    let surfaces = fixture["surfaces"]
        .as_array()
        .expect("surfaces is an array");
    let mut on_the_wire: BTreeSet<String> = BTreeSet::new();
    for (name, _, shape) in SURFACES {
        let surface = surfaces
            .iter()
            .find(|surface| surface["name"] == Value::String((*name).to_string()))
            .unwrap_or_else(|| panic!("the fixture has no surface named {name}"));
        for (_, wire) in objects(&surface["body"], shape, name) {
            collect_paths(wire, "", &mut on_the_wire);
        }
    }

    let mut failures = Vec::new();
    for (scope, path, reason) in ALLOWLIST {
        if reason.trim().is_empty() {
            failures.push(format!("{scope} {path} is allowlisted with no reason"));
        }
        // Any surface, not the scoped one: `/description` is excused on `task.listPage.omitted`
        // precisely BECAUSE that surface omits it, and the entry is still earning its keep as
        // long as the key is part of the contract somewhere.
        if !on_the_wire.contains(*path) {
            failures.push(format!(
                "{path} is allowlisted ({scope}) but no surface in the fixture carries that key \
                 any more — delete the entry"
            ));
        }
    }
    common::no_failures("wire-shapes.json allowlist", &failures);
}

fn collect_paths(value: &Value, path: &str, into: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                let child_path = format!("{path}/{name}");
                into.insert(child_path.clone());
                collect_paths(child, &child_path, into);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_paths(item, &format!("{path}[]"), into);
            }
        }
        _ => {}
    }
}
