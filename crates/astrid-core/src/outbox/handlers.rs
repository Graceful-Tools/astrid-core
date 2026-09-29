//! What each kind of entry actually does when its turn comes.
//!
//! Ported from the handler files in `astrid-ios/Astrid App/Core/Outbox/`.
//!
//! A handler does exactly two things: send the request the entry describes, and fold the server's
//! answer back into the cache. It decides nothing — the service decided all of that when it
//! enqueued the entry, which is why an entry is runnable weeks later on a launch where nothing
//! else about the app's state survived.
//!
//! ## Idempotency is the whole safety argument
//!
//! Every entry carries a `client_request_id`, and every create route treats it as a key: replaying
//! a request the server already applied returns the row it already made. That is what makes
//! resetting an interrupted entry to pending safe (see [`super::scheduler::recovered_on_load`]),
//! and it is what makes a timeout — the case where the client cannot know whether the write
//! landed — recoverable instead of a coin flip between a lost task and a duplicated one.

use std::collections::BTreeMap;

use crate::api::{endpoints, ApiClient, ApiError};
use crate::model::{ChatMessage, Comment, Task, TaskList};
use crate::store::Store;

use super::entry::{kind, Entry};

/// What a handler produced.
pub enum Outcome {
    /// It worked. The map is what dependents may read — a server id, a file id.
    Done(Option<BTreeMap<String, String>>),
    /// It did not work, and trying again might.
    Retry(String),
    /// It did not work and never will. Dead-letter it.
    Dead(String),
    /// The request never reached the server. Not a failure of the write: it waits for the
    /// network without using up an attempt.
    Offline(String),
}

impl Outcome {
    fn done() -> Self {
        Outcome::Done(None)
    }

    fn producing(key: &str, value: &str) -> Self {
        Outcome::Done(Some(BTreeMap::from([(key.to_string(), value.to_string())])))
    }
}

/// Turn an API failure into the right kind of ending.
///
/// The classification lives in [`super::scheduler::is_permanent_failure`] so that this — the part
/// with I/O in it — holds no policy of its own.
fn from_error(error: ApiError) -> Outcome {
    if let ApiError::Transport(transport) = &error {
        return Outcome::Offline(format!("{}{transport}", super::journal::OFFLINE_PREFIX));
    }
    match error.status() {
        Some(status) if super::scheduler::is_permanent_failure(status) => {
            Outcome::Dead(format!("{status}: {error}"))
        }
        _ => Outcome::Retry(error.to_string()),
    }
}

/// Run one entry.
///
/// A `match` rather than a registry of handler objects: the kinds are a closed set defined in this
/// crate, and a registry would buy indirection instead of extensibility. An unknown kind — an
/// entry written by a newer build — is dead-lettered rather than retried forever, and says so.
pub async fn perform(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    match entry.kind.as_str() {
        kind::CREATE_TASK => create_task(client, store, entry).await,
        kind::UPDATE_TASK | kind::COMPLETE_TASK => update_task(client, store, entry).await,
        kind::DELETE_TASK => delete_task(client, store, entry).await,
        kind::UPLOAD_ATTACHMENT => upload_attachment(client, store, entry).await,
        kind::CREATE_COMMENT => create_comment(client, store, entry).await,
        kind::UPDATE_COMMENT => update_comment(client, store, entry).await,
        kind::DELETE_COMMENT => delete_comment(client, store, entry).await,
        kind::SEND_CHAT_MESSAGE => send_chat_message(client, store, entry).await,
        kind::INVITE_TO_LIST => invite_to_list(client, store, entry).await,
        kind::SET_MEMBER_ROLE => set_member_role(client, entry).await,
        kind::REMOVE_MEMBER => remove_member(client, entry).await,
        kind::CANCEL_INVITATION => cancel_invitation(client, entry).await,
        kind::SET_INVITATION_ROLE => set_invitation_role(client, entry).await,
        kind::CREATE_LIST => create_list(client, store, entry).await,
        kind::UPDATE_LIST => update_list(client, store, entry).await,
        kind::DELETE_LIST => delete_list(client, store, entry).await,
        kind::SET_MANUAL_ORDER => set_manual_order(client, store, entry).await,
        kind::UPDATE_SETTINGS => {
            send_body(client, client.put(endpoints::USER_SETTINGS), entry).await
        }
        kind::UPDATE_SMART_TASKS => {
            send_body(client, client.patch(endpoints::SMART_TASKS), entry).await
        }
        kind::ADD_TASK_BLOCKER => add_task_blocker(client, store, entry).await,
        kind::REMOVE_TASK_BLOCKER => remove_task_blocker(client, store, entry).await,
        unknown => Outcome::Dead(format!(
            "no handler for {unknown} — the journal was written by a newer build"
        )),
    }
}

/// The `body` an entry carries, or an empty object. A missing body is not a failure: several kinds
/// are nothing but an id.
fn body(entry: &Entry) -> serde_json::Value {
    entry
        .payload
        .get("body")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}

/// A required id from the payload.
fn id<'a>(entry: &'a Entry, field: &str) -> Result<&'a str, Outcome> {
    entry
        .payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            // Not retryable: the payload is what it is, and it will still be wrong in five minutes.
            Outcome::Dead(format!("the payload has no {field}"))
        })
}

