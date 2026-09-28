//! A list's chat.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// A list's chat, from the cache.
///
/// `channelId` is null when this deployment has no channel for the list — chat is a feature a
/// deployment can be without, and a shell that read that as an error would show a broken panel to
/// everybody using a server that simply does not have it.
pub(super) fn chat(app: &App, list_id: &str) -> Response {
    let channel = match app.context.chat().channel_for_list(list_id) {
        Ok(channel) => channel,
        Err(error) => return Response::failed(error.into()),
    };
    let Some(channel) = channel else {
        return Response::ok(serde_json::json!({
            "channelId": serde_json::Value::Null,
            "messages": [],
        }));
    };

    let messages = app.context.chat().messages(&channel.id).unwrap_or_default();
    let me = app.context.account().current_user_id().ok().flatten();
    let people = app.store.users().unwrap_or_default();
    // A conversation is where a task id is quoted most, so the ids in it link (task 5f3453e2).
    // The list's own board is the one a bare `#12` means.
    let project_id = app
        .store
        .lists()
        .unwrap_or_default()
        .into_iter()
        .find(|list| list.id == list_id)
        .and_then(|list| list.project_id);
    let identifiers = crate::identifier::context_for(
        &app.store.projects().unwrap_or_default(),
        project_id.as_deref(),
    );

    Response::ok(serde_json::json!({
        "channelId": channel.id,
        "name": channel.name,
        "messages": rows::chat::transcript(&messages, me.as_deref(), &people, &identifiers),
    }))
}

/// Catch the chat up with the server.
///
/// The channels first: a list whose channel this client has never seen has nothing to fetch
/// messages for, and that is the ordinary state the first time a conversation is opened.
pub(super) async fn refresh_chat(app: &App, list_id: &str) -> Response {
    if let Err(error) = app.context.chat().refresh_channels().await {
        return Response::failed(error.into());
    }
    let channel = match app.context.chat().channel_for_list(list_id) {
        Ok(Some(channel)) => channel,
        Ok(None) => {
            return Response::ok(serde_json::json!({
                "channelId": serde_json::Value::Null,
                "messages": [],
            }))
        }
        Err(error) => return Response::failed(error.into()),
    };
    if let Err(error) = app.context.chat().refresh_messages(&channel.id).await {
        return Response::failed(error.into());
    }
    chat(app, list_id)
}

pub(super) struct ChatSendOptions<'a> {
    pub reply_to_id: Option<&'a str>,
    pub message_type: Option<crate::model::CommentType>,
    pub file_id: Option<&'a str>,
    pub path: Option<&'a str>,
    pub name: Option<&'a str>,
    pub mime_type: Option<&'a str>,
    pub client_request_id: Option<&'a str>,
}

/// Say something — with a file from this device, queued to upload first, or one already on the
/// server.
pub(super) fn send_chat_message(
    app: &App,
    channel_id: &str,
    content: &str,
    options: ChatSendOptions<'_>,
) -> Response {
    let file = match options.path {
        Some(path) => {
            // The server is told whose file it is: the channel's list, or the channel itself for
            // a conversation with no list (My Tasks) — what the Apple apps sent.
            let list_id = app
                .context
                .chat()
                .channels()
                .unwrap_or_default()
                .into_iter()
                .find(|channel| channel.id == channel_id)
                .and_then(|channel| channel.list_id);
            let context = match list_id {
                Some(list_id) => serde_json::json!({ "listId": list_id }),
                None => serde_json::json!({ "channelId": channel_id }),
            };
            match queue_upload(
                app,
                path,
                options.file_id,
                options.name,
                options.mime_type,
                context,
            ) {
                Ok(file) => Some(file),
                Err(response) => return response,
            }
        }
        None => options.file_id.map(|id| crate::model::SecureFile {
            id: id.to_string(),
            name: options.name.unwrap_or_default().to_string(),
            size: 0,
            mime_type: options.mime_type.unwrap_or_default().to_string(),
        }),
    };
    let message_type = options.message_type.unwrap_or(match file {
        Some(_) => crate::model::CommentType::Attachment,
        None => crate::model::CommentType::Text,
    });
    let author = app.context.account().current_user_id().ok().flatten();
    answer(app.context.chat().send_as(
        channel_id,
        crate::services::chat::Outgoing {
            content,
            message_type,
            file: file.as_ref(),
            reply_to_id: options.reply_to_id,
            author_id: author.as_deref(),
            id: options.client_request_id,
        },
    ))
}

/// Fetch a page of a channel's history, and answer with the channel as the cache now holds it.
pub(super) async fn load_chat_messages(
    app: &App,
    channel_id: &str,
    before: Option<&str>,
    limit: Option<u32>,
) -> Response {
    let before = match before.map(crate::model::date::parse) {
        Some(Some(instant)) => Some(instant),
        Some(None) => return Response::failed(Failure::bad_request("before is not a date")),
        None => None,
    };
    let has_more = match app
        .context
        .chat()
        .load_messages(channel_id, before, limit)
        .await
    {
        Ok(has_more) => has_more,
        Err(error) => return Response::failed(error.into()),
    };
    Response::ok(serde_json::json!({
        "messages": app.context.chat().messages(channel_id).unwrap_or_default(),
        "hasMore": has_more,
    }))
}
