//! Live updates: the server-sent-events stream, and what arrives on it.
//!
//! Ported from `astrid-ios/Astrid App/Core/RealTime/SSEClient.swift` and `SSEReconnectPolicy.swift`.
//!
//! The stream is an optimisation, never a source of truth. Everything it delivers also arrives on
//! the next sync pass, which is what lets it be dropped, reconnected, or missing entirely on a
//! network that blocks long-lived connections without the app being wrong — only slower. Anything
//! that depended on the stream having been connected would be broken on exactly the networks where
//! it matters most.
//!
//! Two things here are pure, and both were bugs before they were rules: [`reconnect`] decides when
//! to try again, and [`parse`] turns a wire frame into an [`Event`]. Neither needs a socket to test.

pub mod parse;
pub mod reconnect;
pub mod stream;

pub use parse::{Event, EventKind, Frame, Typing};
pub use reconnect::ReconnectPolicy;
pub use stream::{run, Stopped};

use std::sync::Arc;

use crate::model::{ChatMessage, Comment, Task, TaskList};
use crate::store::Store;

/// Where the stream lives. `/api/v1/sse`, matching `Constants.API.sseEndpoint` on Apple.
pub const SSE_PATH: &str = "/api/v1/sse";

/// Fold an event into the cache.
///
/// Returns what changed, so the shell can refresh exactly that rather than redrawing everything —
/// a stream that arrives every few seconds and triggers a full reload is worse than no stream.
///
/// A delivery that cannot be applied is dropped rather than escalated: the next sync pass carries
/// the same change, so the cost of ignoring a malformed frame is a few seconds of staleness, and
/// the cost of failing loudly on one is a reconnect loop.
pub fn apply(store: &Store, event: &Event) -> Option<Change> {
    match &event.kind {
        EventKind::TaskCreated(task) | EventKind::TaskUpdated(task) => {
            // Deleted here and the delete is on its way: a late event must not bring it back.
            let deleting = crate::outbox::journal::ids_named(
                store,
                Some(&[crate::outbox::kind::DELETE_TASK]),
                "taskId",
                true,
            )
            .ok()?;
            if deleting.contains(&task.id) {
                return None;
            }
            match store.task(&task.id).ok()? {
                // Older than what is here — the echo of an edit a newer one has overtaken. Keep
                // the newer; its own event, or the next pass, brings the server's word on it.
                Some(cached) if is_older(task.updated_at, cached.updated_at) => return None,
                // Both may have moved: the same field-by-field rule the sync pass applies.
                Some(cached) => store
                    .upsert_task(&crate::sync::conflict::resolve(&cached, task))
                    .ok()?,
                None => store.upsert_task(task).ok()?,
            }
            Some(Change::Task(task.id.clone()))
        }
        EventKind::TaskDeleted(id) => {
            store.delete_task(id).ok()?;
            Some(Change::Task(id.clone()))
        }
        EventKind::ListCreated(list) | EventKind::ListUpdated(list) => {
            store.upsert_list(list).ok()?;
            Some(Change::List(list.id.clone()))
        }
        EventKind::ListDeleted(id) => {
            store.delete_list(id).ok()?;
            Some(Change::List(id.clone()))
        }
        EventKind::CommentAdded(comment) => {
            store.upsert_comments(std::slice::from_ref(comment)).ok()?;
            Some(Change::Comments(comment.task_id.clone()))
        }
        // An edit says what the comment says now, not who wrote it or what it carries — those
        // stay as this device has them (Apple AITD-331).
        EventKind::CommentUpdated(edit) => {
            let merged = match store.comment(&edit.id).ok().flatten() {
                Some(mut cached) => {
                    cached.content = edit.content.clone();
                    cached.r#type = edit.r#type;
                    if edit.updated_at.is_some() {
                        cached.updated_at = edit.updated_at;
                    }
                    cached
                }
                None => edit.clone(),
            };
            store.upsert_comments(std::slice::from_ref(&merged)).ok()?;
            Some(Change::Comments(merged.task_id.clone()))
        }
        EventKind::CommentDeleted { id, task_id } => {
            store.delete_comment(id).ok()?;
            Some(Change::Comments(task_id.clone()))
        }
        EventKind::ChatMessageCreated(message) | EventKind::ChatMessageUpdated(message) => {
            store.upsert_messages(std::slice::from_ref(message)).ok()?;
            Some(Change::Chat(message.channel_id.clone()))
        }
        EventKind::ChatMessageDeleted { id, channel_id } => {
            store.delete_message(id).ok()?;
            Some(Change::Chat(channel_id.clone()))
        }
        EventKind::AgentTyping(typing) => Some(Change::AgentTyping(typing.clone())),
        EventKind::SettingsUpdated => Some(Change::Settings),
        EventKind::ExternalSyncRefresh => Some(Change::NeedsSync),
        // A heartbeat means the connection is alive, which is worth nothing to a reader.
        EventKind::Connected | EventKind::Ping => None,
        // An event from a newer server. Not an error: the next sync pass will carry whatever it
        // was about.
        EventKind::Unknown(_) => None,
    }
}

