//! Comments on a task.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/CommentService.swift`.
//!
//! A comment written offline appears in the thread straight away, under a `temp_` id, and keeps
//! its place until the Outbox delivers it. What it must never do is appear twice — which is what
//! happens when a send times out, the client cannot tell whether it landed, and the retry has no
//! idempotency key. The entry's `client_request_id` is that key, and the server echoes it back on
//! the created comment so a comment arriving by sync can be matched to the optimistic one.

use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;
use crate::model::{Comment, CommentType};
use crate::outbox::{self, journal, kind};

pub struct CommentService {
    context: Context,
}

impl CommentService {
    pub fn new(context: Context) -> Self {
        CommentService { context }
    }

    /// A task's comments, oldest first.
    pub fn for_task(&self, task_id: &str) -> Result<Vec<Comment>> {
        Ok(self.context.store.comments_for_task(task_id)?)
    }

    /// Fetch the thread from the server and replace what is cached for that task.
    pub async fn refresh(&self, task_id: &str) -> Result<Vec<Comment>> {
        let request = self.context.client.get(endpoints::task_comments(task_id));
        let fetched = self
            .context
            .client
            .send_collection::<Comment>(request, Some(endpoints::envelope::COMMENTS))
            .await?;
        self.context.store.upsert_comments(&fetched.items)?;
        Ok(fetched.into_items())
    }

    /// Post a comment. It is in the thread before this returns.
    /// Say something on a task.
    ///
    /// `file` is an already-uploaded attachment. The comment carries its id and the server resolves
    /// it — there is no "attach to task" endpoint, which is why a file always arrives this way.
    pub fn post(
        &self,
        task_id: &str,
        content: &str,
        author_id: Option<&str>,
        comment_type: CommentType,
        file: Option<&crate::model::SecureFile>,
    ) -> Result<Comment> {
        let now = self.context.clock.now();
        let temp_id = outbox::new_temp_id();

        let optimistic: Comment = serde_json::from_value(json!({
            "id": temp_id,
            "taskId": task_id,
            "content": content,
            "type": comment_type,
            "authorId": author_id,
            "createdAt": crate::model::date::format(now),
            "clientRequestId": temp_id,
            // Carried on the optimistic comment so the attachment is on screen the moment it is
            // posted rather than after the next fetch.
            "secureFiles": file.map(std::slice::from_ref),
        }))
        .expect("a comment built from known fields always decodes");
        self.context
            .store
            .upsert_comments(std::slice::from_ref(&optimistic))?;

        let entry = outbox::build(
            kind::CREATE_COMMENT,
            json!({
                "taskId": task_id,
                "body": {
                    "content": content,
                    "type": comment_type,
                    "fileId": file.map(|file| file.id.clone()),
                }
            }),
            &temp_id,
            now,
        )
        .for_temp_id(&temp_id);
        journal::enqueue(&self.context.store, &entry)?;

        Ok(optimistic)
    }

    pub fn edit(&self, comment_id: &str, content: &str) -> Result<()> {
        let now = self.context.clock.now();
        if let Some(mut comment) = self.context.store.comment(comment_id)? {
            comment.content = content.to_string();
            comment.updated_at = Some(now);
            self.context
                .store
                .upsert_comments(std::slice::from_ref(&comment))?;
        }

        let entry = outbox::build(
            kind::UPDATE_COMMENT,
            json!({ "commentId": comment_id, "body": { "content": content } }),
            &outbox::new_temp_id(),
            now,
        );
        let entry = match crate::model::is_temp_id(comment_id) {
            true => entry.for_temp_id(comment_id),
            false => entry,
        };
        journal::enqueue(&self.context.store, &entry)?;
        Ok(())
    }

    pub fn delete(&self, comment_id: &str) -> Result<()> {
        let now = self.context.clock.now();
        self.context.store.delete_comment(comment_id)?;

        let entry = outbox::build(
            kind::DELETE_COMMENT,
            json!({ "commentId": comment_id }),
            &outbox::new_temp_id(),
            now,
        );
        let entry = match crate::model::is_temp_id(comment_id) {
            true => entry.for_temp_id(comment_id),
            false => entry,
        };
        journal::enqueue(&self.context.store, &entry)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport};
    use crate::platform::{FixedClock, MemorySecureStore};
    use crate::store::Store;
    use std::sync::Arc;

