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
        kind::CREATE_COMMENT => create_comment(client, store, entry).await,
        kind::UPDATE_COMMENT => update_comment(client, store, entry).await,
        kind::DELETE_COMMENT => delete_comment(client, store, entry).await,
        kind::SEND_CHAT_MESSAGE => send_chat_message(client, store, entry).await,
        kind::CREATE_LIST => create_list(client, store, entry).await,
        kind::UPDATE_LIST => update_list(client, store, entry).await,
        kind::DELETE_LIST => delete_list(client, store, entry).await,
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
            // then it is replaced rather than joined by a duplicate.
            if let Some(temp_id) = &entry.temp_id {
                if temp_id != &sent.id {
                    let _ = store.delete_message(temp_id);
                }
            }
            let _ = store.upsert_messages(std::slice::from_ref(&sent));
            Outcome::producing("messageId", &sent.id)
        }
        Err(error) => from_error(error),
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
}