/// Whether `incoming` is strictly older than `cached`. Equal is not older: the server's copy of the
/// same moment is the one to keep.
fn is_older(
    incoming: Option<chrono::DateTime<chrono::Utc>>,
    cached: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    matches!((incoming, cached), (Some(incoming), Some(cached)) if incoming < cached)
}

/// What an applied event changed, for the shell to refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Task(String),
    List(String),
    Comments(String),
    Chat(String),
    AgentTyping(Typing),
    Settings,
    /// A reminder has come due while the app was running.
    ///
    /// Carries nothing: the shell asks `remindersDue` for the list, so there is one answer to
    /// "which reminders are outstanding" rather than one here and a different one there.
    RemindersDue,
    /// Something the stream cannot describe changed — ask for a sync pass.
    NeedsSync,
    /// A pass nobody asked for — the sixty-second timer, or the external mirroring — brought
    /// something into the cache.
    ///
    /// This is what makes the timer a floor for the *screen* and not only for the cache: without
    /// it, a change that arrived while the stream was down sat in SQLite until the next click.
    /// Carries what moved so the open task is reloaded only when it is one of them; empty lists
    /// mean the pass could not say, and the shell refreshes what is on screen regardless.
    Synced {
        task_ids: Vec<String>,
        list_ids: Vec<String>,
    },
    /// The inbox moved — a sync pass fetched it and it differs from what was cached. The web
    /// sends no live event for the inbox, so this is how the bell learns.
    Notifications,
    /// The live stream connected (`true`) or dropped (`false`). Said once per edge. What a chat
    /// panel polls on: the stream delivers messages, and polling on top of it is duplication.
    Stream {
        live: bool,
    },
}

impl Change {
    /// The change as a shell reads it: `{"change":"task","id":…}` and so on — the vocabulary the
    /// Windows FFI has always sent, now answered here so every shell hears the same words.
    pub fn to_json(&self) -> String {
        let value = match self {
            Change::Task(id) => serde_json::json!({ "change": "task", "id": id }),
            Change::List(id) => serde_json::json!({ "change": "list", "id": id }),
            Change::Comments(id) => serde_json::json!({ "change": "comments", "taskId": id }),
            Change::Chat(id) => serde_json::json!({ "change": "chat", "channelId": id }),
            Change::AgentTyping(typing) => serde_json::json!({
                "change": "agentTyping", "channelId": typing.channel_id, "taskId": typing.task_id,
                "agentName": typing.agent_name, "active": typing.active
            }),
            Change::Settings => serde_json::json!({ "change": "settings" }),
            Change::RemindersDue => serde_json::json!({ "change": "remindersDue" }),
            Change::NeedsSync => serde_json::json!({ "change": "needsSync" }),
            Change::Synced { task_ids, list_ids } => serde_json::json!({
                "change": "synced", "taskIds": task_ids, "listIds": list_ids
            }),
            Change::Notifications => serde_json::json!({ "change": "notifications" }),
            Change::Stream { live } => serde_json::json!({ "change": "stream", "live": live }),
        };
        value.to_string()
    }
}

