//! Comments, their edits, and the suggestions the box offers.

use super::*;

/// `@` in the comment box offers the task's people and the agent, and choosing one puts the
/// reference the server resolves into the text (task 3271a0c5).
#[tokio::test]
async fn the_comment_box_offers_people_lists_and_tasks_and_inserts_the_reference_task_3271a0c5() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
            .upsert_users(&[
                serde_json::from_value(json!({ "id": "ai-agent-astrid", "name": "Astrid", "email": "astrid@astrid.cc", "isAIAgent": true })).expect("a user"),
                serde_json::from_value(json!({ "id": "me", "name": "Jon", "email": "jon@x.io" })).expect("a user"),
            ])
            .expect("stores");
    call(&app, json!({ "kind": "createList", "name": "Health" })).await;
    let health = list_id_named(&app, "Health").await;
    let created = call(
        &app,
        json!({ "kind": "createTask", "title": "Pushups", "listIds": [health] }),
    )
    .await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let people = call(
        &app,
        json!({ "kind": "commentSuggestions", "taskId": task_id, "text": "hey @as", "caret": 7 }),
    )
    .await;
    assert_eq!(people["ok"], true, "{people}");
    assert_eq!(people["value"]["trigger"]["kind"], "mention");
    assert_eq!(people["value"]["trigger"]["query"], "as");
    assert_eq!(people["value"]["items"][0]["id"], "ai-agent-astrid");
    assert_eq!(people["value"]["items"][0]["isAgent"], true);
    assert_eq!(
        people["value"]["items"].as_array().unwrap().len(),
        1,
        "never the reader"
    );

    let lists = call(
        &app,
        json!({ "kind": "commentSuggestions", "taskId": task_id, "text": "see #", "caret": 5 }),
    )
    .await;
    assert_eq!(lists["value"]["items"][0]["label"], "Health");

    let tasks = call(
        &app,
        json!({ "kind": "commentSuggestions", "taskId": task_id, "text": "!push", "caret": 5 }),
    )
    .await;
    assert_eq!(tasks["value"]["items"][0]["label"], "Pushups");
    assert_eq!(tasks["value"]["items"][0]["secondary"], "Health");

    let quiet = call(
        &app,
        json!({ "kind": "commentSuggestions", "taskId": task_id, "text": "nothing", "caret": 7 }),
    )
    .await;
    assert!(quiet["value"]["trigger"].is_null());

    let applied = call(
        &app,
        json!({
            "kind": "applyCommentSuggestion", "text": "hey @as", "caret": 7,
            "triggerKind": "mention", "id": "ai-agent-astrid", "label": "Astrid"
        }),
    )
    .await;
    assert_eq!(applied["ok"], true, "{applied}");
    assert_eq!(applied["value"]["text"], "hey @[Astrid](ai-agent-astrid) ");
    assert_eq!(
        applied["value"]["caret"],
        "hey @[Astrid](ai-agent-astrid) ".encode_utf16().count()
    );
}

/// A reply nests under the comment it answers, an edit changes what it says, and a delete
/// takes it out — each offline, through the Outbox, and each drawn at once (task 97c817dd).
#[tokio::test]
async fn a_comment_can_be_replied_to_edited_and_deleted_through_the_door_task_97c817dd() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    let created = call(&app, json!({ "kind": "createTask", "title": "Plan" })).await;
    let task_id = created["value"]["id"].as_str().expect("an id").to_string();

    let first = call(
        &app,
        json!({ "kind": "postComment", "taskId": task_id, "content": "Thoughts?" }),
    )
    .await;
    let first_id = first["value"]["id"].as_str().expect("an id").to_string();
    let reply = call(
        &app,
        json!({
            "kind": "postComment", "taskId": task_id, "content": "Yes",
            "parentCommentId": first_id
        }),
    )
    .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["value"]["parentCommentId"], first_id);
    let reply_id = reply["value"]["id"].as_str().expect("an id").to_string();
    let later = call(
        &app,
        json!({ "kind": "postComment", "taskId": task_id, "content": "Another thread" }),
    )
    .await;
    assert_eq!(later["ok"], true);

    // The test clock stands still, so the two top-level comments share a timestamp and the
    // cache orders them by id; the rule under test is that the reply comes right after ITS
    // parent, wherever the parent sits.
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
    let comments = detail["value"]["comments"].as_array().expect("rows");
    assert_eq!(comments.len(), 3);
    let parent_at = comments
        .iter()
        .position(|row| row["id"] == first_id)
        .expect("the parent is drawn");
    let reply_row = &comments[parent_at + 1];
    assert_eq!(reply_row["id"], reply_id, "the reply follows its parent");
    assert_eq!(reply_row["isReply"], true);
    assert_eq!(reply_row["parentId"], first_id);
    assert_eq!(reply_row["indentRight"], true, "the parent is mine");
    let other = comments
        .iter()
        .find(|row| row["content"] == "Another thread")
        .expect("the other thread is drawn");
    assert_eq!(other["isReply"], false);

    let edited = call(
        &app,
        json!({ "kind": "editComment", "commentId": reply_id, "content": "Yes, tomorrow" }),
    )
    .await;
    assert_eq!(edited["ok"], true, "{edited}");
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
    assert!(
        detail["value"]["comments"]
            .as_array()
            .expect("rows")
            .iter()
            .any(|row| row["id"] == reply_id && row["content"] == "Yes, tomorrow"),
        "{detail}"
    );

    let deleted = call(
        &app,
        json!({ "kind": "deleteComment", "commentId": reply_id }),
    )
    .await;
    assert_eq!(deleted["ok"], true);
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
    assert_eq!(
        detail["value"]["comments"].as_array().expect("rows").len(),
        2
    );

    // All of it is journalled for the server: a create, a create with a parent, a create, an
    // update and a delete.
    let outbox = call(&app, json!({ "kind": "outboxStats" })).await;
    assert!(
        outbox["value"]["pending"].as_i64().unwrap_or(0) >= 5,
        "{outbox}"
    );
}

/// The Apple apps draw a comment before they post it; the posted comment has to be that row —
/// the id they drew it under, the author and type they said — or the thread shows it twice
/// (AITD-331).
#[tokio::test]
async fn a_comment_is_posted_as_the_row_the_shell_already_drew() {
    let app = app_with(StubTransport::new());
    let task = call(
        &app,
        json!({ "kind": "createTask", "title": "Talk about it" }),
    )
    .await;
    let task_id = task["value"]["id"].as_str().expect("an id").to_string();

    let posted = call(
        &app,
        json!({ "kind": "postComment", "taskId": task_id, "content": "**hello**",
                "type": "MARKDOWN", "authorId": "someone-else",
                "clientRequestId": "temp_drawn-row", "fileId": "file-1" }),
    )
    .await;
    assert_eq!(posted["ok"], true, "{posted}");
    assert_eq!(posted["value"]["id"], "temp_drawn-row");
    assert_eq!(posted["value"]["type"], "MARKDOWN");
    assert_eq!(posted["value"]["authorId"], "someone-else");
    assert_eq!(posted["value"]["secureFiles"][0]["id"], "file-1");
}
