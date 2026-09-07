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

pub use parse::{Event, EventKind, Frame};
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
            store.upsert_task(task).ok()?;
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
        EventKind::CommentAdded(comment) | EventKind::CommentUpdated(comment) => {
            store.upsert_comments(std::slice::from_ref(comment)).ok()?;
            Some(Change::Comments(comment.task_id.clone()))
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
        EventKind::AgentTyping { channel_id, active } => Some(Change::AgentTyping {
            channel_id: channel_id.clone(),
            active: *active,
        }),
        EventKind::SettingsUpdated => Some(Change::Settings),
        EventKind::ExternalSyncRefresh => Some(Change::NeedsSync),
        // A heartbeat means the connection is alive, which is worth nothing to a reader.
        EventKind::Connected | EventKind::Ping => None,
        // An event from a newer server. Not an error: the next sync pass will carry whatever it
        // was about.
        EventKind::Unknown(_) => None,
    }
}

/// What an applied event changed, for the shell to refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Task(String),
    List(String),
    Comments(String),
    Chat(String),
    AgentTyping {
        channel_id: String,
        active: bool,
    },
    Settings,
    /// A reminder has come due while the app was running.
    ///
    /// Carries nothing: the shell asks `remindersDue` for the list, so there is one answer to
    /// "which reminders are outstanding" rather than one here and a different one there.
    RemindersDue,
    /// Something the stream cannot describe changed — ask for a sync pass.
    NeedsSync,
}

/// Something told when the cache moves. The shell registers one and refreshes what it names.
pub type ChangeListener = Box<dyn Fn(&Change) + Send + Sync>;

/// Everything a listener needs to hold, so the shell subscribes once rather than per screen.
pub struct RealtimeSink {
    store: Arc<Store>,
    listeners: std::sync::Mutex<Vec<ChangeListener>>,
}

impl RealtimeSink {
    pub fn new(store: Arc<Store>) -> Self {
        RealtimeSink {
            store,
            listeners: std::sync::Mutex::new(Vec::new()),
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
}