/// Something told when the cache moves. The shell registers one and refreshes what it names.
pub type ChangeListener = Box<dyn Fn(&Change) + Send + Sync>;

/// Everything a listener needs to hold, so the shell subscribes once rather than per screen.
pub struct RealtimeSink {
    store: Arc<Store>,
    listeners: std::sync::Mutex<Vec<ChangeListener>>,
    live: std::sync::atomic::AtomicBool,
    wake: tokio::sync::Notify,
}

impl RealtimeSink {
    pub fn new(store: Arc<Store>) -> Self {
        RealtimeSink {
            store,
            listeners: std::sync::Mutex::new(Vec::new()),
            live: std::sync::atomic::AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
        }
    }

    /// Drop the stream and connect again now, from a clean failure count — the machine woke, or
    /// the network came back, and neither the open connection nor the backoff describes the world
    /// any more. Remembered if nothing is listening yet, so a call made while the loop is between
    /// steps is not lost.
    pub fn reconnect_now(&self) {
        self.wake.notify_one();
    }

    pub(crate) async fn woken(&self) {
        self.wake.notified().await;
    }

    /// Whether the live stream is connected right now.
    pub fn is_live(&self) -> bool {
        self.live.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Record the stream connecting or dropping, announcing only a change.
    pub(crate) fn set_live(&self, live: bool) {
        if self.live.swap(live, std::sync::atomic::Ordering::SeqCst) != live {
            self.publish(Change::Stream { live });
        }
    }

    pub fn on_change(&self, listener: impl Fn(&Change) + Send + Sync + 'static) {
        self.listeners
            .lock()
            .expect("listener lock")
            .push(Box::new(listener));
    }

    /// Tell everyone about something that did not come from the stream.
    ///
    /// A reminder coming due is a change in what the app should be showing, and it reaches the
    /// shell the same way a colleague's edit does — one subscription, not two.
    pub fn publish(&self, change: Change) {
        for listener in self.listeners.lock().expect("listener lock").iter() {
            listener(&change);
        }
    }

    /// Apply one frame and tell everyone what moved.
    pub fn receive(&self, frame: &str) -> Option<Change> {
        let event = parse::parse(frame)?;
        let change = apply(&self.store, &event)?;
        for listener in self.listeners.lock().expect("listener lock").iter() {
            listener(&change);
        }
        Some(change)
    }
}

/// The types the events carry, re-exported so a caller does not have to reach into `model`.
pub type StreamTask = Task;
pub type StreamList = TaskList;
pub type StreamComment = Comment;
pub type StreamMessage = ChatMessage;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(body: serde_json::Value) -> String {
        format!("data: {body}\n\n")
    }

    #[test]
    fn a_task_event_lands_in_the_cache_and_names_what_changed() {
        let store = Store::in_memory().expect("opens");
        let event = parse::parse(&frame(json!({
            "type": "task_created",
            "data": { "id": "t1", "title": "Buy milk", "listIds": ["l1"] }
        })))
        .expect("parses");

        assert_eq!(apply(&store, &event), Some(Change::Task("t1".into())));
        assert_eq!(store.tasks_in_list("l1").expect("reads").len(), 1);
    }

    #[test]
    fn a_delete_event_removes_it() {
        let store = Store::in_memory().expect("opens");
        store
            .upsert_task(&Task::new("t1", "Buy milk"))
            .expect("stores");

        let event = parse::parse(&frame(json!({
            "type": "task_deleted",
            "data": { "id": "t1" }
        })))
        .expect("parses");
        assert_eq!(apply(&store, &event), Some(Change::Task("t1".into())));
        assert!(store.task("t1").expect("reads").is_none());
    }

    fn task_event(kind: &str, data: serde_json::Value) -> Event {
        parse::parse(&frame(json!({ "type": kind, "data": data }))).expect("parses")
    }

