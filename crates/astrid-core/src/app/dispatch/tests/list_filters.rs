//! A list's own filter.

use super::*;

/// One filter sheet, whichever of the two it is looking at — so it has to answer for My Tasks
/// as well, which has no list row to read.
#[tokio::test]
async fn the_filter_sheet_answers_for_my_tasks_too() {
    let app = app_with(StubTransport::new());

    let options = call(
        &app,
        json!({ "kind": "filterOptions", "listId": "virtual:my-tasks" }),
    )
    .await;

    assert_eq!(options["ok"], true, "{options}");
    assert_eq!(options["value"]["isFiltered"], false);
    assert!(!options["value"]["groups"]
        .as_array()
        .expect("groups")
        .is_empty());
}

/// The same command on a real list is the ordinary list update it always was.
#[tokio::test]
async fn setting_a_list_filter_writes_it_to_the_list() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let set = call(
        &app,
        json!({
            "kind": "setFilter",
            "listId": id,
            "field": "filterCompletion",
            "value": "all",
        }),
    )
    .await;

    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["value"]["filterCompletion"], "all");
}

/// A group a list has and My Tasks does not — "in lists" — has to say so rather than look
/// like a control that quietly does nothing.
#[tokio::test]
async fn a_filter_my_tasks_does_not_have_says_so() {
    let app = app_with(StubTransport::new());

    let set = call(
        &app,
        json!({
            "kind": "setFilter",
            "listId": "virtual:my-tasks",
            "field": "filterInLists",
            "value": "not_in_list",
        }),
    )
    .await;

    assert_eq!(set["ok"], false);
}

/// The sheet offers the values the rules match on, and setting one is an ordinary list edit —
/// which is what makes a filter set here mean the same thing on web.
#[tokio::test]
async fn a_filter_can_be_read_and_then_set() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let offered = call(&app, json!({ "kind": "filterOptions", "listId": id })).await;
    assert_eq!(offered["value"]["isFiltered"], false);
    let groups = offered["value"]["groups"].as_array().expect("groups");
    let due = groups
        .iter()
        .find(|group| group["field"] == "filterDueDate")
        .expect("a due-date group");
    assert_eq!(due["picks"][0]["value"], "all");
    assert_eq!(due["picks"][0]["isSelected"], true);

    call(
        &app,
        json!({
            "kind": "updateList",
            "listId": id,
            "changes": { "filterDueDate": "today" },
        }),
    )
    .await;

    let again = call(&app, json!({ "kind": "filterOptions", "listId": id })).await;
    assert_eq!(again["value"]["isFiltered"], true);
    let due = again["value"]["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .find(|group| group["field"] == "filterDueDate")
        .expect("a due-date group")
        .clone();
    let selected: Vec<&str> = due["picks"]
        .as_array()
        .expect("picks")
        .iter()
        .filter(|pick| pick["isSelected"] == true)
        .map(|pick| pick["value"].as_str().expect("a value"))
        .collect();
    assert_eq!(selected, vec!["today"]);
}
