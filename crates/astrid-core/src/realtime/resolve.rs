//! Turning an event that *names* a row into the event that carries it.
//!
//! The web sends some changes as references: a task event carries the task's id beside a lean
//! projection built for agents (no assignee, no repeat, no lists); a list event carries a row
//! thinner than the list this account sees; a membership event names only the list; an agent's
//! reply arrives as a preview. Applying any of those as if it were the row wiped fields on this
//! machine or was dropped outright. So the stream fetches the row — through the same endpoints a
//! sync pass reads — and applies that, under the same stale-event and deleted-here guards.
//!
//! A newer server also sends the row itself beside the reference (`v1Task`, `v1List`); the parser
//! reads that as the fetched row, so only an event without one reaches here as a reference.

use super::parse::{Event, EventKind};
use crate::api::{endpoints, ApiClient, ApiError};
use crate::model::{Comment, Task, TaskList};

/// The events `event` stands for once its references are fetched. An event that already carries
/// its row is itself. A fetch the server refuses as gone (403/404/410) becomes a deletion — for a
/// list, that is how this account learns it was removed from it. Any other failure leaves it for
/// the next sync pass.
pub async fn resolve(client: &ApiClient, event: Event) -> Vec<Event> {
    let kind = match event.kind {
        EventKind::TaskTouched(id) => match fetch(client, endpoints::task(&id), "task").await {
            Fetched::Row(value) => serde_json::from_value::<Task>(value)
                .ok()
                .map(EventKind::TaskUpdated),
            Fetched::Gone => Some(EventKind::TaskDeleted(id)),
            Fetched::Unknown => None,
        },
        EventKind::ListTouched(id) => match fetch(client, endpoints::list(&id), "list").await {
            Fetched::Row(value) => serde_json::from_value::<TaskList>(value)
                .ok()
                .map(EventKind::ListUpdated),
            Fetched::Gone => Some(EventKind::ListDeleted(id)),
            Fetched::Unknown => None,
        },
        EventKind::CommentsTouched(task_id) => {
            let request = client.get(endpoints::task_comments(&task_id));
            let Ok(comments) = client
                .send_collection::<Comment>(request, Some(endpoints::envelope::COMMENTS))
                .await
            else {
                return Vec::new();
            };
            return comments
                .into_items()
                .into_iter()
                .map(|comment| Event {
                    kind: EventKind::CommentAdded(comment),
                })
                .collect();
        }
        other => Some(other),
    };
    kind.map(|kind| vec![Event { kind }]).unwrap_or_default()
}

enum Fetched {
    Row(serde_json::Value),
    Gone,
    Unknown,
}

async fn fetch(client: &ApiClient, path: String, envelope: &str) -> Fetched {
    match client.send(client.get(path)).await {
        Ok(answer) => Fetched::Row(answer.get(envelope).cloned().unwrap_or(answer)),
        Err(ApiError::Http {
            status: 403 | 404 | 410,
            ..
        }) => Fetched::Gone,
        Err(_) => Fetched::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::StubTransport;
    use crate::platform::MemorySecureStore;
    use serde_json::json;
    use std::sync::Arc;

    fn client(transport: StubTransport) -> ApiClient {
        ApiClient::new(
            "https://astrid.cc",
            Arc::new(transport),
            Arc::new(MemorySecureStore::new()),
        )
    }

    fn event(frame: serde_json::Value) -> Event {
        super::super::parse::parse(&format!("data: {frame}\n\n")).expect("parses")
    }

    /// The web's task event (`services/task.service.ts`): the id, and a lean agent projection
    /// that is not the task. Read as the task, every one was dropped; applied, it would wipe the
    /// assignee, repeat and lists. The task is fetched.
    #[tokio::test]
    async fn a_task_event_is_the_task_fetched_by_its_id() {
        let client = client(StubTransport::new().push_json(
            "/api/v1/tasks/t1",
            200,
            json!({ "task": { "id": "t1", "title": "Edited elsewhere", "assigneeId": "u1" } }),
        ));
        for kind in [
            "task_created",
            "task_updated",
            "task_completed",
            "task_assigned",
        ] {
            let parsed = event(json!({ "type": kind, "data": {
                "taskId": "t1", "task": { "id": "t1", "title": "lean" }
            } }));
            assert_eq!(parsed.kind, EventKind::TaskTouched("t1".into()), "{kind}");
        }
        let resolved = resolve(
            &client,
            event(json!({ "type": "task_updated", "data": {
            "taskId": "t1", "task": { "id": "t1", "title": "lean" }
        } })),
        )
        .await;
        match &resolved[..] {
            [Event {
                kind: EventKind::TaskUpdated(task),
            }] => {
                assert_eq!(task.title, "Edited elsewhere");
                assert_eq!(task.assignee_id.as_deref(), Some("u1"));
            }
            other => panic!("expected the fetched task, got {other:?}"),
        }
    }

    /// AITD-444: an event carrying `v1Task` is applied without a request — the stub answers
    /// nothing, so a fetch would come back empty.
    #[tokio::test]
    async fn aitd_444_a_task_event_carrying_v1_task_is_not_fetched() {
        let client = client(StubTransport::new());
        let resolved = resolve(
            &client,
            event(json!({ "type": "task_completed", "data": {
                "taskId": "t1",
                "task": { "id": "t1", "title": "lean" },
                "v1Task": { "id": "t1", "title": "Whole", "completed": true }
            } })),
        )
        .await;
        match &resolved[..] {
            [Event {
                kind: EventKind::TaskUpdated(task),
            }] => assert_eq!(task.title, "Whole"),
            other => panic!("expected the v1 task, got {other:?}"),
        }
    }

    #[test]
    fn deletions_are_read_by_the_names_the_web_uses() {
        let task =
            event(json!({ "type": "task_deleted", "data": { "taskId": "t1", "taskTitle": "x" } }));
        assert_eq!(task.kind, EventKind::TaskDeleted("t1".into()));
        let list =
            event(json!({ "type": "list_deleted", "data": { "listId": "l1", "listName": "x" } }));
        assert_eq!(list.kind, EventKind::ListDeleted("l1".into()));
    }

    /// Removed from a list elsewhere: the membership event names the list, the fetch is refused,
    /// and the list goes from this machine now rather than at the next sync.
    #[tokio::test]
    async fn being_removed_from_a_list_removes_it_here() {
        let client = client(StubTransport::new().push_json(
            "/api/v1/lists/l1",
            403,
            json!({ "error": "no access" }),
        ));
        let parsed = event(json!({ "type": "list_member_removed", "data": {
            "listId": "l1", "memberId": "me"
        } }));
        let resolved = resolve(&client, parsed).await;
        assert_eq!(resolved[0].kind, EventKind::ListDeleted("l1".into()));
    }

    /// An agent's reply arrives as a preview with no row: the thread is fetched.
    #[tokio::test]
    async fn an_agents_reply_is_fetched_with_its_thread() {
        let client = client(StubTransport::new().push_json(
            "/api/v1/tasks/t1/comments",
            200,
            json!({ "comments": [{ "id": "c1", "taskId": "t1", "content": "Done." }] }),
        ));
        let parsed = event(json!({ "type": "comment_created", "data": {
            "taskId": "t1", "commentId": "c1", "commentContent": "Done."
        } }));
        assert_eq!(parsed.kind, EventKind::CommentsTouched("t1".into()));
        let resolved = resolve(&client, parsed).await;
        assert!(
            matches!(&resolved[..], [Event { kind: EventKind::CommentAdded(c) }] if c.id == "c1")
        );
    }
}
