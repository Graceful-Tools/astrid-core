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
