//! Attachments and the clipboard.

use super::*;

/// The clipboard is the shell's to read and the core's to interpret, so the answer has to name
/// what to do rather than leaving the shell to work it out again.
#[tokio::test]
async fn a_pasted_file_beats_the_picture_of_it() {
    let app = app_with(StubTransport::new());

    let answer = call(
        &app,
        json!({
            "kind": "clipboardPaste",
            "files": [r"C:\shots\one.png"],
            "imageExtension": "png",
        }),
    )
    .await;

    assert_eq!(answer["value"]["action"], "files");
    assert_eq!(answer["value"]["files"][0], r"C:\shots\one.png");
}

/// An ordinary paste stays an ordinary paste, which is the trade this whole path is careful
/// about.
#[tokio::test]
async fn a_paste_with_nothing_attachable_is_left_to_type() {
    let app = app_with(StubTransport::new());

    let answer = call(&app, json!({ "kind": "clipboardPaste", "hasText": true })).await;

    assert_eq!(answer["value"]["action"], "text");
}

#[tokio::test]
async fn a_pasted_screenshot_comes_back_with_a_name() {
    let app = app_with(StubTransport::new());

    let answer = call(
        &app,
        json!({ "kind": "clipboardPaste", "imageExtension": "png" }),
    )
    .await;

    assert_eq!(answer["value"]["action"], "image");
    assert_eq!(
        answer["value"]["name"],
        "Pasted Image 2026-09-07 at 12.00.00.png"
    );
}

/// The whole offline story for attachments in one test: on the task immediately, in the
/// journal, and with a copy of the bytes that survives the original being deleted.
#[tokio::test]
async fn attaching_a_file_works_with_no_connection_at_all() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    let task_id = made["value"]["id"].as_str().expect("an id").to_string();

    let scratch = std::env::temp_dir().join(format!("astrid-{}", crate::outbox::new_temp_id()));
    std::fs::write(&scratch, b"a receipt").expect("writes");

    let attached = call(
        &app,
        json!({
            "kind": "attachFile",
            "taskId": task_id,
            "path": scratch.to_string_lossy(),
        }),
    )
    .await;
    // The original goes, the way a downloads folder gets tidied.
    std::fs::remove_file(&scratch).expect("removes");

    assert_eq!(attached["ok"], true, "{attached}");
    let queued = crate::outbox::journal::all(&app.store).expect("reads");
    let upload = queued
        .iter()
        .find(|entry| entry.kind == crate::outbox::kind::UPLOAD_ATTACHMENT)
        .expect("the upload is queued");
    let held = upload.payload["localPath"].as_str().expect("a path");
    assert_eq!(
        std::fs::read(held).expect("the copy is still there"),
        b"a receipt",
        "the bytes were copied, not merely pointed at"
    );

    // And the comment that carries it names the file by the id the upload will resolve.
    let comment = queued
        .iter()
        .find(|entry| entry.kind == crate::outbox::kind::CREATE_COMMENT)
        .expect("the comment is queued");
    assert_eq!(
        comment.payload["body"]["fileId"].as_str(),
        upload.temp_id.as_deref(),
    );
}

/// Task 48f72aa7: the bytes of a delivered upload go into the download cache rather than being
/// deleted, and the handler has no idea where this installation keeps its files — so the entry
/// carries the destination. Without it the picture is fetched back from the server that was just
/// given it.
#[tokio::test]
async fn an_attachment_says_where_its_bytes_belong_once_it_has_been_sent() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    let task_id = made["value"]["id"].as_str().expect("an id").to_string();

    let scratch = std::env::temp_dir().join(format!("astrid-{}", crate::outbox::new_temp_id()));
    std::fs::write(&scratch, b"a screenshot").expect("writes");
    call(
        &app,
        json!({ "kind": "attachFile", "taskId": task_id, "path": scratch.to_string_lossy() }),
    )
    .await;
    std::fs::remove_file(&scratch).expect("removes");

    let queued = crate::outbox::journal::all(&app.store).expect("reads");
    let upload = queued
        .iter()
        .find(|entry| entry.kind == crate::outbox::kind::UPLOAD_ATTACHMENT)
        .expect("the upload is queued");
    assert_eq!(
        upload.payload["cacheDir"].as_str(),
        Some(app.attachment_cache().to_string_lossy().as_ref()),
    );
}