    struct Fixture {
        service: CommentService,
        store: Arc<Store>,
    }

    fn fixture(transport: StubTransport) -> Fixture {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                Arc::new(transport),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        );
        Fixture {
            service: context.comments(),
            store,
        }
    }

    #[test]
    fn a_comment_is_in_the_thread_before_it_is_sent() {
        let fixture = fixture(StubTransport::new());
        let posted = fixture
            .service
            .post("t1", "on it", Some("u1"), CommentType::Text, None)
            .expect("posts");

        assert!(crate::model::is_temp_id(&posted.id));
        assert_eq!(fixture.service.for_task("t1").expect("reads").len(), 1);

        let entries = journal::all(&fixture.store).expect("reads");
        assert_eq!(entries[0].kind, kind::CREATE_COMMENT);
        assert_eq!(entries[0].payload["taskId"], "t1");
        assert_eq!(entries[0].payload["body"]["content"], "on it");
    }

    /// The key the server echoes back. Without it, a send that times out and is retried posts the
    /// comment twice, and the thread shows it twice.
    #[test]
    fn the_optimistic_comment_carries_the_key_the_retry_will_use() {
        let fixture = fixture(StubTransport::new());
        let posted = fixture
            .service
            .post("t1", "on it", Some("u1"), CommentType::Text, None)
            .expect("posts");
        assert_eq!(
            posted.client_request_id.as_deref(),
            Some(posted.id.as_str())
        );
        assert_eq!(
            journal::all(&fixture.store).expect("reads")[0].client_request_id,
            posted.id
        );
    }

    #[test]
    fn deleting_a_comment_takes_it_out_of_the_thread_at_once() {
        let fixture = fixture(StubTransport::new());
        let posted = fixture
            .service
            .post("t1", "oops", Some("u1"), CommentType::Text, None)
            .expect("posts");
        fixture.service.delete(&posted.id).expect("deletes");

        assert!(fixture.service.for_task("t1").expect("reads").is_empty());
        let kinds: Vec<String> = journal::all(&fixture.store)
            .expect("reads")
            .into_iter()
            .map(|entry| entry.kind)
            .collect();
        assert_eq!(kinds, vec![kind::CREATE_COMMENT, kind::DELETE_COMMENT]);
    }

    /// A comment deleted before its create was delivered has to strand with it, or the delete goes
    /// to an id the server has never issued.
    #[test]
    fn a_delete_before_the_create_landed_travels_in_the_creates_lane() {
        let fixture = fixture(StubTransport::new());
        let posted = fixture
            .service
            .post("t1", "oops", Some("u1"), CommentType::Text, None)
            .expect("posts");
        fixture.service.delete(&posted.id).expect("deletes");

        let entries = journal::all(&fixture.store).expect("reads");
        assert_eq!(entries[1].temp_id.as_deref(), Some(posted.id.as_str()));
        assert_eq!(
            entries[0].serialization_key(),
            entries[1].serialization_key()
        );
    }

    #[tokio::test]
    async fn refreshing_replaces_what_is_cached_for_that_task() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/tasks/t1/comments",
            200,
            json!({ "comments": [
                { "id": "c1", "taskId": "t1", "content": "first", "createdAt": "2026-09-06T12:00:00Z" },
                { "id": "c2", "taskId": "t1", "content": "second", "createdAt": "2026-09-07T12:00:00Z" }
            ] }),
        ));
        let fetched = fixture.service.refresh("t1").await.expect("refreshes");
        assert_eq!(fetched.len(), 2);
        let ids: Vec<String> = fixture
            .service
            .for_task("t1")
            .expect("reads")
            .into_iter()
            .map(|comment| comment.id)
            .collect();
        assert_eq!(ids, vec!["c1", "c2"]);
    }

    /// One unreadable comment must not empty the thread.
    #[tokio::test]
    async fn a_thread_with_one_bad_row_still_shows_the_rest() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/tasks/t1/comments",
            200,
            json!({ "comments": [
                { "id": "c1", "taskId": "t1", "content": "fine" },
                { "taskId": "t1", "content": "no id at all" }
            ] }),
        ));
        assert_eq!(
            fixture
                .service
                .refresh("t1")
                .await
                .expect("refreshes")
                .len(),
            1
        );
    }
}
