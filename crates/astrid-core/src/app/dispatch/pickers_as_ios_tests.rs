//! The pickers answer as iOS's pickers did (AITD-461), including for an editor holding values the
//! cache has not got yet — a task not created, lists not saved, people its search found.
//!
//! `docs/CONTRACTS.md` D47 (assignees), D48 (due picks), D49 (repeat keys), D50 (list picks).

use super::super::tests::app_with;
use crate::api::StubTransport;
use crate::platform::FixedClock;
use serde_json::{json, Value};

async fn call(app: &super::App, command: Value) -> Value {
    serde_json::from_str(&app.run_json(&command.to_string()).await).expect("valid JSON")
}

async fn seeded(seed: Value) -> super::App {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    let mut command = seed;
    command["kind"] = json!("seedCache");
    let seeded = call(&app, command).await;
    assert_eq!(seeded["value"]["seeded"], true, "{seeded}");
    app
}

fn option_ids(answer: &Value) -> Vec<Option<String>> {
    answer["value"]["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|option| option["userId"].as_str().map(str::to_string))
        .collect()
}

/// D47: the shell's inputs win — its lists, the people its search found, its agents — and a
/// task not created yet can still be asked about.
#[tokio::test]
async fn aitd461_assignee_options_take_the_editors_inputs() {
    let app = seeded(json!({
        "tasks": [{ "id": "seed", "title": "seed" }],
        "lists": [
            { "id": "l1", "name": "Work",
              "owner": { "id": "me", "name": "Jon" },
              "listMembers": [
                  { "userId": "zoe", "role": "member", "user": { "id": "zoe", "name": "Zoe" } },
                  { "userId": "ghost", "role": "member" },
              ] },
            { "id": "l2", "name": "Empty" },
        ],
    }))
    .await;

    let answer = call(
        &app,
        json!({
            "kind": "assigneeOptions",
            "listIds": ["l1"],
            "discovered": [{ "id": "amy", "name": "amy" }],
            "agents": [{ "id": "agent-claude", "name": "Claude", "isAIAgent": true }],
        }),
    )
    .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(
        option_ids(&answer),
        [
            None,
            Some("agent-claude".into()),
            Some("me".into()),
            Some("zoe".into()),
            Some("amy".into()),
        ],
        "unassigned, agents, you, then names as written; the unhydrated member is left out"
    );
    assert_eq!(answer["value"]["options"][3]["user"]["name"], "Zoe");

    // A list that resolves but names nobody still offers you (AITD-413).
    let answer = call(
        &app,
        json!({ "kind": "assigneeOptions", "listIds": ["l2"] }),
    )
    .await;
    assert_eq!(option_ids(&answer), [None, Some("me".into())]);
}

/// D50: the Apple pickers' checklist — every list a task can be filed in, its own marked, in the
/// sidebar's order, no cap and no create; a search matches anywhere, without regard to case.
#[tokio::test]
async fn aitd461_list_picks_as_toggles() {
    let mut lists = vec![
        json!({ "id": "fav", "name": "zebra", "isFavorite": true }),
        json!({ "id": "virtual", "name": "Today", "isVirtual": true }),
    ];
    for n in 0..12 {
        lists.push(json!({ "id": format!("l{n:02}"), "name": format!("List {n:02}") }));
    }
    let app = seeded(json!({ "tasks": [{ "id": "seed", "title": "seed" }], "lists": lists })).await;

    let answer = call(
        &app,
        json!({ "kind": "listPicks", "listIds": ["l03"], "asToggles": true }),
    )
    .await;
    assert_eq!(answer["ok"], true, "{answer}");
    let options = answer["value"]["options"].as_array().expect("options");
    assert_eq!(
        options.len(),
        13,
        "every destination, no cap, no virtual list"
    );
    assert_eq!(options[0]["id"], "fav", "favourites first");
    assert_eq!(options[4]["id"], "l03");
    assert_eq!(options[4]["isSelected"], true);
    assert_eq!(answer["value"]["selected"][0]["id"], "l03");
    assert!(answer["value"].get("createName").is_none());

    let found = call(
        &app,
        json!({ "kind": "listPicks", "listIds": [], "asToggles": true, "query": "IST 1" }),
    )
    .await;
    let ids: Vec<&str> = found["value"]["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|option| option["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, ["l10", "l11"]);
}

/// D48: a date pick is the reader's day as an all-day date at UTC midnight, even for a timed task
/// — iOS's quick date. The clock is 2026-09-07T12:00Z.
#[tokio::test]
async fn aitd461_a_date_pick_is_an_all_day_date() {
    let app = seeded(json!({
        "tasks": [{ "id": "t1", "title": "t1", "dueDateTime": "2026-09-07T17:30:00Z",
                    "isAllDay": false }],
    }))
    .await;
    let answer = call(&app, json!({ "kind": "dueDateOptions", "taskId": "t1" })).await;
    assert_eq!(answer["ok"], true, "{answer}");
    let tomorrow = &answer["value"]["dates"][1];
    assert_eq!(tomorrow["titleKey"], "picker.tomorrow");
    assert_eq!(tomorrow["dueDateTime"], "2026-09-08T00:00:00Z");
    assert_eq!(tomorrow["isAllDay"], true);
    assert_eq!(answer["value"]["dates"][0]["isSelected"], true, "due today");

    // An editor's unsaved date, no task needed.
    let answer = call(
        &app,
        json!({ "kind": "dueDateOptions",
                "draft": { "dueDateTime": "2026-09-14T00:00:00Z", "isAllDay": true } }),
    )
    .await;
    assert_eq!(answer["value"]["dates"][3]["isSelected"], true, "next week");
}

/// D48: a time pick on an all-day task is that hour on the task's calendar date — west of UTC its
/// midnight reads as the day before, which is not the day the task is on.
#[tokio::test]
async fn aitd461_a_time_pick_stays_on_an_all_day_tasks_date() {
    let app = super::App::with_parts(
        &crate::app::Config {
            cache_path: ":memory:".into(),
            base_url: "https://astrid.cc".into(),
            platform: Default::default(),
        },
        std::sync::Arc::new(crate::platform::MemorySecureStore::new()),
        std::sync::Arc::new(StubTransport::new()),
        std::sync::Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z").in_zone(-7)),
    )
    .expect("starts");
    let answer = call(
        &app,
        json!({ "kind": "dueDateOptions",
                "draft": { "dueDateTime": "2026-09-10T00:00:00Z", "isAllDay": true } }),
    )
    .await;
    // 09:00 on the 10th in California is 16:00 UTC on the 10th.
    assert_eq!(
        answer["value"]["times"][0]["dueDateTime"],
        "2026-09-10T16:00:00Z"
    );
}

/// D49: the repeat presets carry iOS's keys.
#[tokio::test]
async fn aitd461_repeat_presets_use_ios_keys() {
    let app = seeded(json!({
        "tasks": [{ "id": "t1", "title": "t1", "repeating": "weekly" }],
    }))
    .await;
    let answer = call(&app, json!({ "kind": "repeatOptions", "taskId": "t1" })).await;
    assert_eq!(
        answer["value"]["presets"][0]["titleKey"],
        "repeating.one_time_only"
    );
    assert_eq!(answer["value"]["presets"][2]["isSelected"], true);
}