    /// The echo of an earlier edit — this machine's own, or a colleague's that crossed a newer one
    /// made here — arrives after the newer edit. It must not put the older title back (Apple
    /// `LiveUpdatePolicy`, which this crate had no equivalent of).
    #[test]
    fn an_older_event_does_not_overwrite_a_newer_edit_made_here() {
        let store = Store::in_memory().expect("opens");
        let mine: Task = serde_json::from_value(json!({
            "id": "t1", "title": "Newer, mine", "updatedAt": "2026-09-07T12:05:00Z"
        }))
        .expect("a task");
        store.upsert_task(&mine).expect("stores");

        let stale = task_event(
            "task_updated",
            json!({ "id": "t1", "title": "Older", "updatedAt": "2026-09-07T12:00:00Z" }),
        );
        assert_eq!(apply(&store, &stale), None, "nothing moved");
        assert_eq!(
            store.task("t1").expect("reads").expect("kept").title,
            "Newer, mine"
        );

        // An equal timestamp is the server's word on the same moment: take it.
        let same = task_event(
            "task_updated",
            json!({ "id": "t1", "title": "Server's", "updatedAt": "2026-09-07T12:05:00Z" }),
        );
        assert_eq!(apply(&store, &same), Some(Change::Task("t1".into())));
        assert_eq!(
            store.task("t1").expect("reads").expect("kept").title,
            "Server's"
        );
    }

    /// Deleted here, delete on its way: a late event about the task must not bring it back.
    #[test]
    fn a_late_event_does_not_bring_back_a_task_deleted_here() {
        let store = Store::in_memory().expect("opens");
        crate::outbox::journal::enqueue(
            &store,
            &crate::outbox::Entry::new(
                "e1",
                crate::outbox::kind::DELETE_TASK,
                json!({ "taskId": "t1" }),
                "k1",
                crate::model::date::parse("2026-09-07T12:00:00Z").expect("an instant"),
            ),
        )
        .expect("enqueues");

        let late = task_event(
            "task_updated",
            json!({ "id": "t1", "title": "Deleted here" }),
        );
        assert_eq!(apply(&store, &late), None);
        assert!(store.task("t1").expect("reads").is_none());
    }

    /// A heartbeat is not news.
    #[test]
    fn a_ping_changes_nothing() {
        let store = Store::in_memory().expect("opens");
        let event = parse::parse(&frame(json!({ "type": "ping" }))).expect("parses");
        assert_eq!(apply(&store, &event), None);
    }

    /// An event from a newer server is not an error. Whatever it was about arrives on the next
    /// sync pass anyway, which is the reason the stream is allowed to be incomplete.
    #[test]
    fn an_event_this_build_has_never_heard_of_is_ignored_quietly() {
        let store = Store::in_memory().expect("opens");
        let event = parse::parse(&frame(json!({
            "type": "somethingLater",
            "data": { "id": "x" }
        })))
        .expect("parses");
        assert_eq!(apply(&store, &event), None);
    }

    #[test]
    fn listeners_hear_about_what_moved() {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let sink = RealtimeSink::new(store);
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = heard.clone();
        sink.on_change(move |change| recorder.lock().expect("lock").push(change.clone()));

        sink.receive(&frame(json!({
            "type": "task_updated",
            "data": { "id": "t1", "title": "Buy oat milk" }
        })));

        assert_eq!(
            heard.lock().expect("lock").as_slice(),
            &[Change::Task("t1".into())]
        );
    }

    /// The stream can say "something changed that I cannot describe" — an external provider pushed
    /// a batch. The answer is a sync pass, not a guess.
    #[test]
    fn an_external_refresh_asks_for_a_sync_rather_than_inventing_one() {
        let store = Store::in_memory().expect("opens");
        let event =
            parse::parse(&frame(json!({ "type": "external_sync_refresh" }))).expect("parses");
        assert_eq!(apply(&store, &event), Some(Change::NeedsSync));
    }

