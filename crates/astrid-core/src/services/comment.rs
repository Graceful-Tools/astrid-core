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
        // What was here before asking: only those can be missing from the answer because they
        // were deleted. One that arrived meanwhile — from the stream, or a create delivered while
        // the request was out — is newer than the answer, not absent from it.
        let held_before: std::collections::HashSet<String> = self
            .context
            .store
            .comments_for_task(task_id)?
            .into_iter()
            .map(|comment| comment.id)
            .collect();
        let request = self.context.client.get(endpoints::task_comments(task_id));
        let fetched = self
            .context
            .client
            .send_collection::<Comment>(request, Some(endpoints::envelope::COMMENTS))
            .await?;
        let rows = fetched.into_items();

        // An edit or a delete still on its way is what the person last saw: the server's copy of
        // that comment neither overwrites the edit nor brings the deleted one back.
        let touched = journal::ids_named(
            &self.context.store,
            Some(&[kind::UPDATE_COMMENT, kind::DELETE_COMMENT]),
            "commentId",
            false,
        )?;
        let incoming: Vec<Comment> = rows
            .iter()
            .filter(|comment| !touched.contains(&comment.id))
            .cloned()
            .collect();
        self.context.store.upsert_comments(&incoming)?;

        // The list is the whole thread, so a delivered comment it does not return was deleted
        // elsewhere. One still on its way is not evidence of anything (AITD-354).
        let returned: std::collections::HashSet<&str> =
            rows.iter().map(|comment| comment.id.as_str()).collect();
        for cached in self.context.store.comments_for_task(task_id)? {
            if !crate::model::is_temp_id(&cached.id)
                && held_before.contains(&cached.id)
                && !returned.contains(cached.id.as_str())
                && !touched.contains(&cached.id)
            {
                self.context.store.delete_comment(&cached.id)?;
            }
        }
        Ok(rows)
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
        self.post_under(task_id, content, author_id, comment_type, file, None)
    }

    /// Say something on a task — possibly in answer to another comment (task 97c817dd).
    ///
    /// A reply is a comment with a parent, on the wire and in the cache, so it nests under the
    /// comment it answers the moment it is posted and reaches the server through the Outbox like
    /// any other.
    pub fn post_under(
        &self,
        task_id: &str,
        content: &str,
        author_id: Option<&str>,
        comment_type: CommentType,
        file: Option<&crate::model::SecureFile>,
        parent_comment_id: Option<&str>,
    ) -> Result<Comment> {
        self.post_as(
            task_id,
            content,
            author_id,
            comment_type,
            file,
            parent_comment_id,
            None,
        )
    }

    /// [`Self::post_under`], under an id the caller chose — the row a shell already drew for the
    /// comment, so the one it draws next is the same row rather than a second (Apple AITD-331).
    /// It must be a temporary id; anything else is replaced with a fresh one, because a real-
    /// looking id on an unsent comment would be mistaken for the server's.
    #[allow(clippy::too_many_arguments)]
    pub fn post_as(
        &self,
        task_id: &str,
        content: &str,
        author_id: Option<&str>,
        comment_type: CommentType,
        file: Option<&crate::model::SecureFile>,
        parent_comment_id: Option<&str>,
        id: Option<&str>,
    ) -> Result<Comment> {
        let now = self.context.clock.now();
        let temp_id = id
            .filter(|id| crate::model::is_temp_id(id))
            .map(str::to_string)
            .unwrap_or_else(outbox::new_temp_id);

        let optimistic: Comment = serde_json::from_value(json!({
            "id": temp_id,
            "taskId": task_id,
            "content": content,
            "type": comment_type,
            "authorId": author_id,
            "parentCommentId": parent_comment_id,
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
                    "parentCommentId": parent_comment_id,
                    // When it was written, not when it was delivered: the route keeps it "for
                    // offline-first ordering", so a comment typed offline keeps its place.
                    "createdAt": crate::model::date::format(now),
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

    /// A task's comment list is the whole thread, so a comment the server no longer returns was
    /// deleted — on the web, or on another device while this one's stream was down. It goes.
    /// A comment still on its way never does (the Apple apps' `CommentCachePruner`, AITD-354).
    #[tokio::test]
    async fn a_refresh_forgets_comments_deleted_elsewhere_but_not_unsent_ones() {
        let fixture = fixture(StubTransport::new().push_json(
            "/comments",
            200,
            json!({ "comments": [{ "id": "c2", "taskId": "t1", "content": "kept" }] }),
        ));
        let cached: Vec<Comment> = serde_json::from_value(json!([
            { "id": "c1", "taskId": "t1", "content": "deleted on the web" },
            { "id": "c2", "taskId": "t1", "content": "kept" },
            { "id": "c9", "taskId": "t2", "content": "another task" }
        ]))
        .expect("decodes");
        fixture.store.upsert_comments(&cached).expect("stores");
        let unsent = fixture
            .service
            .post("t1", "on its way", None, CommentType::Text, None)
            .expect("posts");

        fixture.service.refresh("t1").await.expect("refreshes");
        let ids: Vec<String> = fixture
            .service
            .for_task("t1")
            .expect("reads")
            .into_iter()
            .map(|comment| comment.id)
            .collect();
        assert!(ids.contains(&"c2".to_string()));
        assert!(ids.contains(&unsent.id));
        assert!(!ids.contains(&"c1".to_string()), "{ids:?}");
        assert_eq!(fixture.service.for_task("t2").expect("reads").len(), 1);
    }

    /// An edit waiting to be sent is what the person last saw; the server's older copy of the
    /// comment does not overwrite it.
    #[tokio::test]
    async fn a_refresh_does_not_undo_an_edit_on_its_way() {
        let fixture = fixture(StubTransport::new().push_json(
            "/comments",
            200,
            json!({ "comments": [{ "id": "c1", "taskId": "t1", "content": "before" }] }),
        ));
        let cached: Vec<Comment> =
            serde_json::from_value(json!([{ "id": "c1", "taskId": "t1", "content": "before" }]))
                .expect("decodes");
        fixture.store.upsert_comments(&cached).expect("stores");
        fixture.service.edit("c1", "after").expect("edits");

        fixture.service.refresh("t1").await.expect("refreshes");
        assert_eq!(
            fixture.service.for_task("t1").expect("reads")[0].content,
            "after"
        );
    }

    /// The stream can bring the server's copy of a comment before the post's own answer does. It
    /// replaces the optimistic comment it echoes rather than joining it.
    #[test]
    fn the_servers_copy_replaces_the_comment_it_echoes() {
        let fixture = fixture(StubTransport::new());
        let posted = fixture
            .service
            .post("t1", "on it", None, CommentType::Text, None)
            .expect("posts");
        let echoed: Comment = serde_json::from_value(json!({
            "id": "c1", "taskId": "t1", "content": "on it", "clientRequestId": posted.id
        }))
        .expect("decodes");
        fixture
            .store
            .upsert_comments(std::slice::from_ref(&echoed))
            .expect("stores");
        let ids: Vec<String> = fixture
            .service
            .for_task("t1")
            .expect("reads")
            .into_iter()
            .map(|comment| comment.id)
            .collect();
        assert_eq!(ids, vec!["c1"]);
        assert_eq!(
            fixture.store.resolve_id(&posted.id).expect("resolves"),
            "c1"
        );
    }

    /// A comment that arrives while the refresh is in flight — the stream, or a delivered create
    /// — is not in an answer that predates it, and is not deleted for it.
    #[tokio::test]
    async fn a_comment_that_arrives_mid_refresh_is_not_pruned() {
        let fixture =
            fixture(StubTransport::new().push_json("/comments", 200, json!({ "comments": [] })));
        let arrived: Comment =
            serde_json::from_value(json!({ "id": "c-new", "taskId": "t1", "content": "hi" }))
                .expect("decodes");
        // The stream lands a comment between the snapshot and the answer.
        let (outcome, ()) = tokio::join!(fixture.service.refresh("t1"), async {
            fixture
                .store
                .upsert_comments(std::slice::from_ref(&arrived))
                .expect("stores");
        });
        outcome.expect("refreshes");
        assert_eq!(fixture.service.for_task("t1").expect("reads").len(), 1);
    }
}