/// Pull the object out of an envelope (`{ "task": … }`), or take the body as-is when the route
/// answered bare. Both shapes exist across v1, and on the same route across deployments.
fn unwrap_envelope(value: serde_json::Value, key: &str) -> serde_json::Value {
    value.get(key).cloned().unwrap_or(value)
}

async fn create_task(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let request = client.post(endpoints::TASKS).value(with_client_request_id(
        body(entry),
        &entry.client_request_id,
    ));
    match client.send(request).await {
        Ok(value) => {
            let created: Task =
                match serde_json::from_value(unwrap_envelope(value, endpoints::envelope::TASK)) {
                    Ok(task) => task,
                    Err(error) => {
                        return Outcome::Retry(format!("unreadable create response: {error}"))
                    }
                };
            // The optimistic row goes, the real one arrives, and the mapping outlives both so a
            // queued edit that still names the temporary id can find its way.
            if let Some(temp_id) = &entry.temp_id {
                let _ = store.record_id_mapping(
                    temp_id,
                    &created.id,
                    created.updated_at.unwrap_or_else(chrono::Utc::now),
                );
                if temp_id != &created.id {
                    let _ = store.delete_task(temp_id);
                }
            }
            let _ = store.upsert_task(&created);
            Outcome::producing("taskId", &created.id)
        }
        Err(error) => from_error(error),
    }
}