/// The upload has to be sent before the comment that names its file, which is what the order
/// in the journal decides.
#[tokio::test]
async fn the_upload_is_queued_before_the_comment_that_carries_it() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    let task_id = made["value"]["id"].as_str().expect("an id").to_string();
    let scratch = std::env::temp_dir().join(format!("astrid-{}", crate::outbox::new_temp_id()));
    std::fs::write(&scratch, b"a receipt").expect("writes");

    call(
        &app,
        json!({
            "kind": "attachFile",
            "taskId": task_id,
            "path": scratch.to_string_lossy(),
        }),
    )
    .await;
    let _ = std::fs::remove_file(&scratch);

    let kinds: Vec<String> = crate::outbox::journal::all(&app.store)
        .expect("reads")
        .into_iter()
        .map(|entry| entry.kind)
        .collect();
    let upload = kinds
        .iter()
        .position(|kind| kind == crate::outbox::kind::UPLOAD_ATTACHMENT)
        .expect("an upload");
    let comment = kinds
        .iter()
        .position(|kind| kind == crate::outbox::kind::CREATE_COMMENT)
        .expect("a comment");
    assert!(upload < comment);
}

/// A file that is not there cannot be attached, and saying so beats a comment pointing at
/// nothing.
#[tokio::test]
async fn attaching_a_file_that_is_not_there_fails_rather_than_queueing_nothing() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    let task_id = made["value"]["id"].as_str().expect("an id").to_string();

    let attached = call(
        &app,
        json!({
            "kind": "attachFile",
            "taskId": task_id,
            "path": "C:\\nowhere\\nothing.png",
        }),
    )
    .await;

    assert_eq!(attached["ok"], false);
    assert!(!crate::outbox::journal::all(&app.store)
        .expect("reads")
        .iter()
        .any(|entry| entry.kind == crate::outbox::kind::UPLOAD_ATTACHMENT));
}

/// A file reaches a task through a comment — there is no attach-to-task endpoint anywhere —
/// so the attachments on a task are its own files plus its comments'.
#[tokio::test]
async fn attachments_are_gathered_from_the_task_and_its_comments() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Plan the trip" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    // A comment carrying a file, as one arrives from the server.
    app.store
        .upsert_comments(&[serde_json::from_value(json!({
            "id": "c1",
            "taskId": id,
            "content": "the itinerary",
            "secureFiles": [{
                "id": "f1",
                "originalName": "itinerary.pdf",
                "fileSize": 1024,
                "mimeType": "application/pdf",
            }],
        }))
        .expect("a comment")])
        .expect("stores");

    let found = call(&app, json!({ "kind": "attachments", "taskId": id })).await;
    let files = found["value"]["files"].as_array().expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "itinerary.pdf");
    // Not downloaded yet, so the shell offers to fetch it rather than to open it.
    assert_eq!(files[0]["isCached"], false);
}

#[tokio::test]
async fn downloading_a_file_that_is_not_on_the_task_is_a_not_found() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createTask", "title": "Plan the trip" }),
    )
    .await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let answered = call(
        &app,
        json!({ "kind": "downloadAttachment", "taskId": id, "fileId": "nope" }),
    )
    .await;
    assert_eq!(answered["error"]["kind"], "notFound");
}

/// A photo attached from the Apple apps keeps the temporary id its thumbnail is drawn under, and
/// the name and type the picker gave it.
#[tokio::test]
async fn an_attached_file_keeps_the_id_its_thumbnail_is_drawn_under() {
    let app = app_with(StubTransport::new());
    let task = call(&app, json!({ "kind": "createTask", "title": "Photo" })).await;
    let task_id = task["value"]["id"].as_str().expect("an id").to_string();
    let path = std::env::temp_dir().join(format!("astrid-{}.bin", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"not really a jpeg").expect("writes");

    let posted = call(
        &app,
        json!({ "kind": "attachFile", "taskId": task_id, "path": path.to_string_lossy(),
                "fileId": "temp_thumb", "name": "IMG_0001.jpg", "mimeType": "image/jpeg",
                "clientRequestId": "temp_comment" }),
    )
    .await;
    assert_eq!(posted["ok"], true, "{posted}");
    assert_eq!(posted["value"]["id"], "temp_comment");
    assert_eq!(posted["value"]["secureFiles"][0]["id"], "temp_thumb");
    assert_eq!(
        posted["value"]["secureFiles"][0]["originalName"],
        "IMG_0001.jpg"
    );
    assert_eq!(posted["value"]["secureFiles"][0]["mimeType"], "image/jpeg");
    let _ = std::fs::remove_file(path);
}
