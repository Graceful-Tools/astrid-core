//! List chat and the conversation with Astrid.

use super::*;

/// A message typed offline is in the transcript at once, marked as still going. The panel is
/// drawn from the cache and catches up afterwards, the same order the task list uses.
#[tokio::test]
async fn a_message_is_in_the_transcript_before_it_is_sent() {
    let app = app_with(StubTransport::new());
    app.store
        .upsert_channels(&[serde_json::from_value(json!({
            "id": "c1", "listId": "l1", "name": "Work"
        }))
        .expect("a channel")])
        .expect("stores");
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    call(
        &app,
        json!({ "kind": "sendChatMessage", "channelId": "c1", "content": "on my way" }),
    )
    .await;

    let panel = call(&app, json!({ "kind": "chat", "listId": "l1" })).await;
    let messages = panel["value"]["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"], "on my way");
    assert_eq!(messages[0]["isMine"], true);
    assert_eq!(messages[0]["isPending"], true);
}

/// AATD-62: My Tasks has a conversation too — iOS's virtual channel, keyed by the reader. Asked for
/// `virtual:my-tasks`, chat finds it and resolveChatChannel answers it.
#[tokio::test]
async fn aatd62_my_tasks_has_its_own_conversation() {
    let app = app_with(StubTransport::new());
    app.store
        .upsert_channels(&[serde_json::from_value(json!({
            "id": "c7", "virtualKey": "virtual-chat:me:my-tasks", "name": "My Tasks"
        }))
        .expect("a channel")])
        .expect("stores");
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    let resolved = call(
        &app,
        json!({ "kind": "resolveChatChannel", "listId": "virtual:my-tasks" }),
    )
    .await;
    assert_eq!(resolved["value"]["id"], "c7", "{resolved}");
    call(
        &app,
        json!({ "kind": "sendChatMessage", "channelId": "c7", "content": "plan the week" }),
    )
    .await;
    let panel = call(
        &app,
        json!({ "kind": "chat", "listId": "virtual:my-tasks" }),
    )
    .await;
    assert_eq!(panel["value"]["channelId"], "c7", "{panel}");
    assert_eq!(panel["value"]["messages"][0]["content"], "plan the week");
}

/// A picture sent in chat: in the transcript at once under the id its thumbnail was drawn with,
/// the upload queued with the conversation's list, and the message waiting for the upload — the
/// Apple apps' upload-then-send chain, now the core's.
#[tokio::test]
async fn a_picture_sent_in_chat_waits_for_its_upload() {
    let app = app_with(StubTransport::new());
    app.store
        .upsert_channels(&[
            serde_json::from_value(json!({ "id": "c1", "listId": "l1" })).expect("a channel"),
        ])
        .expect("stores");
    let scratch = std::env::temp_dir().join(format!("astrid-{}", crate::outbox::new_temp_id()));
    std::fs::write(&scratch, b"a photo").expect("writes");

    let sent = call(
        &app,
        json!({
            "kind": "sendChatMessage", "channelId": "c1", "content": "",
            "path": scratch.to_string_lossy(), "fileId": "temp_thumb", "name": "photo.jpg",
            "mimeType": "image/jpeg", "clientRequestId": "temp_row"
        }),
    )
    .await;
    std::fs::remove_file(&scratch).expect("removes");

    assert_eq!(sent["ok"], true, "{sent}");
    assert_eq!(sent["value"]["id"], "temp_row");
    assert_eq!(sent["value"]["type"], "ATTACHMENT");
    assert_eq!(sent["value"]["secureFiles"][0]["id"], "temp_thumb");
    let queued = crate::outbox::journal::all(&app.store).expect("reads");
    let upload = queued
        .iter()
        .find(|entry| entry.kind == crate::outbox::kind::UPLOAD_ATTACHMENT)
        .expect("the upload is queued");
    assert_eq!(upload.payload["context"]["listId"], "l1");
    let ready = crate::outbox::scheduler::runnable(&queued, app.clock.now(), &Default::default());
    assert_eq!(
        ready
            .iter()
            .map(|entry| entry.kind.as_str())
            .collect::<Vec<_>>(),
        vec![crate::outbox::kind::UPLOAD_ATTACHMENT],
        "the message goes after the file it carries"
    );
}

/// Opening a conversation for the first time: the channel from the server, then its newest page.
#[tokio::test]
async fn a_conversation_is_resolved_then_paged() {
    let transport = StubTransport::new()
        .push_json(
            "/api/v1/chat/channels",
            200,
            json!({ "channel": { "id": "c1", "listId": "l1" } }),
        )
        .push_json(
            "/messages",
            200,
            json!({ "messages": [
                { "id": "m1", "channelId": "c1", "content": "hi", "createdAt": "2026-09-07T11:00:00Z" }
            ], "hasMore": true }),
        );
    let app = app_with(transport);
    let channel = call(
        &app,
        json!({ "kind": "resolveChatChannel", "listId": "l1" }),
    )
    .await;
    assert_eq!(channel["value"]["id"], "c1", "{channel}");
    let page = call(
        &app,
        json!({ "kind": "loadChatMessages", "channelId": "c1" }),
    )
    .await;
    assert_eq!(page["value"]["hasMore"], true, "{page}");
    assert_eq!(page["value"]["messages"][0]["id"], "m1");
    let cached = call(&app, json!({ "kind": "chatMessages", "channelId": "c1" })).await;
    assert_eq!(cached["value"][0]["content"], "hi");
}

/// Chat is a feature a deployment can be without. A shell that read "no channel" as an error
/// would show a broken panel to everybody on a server that simply does not have it.
#[tokio::test]
async fn a_list_with_no_channel_has_an_empty_chat_rather_than_an_error() {
    let app = app_with(StubTransport::new());
    let panel = call(&app, json!({ "kind": "chat", "listId": "l1" })).await;
    assert_eq!(panel["ok"], true);
    assert!(panel["value"]["channelId"].is_null());
    assert!(panel["value"]["messages"]
        .as_array()
        .expect("messages")
        .is_empty());
}
