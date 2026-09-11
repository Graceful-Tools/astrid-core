//! Files on a task: listing, fetching, attaching, pasting.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// The files on a task, and whether each is already on this machine.
pub(super) fn attachments(app: &App, task_id: &str) -> Response {
    let service = app.context.attachments(app.attachment_cache());
    match service.for_task(task_id) {
        Ok(files) => Response::ok(serde_json::json!({
            "files": files
                .iter()
                .map(|file| serde_json::json!({
                    "id": file.id,
                    "name": file.name,
                    "size": file.size,
                    "mimeType": file.mime_type,
                    // So the shell can offer "Open" rather than "Download" for one already here.
                    "isCached": service.is_cached(file),
                    "path": service.cached_path(file).to_string_lossy(),
                }))
                .collect::<Vec<_>>(),
        })),
        Err(error) => Response::failed(error.into()),
    }
}

/// Fetch one file and say where it landed.
pub(super) async fn download_attachment(app: &App, task_id: &str, file_id: &str) -> Response {
    let service = app.context.attachments(app.attachment_cache());
    let file = match service.for_task(task_id) {
        Ok(files) => files.into_iter().find(|file| file.id == file_id),
        Err(error) => return Response::failed(error.into()),
    };
    let Some(file) = file else {
        return Response::failed(Failure::not_found("attachment", file_id));
    };
    match service.download(&file).await {
        Ok(path) => Response::ok(serde_json::json!({ "path": path.to_string_lossy() })),
        Err(error) => Response::failed(error.into()),
    }
}

/// Upload a file and post the comment that carries it.
///
/// One command rather than two, because a file uploaded with no comment naming it is a file nobody
/// can reach: it exists on the server and appears on no task.
pub(super) fn attach_file(app: &App, task_id: &str, path: &str, content: Option<&str>) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    // The list decides who may read the file afterwards, so the server is told which one.
    let list_id = task.effective_list_ids().into_iter().next();

    // A copy on disk and a temporary id, not a request. The file is on the task the moment
    // somebody chooses it, and it goes when there is a connection — see the module note on
    // `astrid_core::services::attachment`.
    let service = app.context.attachments(app.attachment_cache());
    let (file, held) = match service.queue(std::path::Path::new(path)) {
        Ok(queued) => queued,
        Err(error) => return Response::failed(error.into()),
    };

    let entry = crate::outbox::build(
        crate::outbox::kind::UPLOAD_ATTACHMENT,
        serde_json::json!({
            "localPath": held.to_string_lossy(),
            "name": file.name,
            "mimeType": file.mime_type,
            "context": { "listId": list_id },
        }),
        &file.id,
        app.clock.now(),
    )
    .for_temp_id(&file.id);
    if let Err(error) = crate::outbox::journal::enqueue(&app.store, &entry) {
        return Response::failed(error.into());
    }

    // Queued after the upload, so it goes second and finds the real file id waiting for it.
    let author = app.context.account().current_user_id().ok().flatten();
    answer(app.context.comments().post(
        task_id,
        content.unwrap_or_default(),
        author.as_deref(),
        crate::model::CommentType::Attachment,
        Some(&file),
    ))
}

/// What a paste should attach, if anything.
pub(super) fn clipboard_paste(
    app: &App,
    files: Vec<String>,
    image_extension: Option<String>,
    has_text: bool,
) -> Response {
    let board = crate::paste::Clipboard {
        files,
        image_extension,
        has_text,
    };
    match crate::paste::decide(&board, app.clock.now(), app.clock.utc_offset()) {
        crate::paste::Paste::Files(files) => {
            Response::ok(serde_json::json!({ "action": "files", "files": files }))
        }
        crate::paste::Paste::Image { name } => {
            Response::ok(serde_json::json!({ "action": "image", "name": name }))
        }
        // Named rather than empty: "nothing to attach" and "the core did not understand you" are
        // different answers, and a shell that could not tell them apart would swallow a text paste
        // on the day this command grows a new shape.
        crate::paste::Paste::Text => Response::ok(serde_json::json!({ "action": "text" })),
    }
}