    // ── Comments and chat, in the shapes the web sends ─────────────────────────────────────
    //
    // `services/comment.service.ts` and the chat routes wrap the row (`data.comment`,
    // `data.message`) beside the ids, and name deletions `commentId` / `messageId`. Read as the
    // row itself, every one of these was dropped: no live comment or chat message ever reached
    // this cache (the Apple apps' own client read them this way).

    #[test]
    fn a_comment_from_the_stream_lands_in_its_task() {
        let store = Store::in_memory().expect("opens");
        let event = task_event(
            "comment_created",
            json!({
                "taskId": "t1", "taskTitle": "Plan", "commentId": "c1",
                "commentContent": "Looks good", "commenterName": "Dana", "userId": "u2",
                "comment": {
                    "id": "c1", "content": "Looks good", "authorName": "Dana", "authorId": "u2",
                    "isAgent": false, "createdAt": "2026-09-07T12:00:00.000Z", "type": "TEXT",
                    "author": { "id": "u2", "name": "Dana" }, "parentCommentId": null,
                    "secureFiles": []
                }
            }),
        );
        assert_eq!(apply(&store, &event), Some(Change::Comments("t1".into())));
        let comments = store.comments_for_task("t1").expect("reads");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].content, "Looks good");
    }

    /// An edit event carries the text, not the author or the files this device already has —
    /// it changes what it says and nothing else (Apple AITD-331).
    #[test]
    fn a_comment_edit_from_the_stream_keeps_what_it_did_not_say() {
        let store = Store::in_memory().expect("opens");
        let cached: Comment = serde_json::from_value(json!({
            "id": "c1", "taskId": "t1", "content": "before", "authorId": "u2",
            "secureFiles": [{ "id": "f1", "name": "a.png", "size": 1, "mimeType": "image/png" }]
        }))
        .expect("a comment");
        store.upsert_comments(&[cached]).expect("stores");

        let event = task_event(
            "comment_updated",
            json!({
                "taskId": "t1", "commentId": "c1", "commentContent": "after",
                "comment": { "id": "c1", "content": "after", "type": "TEXT",
                             "updatedAt": "2026-09-07T12:05:00.000Z", "parentCommentId": null }
            }),
        );
        assert_eq!(apply(&store, &event), Some(Change::Comments("t1".into())));
        let comment = &store.comments_for_task("t1").expect("reads")[0];
        assert_eq!(comment.content, "after");
        assert_eq!(comment.author_id.as_deref(), Some("u2"));
        assert_eq!(comment.secure_files.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn a_comment_deleted_elsewhere_goes() {
        let store = Store::in_memory().expect("opens");
        let cached: Comment =
            serde_json::from_value(json!({ "id": "c1", "taskId": "t1", "content": "x" }))
                .expect("a comment");
        store.upsert_comments(&[cached]).expect("stores");
        let event = task_event(
            "comment_deleted",
            json!({ "taskId": "t1", "taskTitle": "Plan", "commentId": "c1", "deletedByName": "Dana" }),
        );
        assert_eq!(apply(&store, &event), Some(Change::Comments("t1".into())));
        assert!(store.comments_for_task("t1").expect("reads").is_empty());
    }

    #[test]
    fn a_chat_message_from_the_stream_lands_in_its_channel() {
        let store = Store::in_memory().expect("opens");
        let event = task_event(
            "chat_message_created",
            json!({ "channelId": "ch1", "message": {
                "id": "m1", "content": "hi", "authorId": "u2", "createdAt": "2026-09-07T12:00:00.000Z"
            } }),
        );
        assert_eq!(apply(&store, &event), Some(Change::Chat("ch1".into())));
        assert_eq!(
            store.messages_in_channel("ch1").expect("reads")[0].content,
            "hi"
        );

        let deleted = task_event(
            "chat_message_deleted",
            json!({ "channelId": "ch1", "messageId": "m1" }),
        );
        assert_eq!(apply(&store, &deleted), Some(Change::Chat("ch1".into())));
        assert!(store.messages_in_channel("ch1").expect("reads").is_empty());
    }
}