async fn update_task(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let task_id = match id(entry, "taskId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client.put(endpoints::task(task_id)).value(body(entry));
    match client.send(request).await {
        Ok(value) => {
            if let Ok(task) =
                serde_json::from_value::<Task>(unwrap_envelope(value, endpoints::envelope::TASK))
            {
                let task = match store.task(&task.id) {
                    Ok(Some(cached)) => task.keeping_unsent(&cached),
                    _ => task,
                };
                let _ = store.upsert_task(&task);
            }
            Outcome::done()
        }
        Err(error) => from_error(error),
    }
}

async fn delete_task(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let task_id = match id(entry, "taskId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    match client.send(client.delete(endpoints::task(task_id))).await {
        Ok(_) => {
            let _ = store.delete_task(task_id);
            Outcome::done()
        }
        // A task that is already gone is a delete that succeeded. Dead-lettering a 404 here would
        // leave the row in the cache and show the user a task they deleted twice.
        Err(error) if error.status() == Some(404) => {
            let _ = store.delete_task(task_id);
            Outcome::done()
        }
        Err(error) => from_error(error),
    }
}

/// One task starts waiting on another (task 69a840a4).
///
/// `201` created and `200` already-linked are both success: the edge's unique constraint makes the
/// write idempotent, which is what lets a replayed journal be harmless. A `409` is the cycle
/// refusal and is **dead-lettered rather than retried** — the graph will still contain the cycle in
/// five minutes, so retrying is a loop that never ends. The refusal is surfaced instead, because
/// the row has drawn an optimistic chip that has to come back off.
async fn add_task_blocker(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let task_id = match id(entry, "taskId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client
        .post(endpoints::task_blockers(task_id))
        .value(body(entry));
    match client.send(request).await {
        Ok(value) => {
            cache_blocked_by(store, task_id, value);
            Outcome::done()
        }
        Err(error) if error.status() == Some(409) => Outcome::Dead(
            "those tasks would end up waiting for each other, so the link was refused".into(),
        ),
        Err(error) => from_error(error),
    }
}

/// One task stops waiting on another.
///
/// A `404` is a removal that already happened — the edge is gone, which is the state asked for —
/// so it succeeds rather than dead-lettering, for the same reason deleting an absent task does.
async fn remove_task_blocker(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let task_id = match id(entry, "taskId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let blocking_task_id = match id(entry, "blockingTaskId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    match client
        .send(client.delete(endpoints::task_blocker(task_id, blocking_task_id)))
        .await
    {
        Ok(value) => {
            cache_blocked_by(store, task_id, value);
            Outcome::done()
        }
        Err(error) if error.status() == Some(404) => Outcome::done(),
        Err(error) => from_error(error),
    }
}

/// Replace the cached `blockedBy` with what the mutation answered, and mirror the ids onto the
/// task.
///
/// The mutation response carries the whole list rather than the one edge, so the optimistic chip
/// — which may have been drawn as `hidden` for want of a local copy — is corrected by the same
/// round trip that made the change. A response this build cannot read leaves the cache alone: the
/// optimistic state is closer to the truth than an empty row would be.
fn cache_blocked_by(store: &Store, task_id: &str, value: serde_json::Value) {
    let Some(blocked_by) = value.get("blockedBy").cloned() else {
        return;
    };
    let key = crate::services::dependency::cache_key(task_id);
    let mut dependencies: crate::services::Dependencies = store
        .metadata(&key)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    match serde_json::from_value(blocked_by) {
        Ok(blockers) => dependencies.blocked_by = blockers,
        Err(_) => return,
    }
    if let Ok(json) = serde_json::to_string(&dependencies) {
        let _ = store.set_metadata(&key, &json);
    }
    if let Ok(Some(mut task)) = store.task(task_id) {
        task.blocked_by = Some(
            dependencies
                .blocked_by
                .iter()
                .map(|blocker| blocker.id.clone())
                .collect(),
        );
        let _ = store.upsert_task(&task);
    }
}

async fn create_comment(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let task_id = match id(entry, "taskId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client
        .post(endpoints::task_comments(task_id))
        .value(with_client_request_id(
            body(entry),
            &entry.client_request_id,
        ));
    match client.send(request).await {
        Ok(value) => {
            let created: Comment = match serde_json::from_value(unwrap_envelope(
                value,
                endpoints::envelope::COMMENT,
            )) {
                Ok(comment) => comment,
                Err(error) => {
                    return Outcome::Retry(format!("unreadable comment response: {error}"))
                }
            };
            if let Some(temp_id) = &entry.temp_id {
                if temp_id != &created.id {
                    let _ = store.delete_comment(temp_id);
                }
            }
            let _ = store.upsert_comments(std::slice::from_ref(&created));
            Outcome::producing("commentId", &created.id)
        }
        Err(error) => from_error(error),
    }
}

async fn update_comment(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let comment_id = match id(entry, "commentId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client
        .put(endpoints::comment(comment_id))
        .value(body(entry));
    match client.send(request).await {
        Ok(value) => {
            if let Ok(comment) = serde_json::from_value::<Comment>(unwrap_envelope(
                value,
                endpoints::envelope::COMMENT,
            )) {
                let _ = store.upsert_comments(&[comment]);
            }
            Outcome::done()
        }
        Err(error) => from_error(error),
    }
}

async fn delete_comment(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let comment_id = match id(entry, "commentId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    match client
        .send(client.delete(endpoints::comment(comment_id)))
        .await
    {
        Ok(_) => {
            let _ = store.delete_comment(comment_id);
            Outcome::done()
        }
        Err(error) if error.status() == Some(404) => {
            let _ = store.delete_comment(comment_id);
            Outcome::done()
        }
        Err(error) => from_error(error),
    }
}

async fn send_chat_message(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let channel_id = match id(entry, "channelId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request =
        client
            .post(endpoints::channel_messages(channel_id))
            .value(with_client_request_id(
                body(entry),
                &entry.client_request_id,
            ));
    match client.send(request).await {
        Ok(value) => {
            let sent: ChatMessage = match serde_json::from_value(unwrap_envelope(
                value,
                endpoints::envelope::MESSAGE,
            )) {
                Ok(message) => message,
                Err(error) => {
                    return Outcome::Retry(format!("unreadable message response: {error}"))
                }
            };
            // The optimistic message keeps its place in the transcript until the real one lands,
            // then it is replaced rather than joined by a duplicate — and a reply written to it
            // meanwhile resolves to the real one.
            let _ = store.upsert_messages(std::slice::from_ref(&sent));
            if let Some(temp_id) = &entry.temp_id {
                if temp_id != &sent.id {
                    let _ = store.delete_message(temp_id);
                    let _ = store.record_id_mapping(temp_id, &sent.id, chrono::Utc::now());
                }
            }
            Outcome::producing("messageId", &sent.id)
        }
        Err(error) => from_error(error),
    }
}

/// Deliver an invitation made offline, and put the server's answer — a member, or an invitation
/// waiting — where this device's queued one was.
async fn invite_to_list(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let list_id = match id(entry, "listId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client
        .post(endpoints::list_members(list_id))
        .value(with_client_request_id(
            body(entry),
            &entry.client_request_id,
        ));
    match client.send(request).await {
        Ok(answer) => {
            let text = |key: &str| entry.payload["body"][key].as_str().unwrap_or_default();
            crate::services::members::record_invite_answer(
                store,
                list_id,
                text("email"),
                text("role"),
                entry.temp_id.as_deref(),
                &answer,
            );
            Outcome::Done(None)
        }
        // The route has no idempotency key: a replay after a lost answer is told the person is
        // already invited or already on the list — which is what was asked for. The queued
        // invitation row goes; the next roster read shows the server's.
        Err(ApiError::Http {
            status: 400,
            message,
        }) if message.contains("already been sent")
            || message.contains("already a member")
            || message.contains("already the owner") =>
        {
            if let (Ok(Some(mut list)), Some(temp_id)) =
                (store.list(list_id), entry.temp_id.as_deref())
            {
                if let Some(invitations) = list.invitations.as_mut() {
                    invitations.retain(|invite| invite.id != temp_id);
                }
                let _ = store.upsert_list(&list);
            }
            Outcome::Done(None)
        }
        Err(error) => from_error(error),
    }
}

async fn set_member_role(client: &ApiClient, entry: &Entry) -> Outcome {
    let (Ok(list_id), Ok(user_id)) = (id(entry, "listId"), id(entry, "userId")) else {
        return Outcome::Dead("a role change that names no member".into());
    };
    let request = client
        .put(endpoints::list_member(list_id, user_id))
        .value(body(entry));
    done_or(client.send(request).await)
}

/// Removing somebody already gone is the outcome asked for.
async fn remove_member(client: &ApiClient, entry: &Entry) -> Outcome {
    let (Ok(list_id), Ok(user_id)) = (id(entry, "listId"), id(entry, "userId")) else {
        return Outcome::Dead("a removal that names no member".into());
    };
    gone_is_done(
        client
            .send(client.delete(endpoints::list_member(list_id, user_id)))
            .await,
    )
}

async fn cancel_invitation(client: &ApiClient, entry: &Entry) -> Outcome {
    let list_id = match id(entry, "listId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client
        .delete(endpoints::list_invitations(list_id))
        .value(body(entry));
    gone_is_done(client.send(request).await)
}

async fn set_invitation_role(client: &ApiClient, entry: &Entry) -> Outcome {
    let list_id = match id(entry, "listId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client
        .put(endpoints::list_invitations(list_id))
        .value(body(entry));
    done_or(client.send(request).await)
}

fn done_or(sent: Result<serde_json::Value, ApiError>) -> Outcome {
    match sent {
        Ok(_) => Outcome::Done(None),
        Err(error) => from_error(error),
    }
}

fn gone_is_done(sent: Result<serde_json::Value, ApiError>) -> Outcome {
    match sent {
        Err(error) if error.status() == Some(404) => Outcome::Done(None),
        other => done_or(other),
    }
}

/// Send a queued file, and hand its real id to whatever is waiting for it.
///
/// The comment carrying this attachment is already in the journal, naming the file by its
/// temporary id; recording the mapping is what turns that into the real one.
///
/// The copy on disk leaves the pending directory only on success or on a permanent failure — a
/// retry needs the bytes. On success it is **moved into the download cache** under the id the
/// server gave it rather than deleted: the screen that has just drawn it is still holding the
/// temporary id, and deleting the bytes made it fetch back the picture this device had that moment
/// uploaded (task 48f72aa7). `cacheDir` in the payload says where they belong; an entry queued by
/// a build that predates it simply deletes, as it always did.
async fn upload_attachment(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let payload = &entry.payload;
    let text = |key: &str| payload.get(key).and_then(|value| value.as_str());
    let (Some(path), Some(name)) = (text("localPath"), text("name")) else {
        return Outcome::Dead("an upload with no file to send".into());
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        // The copy is gone: a cache somebody cleared, or a half-finished sign-out. There is
        // nothing left to send and no amount of retrying will bring it back.
        Err(error) => return Outcome::Dead(format!("the queued file is gone: {error}")),
    };
    let mime = text("mimeType").unwrap_or("application/octet-stream");
    let mut context = payload
        .get("context")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    // What the upload route dedupes on: a retry after a lost answer gets the same file back
    // rather than a second blob and file row.
    if let Some(fields) = context.as_object_mut() {
        fields.insert(
            "clientRequestId".into(),
            serde_json::json!(entry.client_request_id),
        );
    }

    let boundary = format!("astrid-{}", entry.client_request_id);
    let body =
        crate::services::attachment::multipart(&boundary, name, mime, &bytes, &context.to_string());
    let request = client
        .post(endpoints::REQUEST_UPLOAD)
        .bytes(format!("multipart/form-data; boundary={boundary}"), body);

    match client.send(request).await {
        Ok(value) => {
            let Some(file_id) = value.get("fileId").and_then(|value| value.as_str()) else {
                return Outcome::Retry("the upload answered without a file id".into());
            };
            if let Some(temp_id) = &entry.temp_id {
                let _ = store.record_id_mapping(temp_id, file_id, chrono::Utc::now());
            }
            match text("cacheDir") {
                Some(cache_dir) => {
                    crate::services::attachment::promote(
                        std::path::Path::new(path),
                        std::path::Path::new(cache_dir),
                        file_id,
                        name,
                    );
                }
                None => {
                    let _ = std::fs::remove_file(path);
                }
            }
            Outcome::producing("fileId", file_id)
        }
        Err(error) => {
            let outcome = from_error(error);
            // Permanently refused — too large, or a list this account cannot write to. Keeping the
            // bytes would leave them in the cache directory for ever with nothing to send them.
            if matches!(outcome, Outcome::Dead(_)) {
                let _ = std::fs::remove_file(path);
            }
            outcome
        }
    }
}

async fn create_list(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let request = client.post(endpoints::LISTS).value(with_client_request_id(
        body(entry),
        &entry.client_request_id,
    ));
    match client.send(request).await {
        Ok(value) => {
            let created: TaskList =
                match serde_json::from_value(unwrap_envelope(value, endpoints::envelope::LIST)) {
                    Ok(list) => list,
                    Err(error) => {
                        return Outcome::Retry(format!("unreadable list response: {error}"))
                    }
                };
            if let Some(temp_id) = &entry.temp_id {
                let _ = store.record_id_mapping(temp_id, &created.id, chrono::Utc::now());
                if temp_id != &created.id {
                    let _ = store.delete_list(temp_id);
                }
            }
            let _ = store.upsert_list(&created);
            Outcome::producing("listId", &created.id)
        }
        Err(error) => from_error(error),
    }
}

/// A hand-arranged order, sent whole (task 7883f710).
///
/// The route answers `{ list, order, meta }`, and `order` is read rather than assumed: the server
/// drops ids of tasks that left the list, collapses repeats and appends what was not named, so
/// what it kept can differ from what was sent. Its answer is what the cache keeps, on the list.
async fn set_manual_order(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let list_id = match id(entry, "listId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let order = entry
        .payload
        .get("order")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let request = client
        .post(endpoints::manual_order(list_id))
        .value(serde_json::json!({ "order": order }));
    match client.send(request).await {
        Ok(value) => {
            // Only the order. The answer's list is the raw row (`lib/list-manual-order.ts`): no
            // per-person favourite or view, no roster names, no count — taken whole, it knocked
            // the list out of Favorites and blanked its members until the next pass.
            let kept: Option<Vec<String>> = value
                .get("order")
                .or_else(|| value.pointer("/list/manualSortOrder"))
                .cloned()
                .and_then(|order| serde_json::from_value(order).ok());
            if let (Some(order), Ok(Some(mut list))) = (kept, store.list(list_id)) {
                list.manual_sort_order = Some(order);
                let _ = store.upsert_list(&list);
            }
            Outcome::done()
        }
        Err(error) => from_error(error),
    }
}

async fn update_list(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let list_id = match id(entry, "listId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    let request = client.put(endpoints::list(list_id)).value(body(entry));
    match client.send(request).await {
        Ok(value) => {
            if let Ok(list) = serde_json::from_value::<TaskList>(unwrap_envelope(
                value,
                endpoints::envelope::LIST,
            )) {
                let _ = store.upsert_list(&list);
            }
            Outcome::done()
        }
        Err(error) => from_error(error),
    }
}

async fn delete_list(client: &ApiClient, store: &Store, entry: &Entry) -> Outcome {
    let list_id = match id(entry, "listId") {
        Ok(id) => id,
        Err(outcome) => return outcome,
    };
    match client.send(client.delete(endpoints::list(list_id))).await {
        Ok(_) => {
            let _ = store.delete_list(list_id);
            Outcome::done()
        }
        Err(error) if error.status() == Some(404) => {
            let _ = store.delete_list(list_id);
            Outcome::done()
        }
        Err(error) => from_error(error),
    }
}

/// Send the entry's body on a request that carries nothing else, and fold nothing back.
///
/// For the settings writes: the service merged the change into the cache when it was made, and
/// the next settings refresh reads the server's copy back. A settings write is a merge on the
/// server too, which is what makes replaying it a week later safe.
async fn send_body(client: &ApiClient, request: crate::api::Request, entry: &Entry) -> Outcome {
    match client.send(request.value(body(entry))).await {
        Ok(_) => Outcome::done(),
        Err(error) => from_error(error),
    }
}

/// Attach the idempotency key to a create body.
///
/// Set here rather than by the caller so no create can be enqueued without one. A create that
/// times out and is retried without a key is the duplicate-task bug, and it is invisible until
/// someone's network is bad.
fn with_client_request_id(
    mut body: serde_json::Value,
    client_request_id: &str,
) -> serde_json::Value {
    if let Some(object) = body.as_object_mut() {
        object.insert(
            "clientRequestId".to_string(),
            serde_json::Value::String(client_request_id.to_string()),
        );
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::StubTransport;
    use crate::model::date;
    use crate::platform::MemorySecureStore;
    use std::sync::Arc;

    fn t0() -> chrono::DateTime<chrono::Utc> {
        date::parse("2026-09-07T12:00:00Z").expect("an instant")
    }

    fn fixture(transport: StubTransport) -> (ApiClient, Store, Arc<StubTransport>) {
        let transport = Arc::new(transport);
        let client = ApiClient::new(
            "https://astrid.cc",
            transport.clone(),
            Arc::new(MemorySecureStore::new()),
        );
        (client, Store::in_memory().expect("opens"), transport)
    }

    fn entry(kind: &str, payload: serde_json::Value) -> Entry {
        Entry::new("e1", kind, payload, "temp_abc", t0())
    }

    /// A directory of this installation's own, so two tests cannot promote into each other.
    fn a_cache_dir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("astrid-cache-{}", crate::outbox::new_temp_id()));
        std::fs::create_dir_all(&dir).expect("creates");
        dir
    }

    fn a_queued_file(bytes: &[u8]) -> std::path::PathBuf {
        let held = std::env::temp_dir().join(format!("astrid-{}", crate::outbox::new_temp_id()));
        std::fs::write(&held, bytes).expect("writes");
        held
    }

    /// The temporary id is what the comment queued beside this names its file by, so the mapping
    /// is the whole point: without it the comment reaches the server naming a file id that never
    /// existed.
    #[tokio::test]
    async fn a_queued_upload_sends_the_file_and_hands_over_its_real_id() {
        let (client, store, transport) = fixture(StubTransport::new().push_json(
            "/api/v1/secure-upload/request-upload",
            200,
            serde_json::json!({ "fileId": "file_real" }),
        ));
        let held = a_queued_file(b"a receipt");

        let entry = entry(
            kind::UPLOAD_ATTACHMENT,
            serde_json::json!({
                "localPath": held.to_string_lossy(),
                "name": "receipt.png",
                "mimeType": "image/png",
                "context": { "listId": "l1" },
            }),
        )
        .for_temp_id("temp_abc");
        let outcome = perform(&client, &store, &entry).await;

        assert!(
            matches!(outcome, Outcome::Done(Some(ref result)) if result["fileId"] == "file_real")
        );
        assert_eq!(store.resolve_id("temp_abc").expect("resolves"), "file_real");
        assert!(!held.exists(), "the copy is not kept once it has been sent");

        let sent = transport.requests();
        let body = String::from_utf8_lossy(sent[0].body.as_deref().unwrap_or_default()).to_string();
        assert!(body.contains("receipt.png"), "{body}");
        assert!(body.contains("a receipt"), "{body}");
        assert!(body.contains("l1"), "the list that decides who may read it");
    }

    /// Task 48f72aa7: a pasted screenshot drew as a chip until the thread had been fetched and the
    /// picture downloaded again, from the machine that had just uploaded it — because delivering
    /// the upload DELETED the only copy on disk. The bytes move into the download cache under the
    /// id the server gave them instead, which is the first place `local_path` looks.
    #[tokio::test]
    async fn a_delivered_upload_keeps_its_bytes_in_the_cache_rather_than_deleting_them() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/api/v1/secure-upload/request-upload",
            200,
            serde_json::json!({ "fileId": "file_real" }),
        ));
        let cache = a_cache_dir();
        let held = cache.join("pending").join("temp_abc");
        std::fs::create_dir_all(held.parent().expect("a pending directory")).expect("creates");
        std::fs::write(&held, b"a screenshot").expect("writes");

        let entry = entry(
            kind::UPLOAD_ATTACHMENT,
            serde_json::json!({
                "localPath": held.to_string_lossy(),
                "name": "Screenshot 2026-09-26.png",
                "mimeType": "image/png",
                "cacheDir": cache.to_string_lossy(),
            }),
        )
        .for_temp_id("temp_abc");
        let outcome = perform(&client, &store, &entry).await;

        assert!(matches!(outcome, Outcome::Done(_)));
        assert!(!held.exists(), "the pending copy is not left behind");
        let promoted = cache.join("file_real.png");
        assert!(
            promoted.exists(),
            "the bytes belong in the cache under the id the server gave them",
        );
        assert_eq!(
            std::fs::read(&promoted).expect("reads"),
            b"a screenshot",
            "and they are the bytes this device attached, not a re-download",
        );
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    /// Journal rows written by a build with no `cacheDir` are already on disk in people's caches.
    /// They keep today's behaviour rather than promoting to a directory nobody named.
    #[tokio::test]
    async fn an_upload_queued_before_this_change_still_just_deletes_its_copy() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/api/v1/secure-upload/request-upload",
            200,
            serde_json::json!({ "fileId": "file_real" }),
        ));
        let held = a_queued_file(b"a receipt");

        let entry = entry(
            kind::UPLOAD_ATTACHMENT,
            serde_json::json!({
                "localPath": held.to_string_lossy(),
                "name": "receipt.png",
                "mimeType": "image/png",
            }),
        )
        .for_temp_id("temp_abc");

        assert!(matches!(
            perform(&client, &store, &entry).await,
            Outcome::Done(_)
        ));
        assert!(!held.exists());
    }

    /// A cache somebody cleared, or a half-finished sign-out. There is nothing left to send, and
    /// no amount of retrying brings it back.
    #[tokio::test]
    async fn an_upload_whose_copy_is_gone_is_dead_rather_than_retried_for_ever() {
        let (client, store, _) = fixture(StubTransport::new());

        let entry = entry(
            kind::UPLOAD_ATTACHMENT,
            serde_json::json!({
                "localPath": "C:\\nowhere\\nothing.png",
                "name": "nothing.png",
            }),
        );
        let outcome = perform(&client, &store, &entry).await;

        assert!(matches!(outcome, Outcome::Dead(_)));
    }

    /// Being offline is the case this whole path exists for: the bytes stay, and the next drain
    /// sends them.
    #[tokio::test]
    async fn an_upload_that_cannot_reach_the_server_keeps_the_file_for_next_time() {
        let (client, store, _) = fixture(StubTransport::new().fallback(Err(
            crate::api::TransportError::Unreachable("no network".into()),
        )));
        let held = a_queued_file(b"a receipt");

        let entry = entry(
            kind::UPLOAD_ATTACHMENT,
            serde_json::json!({
                "localPath": held.to_string_lossy(),
                "name": "receipt.png",
                "mimeType": "image/png",
            }),
        );
        let outcome = perform(&client, &store, &entry).await;

        assert!(matches!(outcome, Outcome::Offline(_)));
        assert!(held.exists(), "a retry needs the bytes");
        std::fs::remove_file(&held).expect("removes");
    }

    #[tokio::test]
    async fn creating_a_task_swaps_the_optimistic_row_for_the_real_one() {
        let (client, store, transport) = fixture(StubTransport::new().push_json(
            "/api/v1/tasks",
            200,
            serde_json::json!({ "task": { "id": "cm3real", "title": "Buy milk" } }),
        ));
        store
            .upsert_task(&Task::new("temp_abc", "Buy milk"))
            .expect("stores");

        let entry = entry(
            kind::CREATE_TASK,
            serde_json::json!({ "body": { "title": "Buy milk" } }),
        )
        .for_temp_id("temp_abc");
        let outcome = perform(&client, &store, &entry).await;

        assert!(
            matches!(outcome, Outcome::Done(Some(ref result)) if result["taskId"] == "cm3real")
        );
        assert!(store.task("temp_abc").expect("reads").is_none());
        assert_eq!(
            store
                .task("cm3real")
                .expect("reads")
                .expect("present")
                .title,
            "Buy milk"
        );
        // The mapping outlives the entry: an edit queued before the create landed still names the
        // temporary id.
        assert_eq!(store.resolve_id("temp_abc").expect("resolves"), "cm3real");

        // And the key went with it — without one, a retried create makes a second task.
        let body: serde_json::Value =
            serde_json::from_slice(transport.requests()[0].body.as_ref().expect("a body"))
                .expect("valid JSON");
        assert_eq!(body["clientRequestId"], "temp_abc");
    }

    /// A delete of something already gone is a delete that worked. Dead-lettering it leaves the
    /// row in the cache and shows the user a task they have deleted twice.
    /// A settings change made on a train goes out when the train does: the body the person
    /// set, on the settings route, and nothing else.
    #[tokio::test]
    async fn a_queued_settings_change_is_sent_as_it_was_made() {
        let (client, store, transport) = fixture(StubTransport::new().push_json(
            "/api/v1/users/me/settings",
            200,
            serde_json::json!({ "ok": true }),
        ));
        let outcome = perform(
            &client,
            &store,
            &entry(
                kind::UPDATE_SETTINGS,
                serde_json::json!({ "body": { "reminderSettings": { "enablePushReminders": false } } }),
            ),
        )
        .await;

        assert!(matches!(outcome, Outcome::Done(None)));
        let sent = transport.requests();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].url.ends_with("/api/v1/users/me/settings"));
        assert_eq!(sent[0].method.as_str(), "PUT");
        let body: serde_json::Value =
            serde_json::from_slice(sent[0].body.as_deref().unwrap_or(b"{}")).expect("json");
        assert_eq!(body["reminderSettings"]["enablePushReminders"], false);
    }

    #[tokio::test]
    async fn a_queued_smart_task_change_patches_its_route() {
        let (client, store, transport) = fixture(StubTransport::new().push_json(
            "/api/v1/users/me/smart-tasks",
            200,
            serde_json::json!({}),
        ));
        let outcome = perform(
            &client,
            &store,
            &entry(
                kind::UPDATE_SMART_TASKS,
                serde_json::json!({ "body": { "taskDisplayMode": "project" } }),
            ),
        )
        .await;

        assert!(matches!(outcome, Outcome::Done(None)));
        let sent = transport.requests();
        assert_eq!(sent[0].method.as_str(), "PATCH");
        assert!(sent[0].url.ends_with("/api/v1/users/me/smart-tasks"));
    }

    #[tokio::test]
    async fn deleting_something_that_is_already_gone_counts_as_done() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/api/v1/tasks/t1",
            404,
            serde_json::json!({ "error": "Not found" }),
        ));
        store.upsert_task(&Task::new("t1", "x")).expect("stores");

        let outcome = perform(
            &client,
            &store,
            &entry(kind::DELETE_TASK, serde_json::json!({ "taskId": "t1" })),
        )
        .await;
        assert!(matches!(outcome, Outcome::Done(_)));
        assert!(store.task("t1").expect("reads").is_none());
    }

    #[tokio::test]
    async fn a_refusal_is_dead_lettered_and_a_server_fault_is_retried() {
        let (client, store, _) = fixture(
            StubTransport::new()
                .push_json("/api/v1/tasks/t1", 403, serde_json::json!({}))
                .push_json("/api/v1/tasks/t1", 503, serde_json::json!({})),
        );
        let entry = entry(
            kind::UPDATE_TASK,
            serde_json::json!({ "taskId": "t1", "body": { "title": "x" } }),
        );
        assert!(matches!(
            perform(&client, &store, &entry).await,
            Outcome::Dead(_)
        ));
        assert!(matches!(
            perform(&client, &store, &entry).await,
            Outcome::Retry(_)
        ));
    }

    /// A payload missing the id it needs will still be missing it in five minutes.
    #[tokio::test]
    async fn an_unusable_payload_is_dead_lettered_rather_than_retried_forever() {
        let (client, store, transport) = fixture(StubTransport::new());
        let outcome = perform(
            &client,
            &store,
            &entry(kind::UPDATE_TASK, serde_json::json!({ "body": {} })),
        )
        .await;
        assert!(matches!(outcome, Outcome::Dead(_)));
        assert!(transport.requests().is_empty());
    }

    /// An entry from a newer build: dead-letter it with a reason, rather than retry a kind nothing
    /// here can run.
    #[tokio::test]
    async fn an_unknown_kind_is_dead_lettered_with_an_explanation() {
        let (client, store, _) = fixture(StubTransport::new());
        let outcome = perform(
            &client,
            &store,
            &entry("somethingLater", serde_json::json!({})),
        )
        .await;
        match outcome {
            Outcome::Dead(reason) => assert!(reason.contains("newer build"), "{reason}"),
            _ => panic!("an unknown kind must not be retried"),
        }
    }

    /// The transcript keeps its place: the optimistic message is replaced, not joined.
    #[tokio::test]
    async fn a_sent_message_replaces_the_optimistic_one() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/messages",
            200,
            serde_json::json!({ "message": { "id": "m1", "channelId": "c1", "content": "hi" } }),
        ));
        let optimistic: ChatMessage =
            serde_json::from_str(r#"{"id":"temp_abc","channelId":"c1","content":"hi"}"#)
                .expect("decodes");
        store.upsert_messages(&[optimistic]).expect("stores");

        let entry = entry(
            kind::SEND_CHAT_MESSAGE,
            serde_json::json!({ "channelId": "c1", "body": { "content": "hi" } }),
        )
        .for_temp_id("temp_abc");
        assert!(matches!(
            perform(&client, &store, &entry).await,
            Outcome::Done(Some(_))
        ));

        let ids: Vec<String> = store
            .messages_in_channel("c1")
            .expect("reads")
            .into_iter()
            .map(|message| message.id)
            .collect();
        assert_eq!(ids, vec!["m1"]);
    }

    /// Some deployments answer bare, some wrap. Both have been seen on the same route.
    #[tokio::test]
    async fn a_bare_response_is_read_as_readily_as_a_wrapped_one() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/api/v1/tasks",
            200,
            serde_json::json!({ "id": "cm3real", "title": "Buy milk" }),
        ));
        let entry = entry(
            kind::CREATE_TASK,
            serde_json::json!({ "body": { "title": "Buy milk" } }),
        );
        assert!(
            matches!(perform(&client, &store, &entry).await, Outcome::Done(Some(ref r)) if r["taskId"] == "cm3real")
        );
    }

    /// The reorder answer is the raw list row (`astrid-web lib/list-manual-order.ts`): no
    /// favourite, no member names, no count. Only its order is taken.
    #[tokio::test]
    async fn a_reorder_answer_changes_the_order_and_nothing_else() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/manual-order",
            200,
            serde_json::json!({ "list": {
                "id": "l1", "name": "Home", "manualSortOrder": ["t2", "t1"],
                "listMembers": [{ "userId": "u2", "role": "member" }]
            } }),
        ));
        let cached: TaskList = serde_json::from_value(serde_json::json!({
            "id": "l1", "name": "Home", "isFavorite": true, "taskCount": 12,
            "manualSortOrder": ["t1", "t2"],
            "listMembers": [{ "userId": "u2", "role": "member", "user": { "id": "u2", "name": "Dana" } }]
        }))
        .expect("a list");
        store.upsert_list(&cached).expect("stores");

        let outcome = perform(
            &client,
            &store,
            &entry(
                kind::SET_MANUAL_ORDER,
                serde_json::json!({ "listId": "l1", "order": ["t2", "t1"] }),
            ),
        )
        .await;
        assert!(matches!(outcome, Outcome::Done(_)));
        let list = store.list("l1").expect("reads").expect("kept");
        assert_eq!(
            list.manual_sort_order,
            Some(vec!["t2".to_string(), "t1".to_string()])
        );
        assert_eq!(list.is_favorite, Some(true), "still a favourite");
        let members = list.list_members.expect("members");
        assert!(members[0].user.is_some(), "the roster keeps its names");
    }

    /// An edit's answer does not carry the blocker ids; it is not saying the task stopped waiting.
    #[tokio::test]
    async fn an_edits_answer_keeps_what_the_task_is_waiting_on() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/api/v1/tasks/t1",
            200,
            serde_json::json!({ "task": { "id": "t1", "title": "Renamed" } }),
        ));
        let mut cached = Task::new("t1", "Old");
        cached.blocked_by = Some(vec!["t9".to_string()]);
        store.upsert_task(&cached).expect("stores");
        perform(
            &client,
            &store,
            &entry(
                kind::UPDATE_TASK,
                serde_json::json!({ "taskId": "t1", "body": { "title": "Renamed" } }),
            ),
        )
        .await;
        let task = store.task("t1").expect("reads").expect("kept");
        assert_eq!(task.title, "Renamed");
        assert_eq!(task.blocked_by, Some(vec!["t9".to_string()]));
    }

    /// A replayed invitation is told it was already sent (the route has no idempotency key). That
    /// is what was asked for, not a failure to dead-letter.
    #[tokio::test]
    async fn a_replayed_invitation_that_already_landed_is_done() {
        let (client, store, _) = fixture(StubTransport::new().push_json(
            "/api/v1/lists/l1/members",
            400,
            serde_json::json!({ "error": "An invitation has already been sent to this email" }),
        ));
        let mut invite = entry(
            kind::INVITE_TO_LIST,
            serde_json::json!({ "listId": "l1", "body": { "email": "ada@example.com", "role": "member" } }),
        );
        invite.temp_id = Some("temp_i".into());
        assert!(matches!(
            perform(&client, &store, &invite).await,
            Outcome::Done(_)
        ));
    }

    /// The upload route dedupes on the context's clientRequestId; without it a retry after a lost
    /// answer uploaded the file twice.
    #[tokio::test]
    async fn an_upload_carries_its_idempotency_key() {
        let held = std::env::temp_dir().join(format!("astrid-{}", crate::outbox::new_temp_id()));
        std::fs::write(&held, b"bytes").expect("writes");
        let (client, store, transport) = fixture(StubTransport::new().push_json(
            "/request-upload",
            200,
            serde_json::json!({ "fileId": "f1" }),
        ));
        let upload = Entry::new(
            "e1",
            kind::UPLOAD_ATTACHMENT,
            serde_json::json!({
                "localPath": held.to_string_lossy(), "name": "a.png", "mimeType": "image/png",
                "context": { "listId": "l1" }
            }),
            "temp_upload_key",
            t0(),
        );
        perform(&client, &store, &upload).await;
        let sent = transport.requests();
        let body = String::from_utf8_lossy(sent[0].body.as_deref().expect("a body")).to_string();
        assert!(body.contains("temp_upload_key"), "{body}");
    }
}
