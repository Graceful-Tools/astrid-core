//! Turning a wire frame into something the cache can act on.
//!
//! Ported from the buffer handling in `astrid-ios/Astrid App/Core/RealTime/SSEClient.swift`.
//!
//! ## Two frame formats, both live
//!
//! The stream has been through a format change and the client has to read both, because a client
//! can be talking to either deployment:
//!
//! - **Named**: the type is on its own `event:` line, and `data:` is the payload.
//! - **Unnamed**: there is no `event:` line; the payload is `{ "type": …, "data": … }`.
//!
//! Reading only one of them looks like a stream that connects successfully and delivers nothing —
//! the worst failure available here, because the app keeps working (sync still runs) and only
//! feels slow.
//!
//! Pure: a frame is a string in and an [`Event`] out, so every shape can be a test rather than
//! something you reproduce by asking a colleague to edit a task.

use crate::model::{ChatMessage, Comment, Task, TaskList};

/// A frame as it came off the wire, already split from the stream on the blank line.
pub type Frame<'a> = &'a str;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    TaskCreated(Task),
    TaskUpdated(Task),
    TaskDeleted(String),
    ListCreated(TaskList),
    ListUpdated(TaskList),
    ListDeleted(String),
    CommentAdded(Comment),
    CommentUpdated(Comment),
    CommentDeleted {
        id: String,
        task_id: String,
    },
    ChatMessageCreated(ChatMessage),
    ChatMessageUpdated(ChatMessage),
    ChatMessageDeleted {
        id: String,
        channel_id: String,
    },
    AgentTyping {
        channel_id: String,
        active: bool,
    },
    SettingsUpdated,
    ExternalSyncRefresh,
    Connected,
    Ping,
    /// A type this build does not know. Carried rather than dropped so it can be logged once and
    /// counted, instead of looking like a parse failure.
    Unknown(String),
}

/// Read one frame.
///
/// `None` means there was nothing in it worth acting on — a comment line, a blank keep-alive, a
/// frame with no data. Not an error: SSE is full of those by design.
pub fn parse(frame: Frame<'_>) -> Option<Event> {
    let mut event_type: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();

    for line in frame.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            // Multi-line data is joined with newlines, which is what the spec says and what a
            // pretty-printed JSON payload needs. Trimming each line and dropping the rest is how
            // a large payload silently becomes unparseable.
            data_lines.push(rest.trim_start());
        }
    }

    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    let payload: serde_json::Value = serde_json::from_str(&data).ok()?;

    // Unnamed frames carry the type inside; named ones carry it on the `event:` line. Whichever
    // is present wins, and the inner one is preferred because a deployment that sends both agrees
    // with itself.
    let kind = payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or(event_type)?;

    // The unnamed format nests the real payload under `data`; the named one is the payload.
    let body = payload.get("data").cloned().unwrap_or(payload);

    Some(Event {
        kind: read(&kind, body),
    })
}

fn read(kind: &str, body: serde_json::Value) -> EventKind {
    let id_of = |body: &serde_json::Value, field: &str| {
        body.get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };

    match kind {
        "task_created" => decode(body).map(EventKind::TaskCreated),
        "task_updated" => decode(body).map(EventKind::TaskUpdated),
        "task_deleted" => id_of(&body, "id").map(EventKind::TaskDeleted),
        "list_created" => decode(body).map(EventKind::ListCreated),
        "list_updated" => decode(body).map(EventKind::ListUpdated),
        "list_deleted" => id_of(&body, "id").map(EventKind::ListDeleted),
        // Both spellings are in the wild; they mean the same thing.
        "comment_added" | "comment_created" => decode(body).map(EventKind::CommentAdded),
        "comment_updated" => decode(body).map(EventKind::CommentUpdated),
        "comment_deleted" => id_of(&body, "id").map(|id| EventKind::CommentDeleted {
            id,
            task_id: id_of(&body, "taskId").unwrap_or_default(),
        }),
        "chat_message_created" => decode(body).map(EventKind::ChatMessageCreated),
        "chat_message_updated" => decode(body).map(EventKind::ChatMessageUpdated),
        "chat_message_deleted" => id_of(&body, "id").map(|id| EventKind::ChatMessageDeleted {
            id,
            channel_id: id_of(&body, "channelId").unwrap_or_default(),
        }),
        "agent_typing_start" => {
            id_of(&body, "channelId").map(|channel_id| EventKind::AgentTyping {
                channel_id,
                active: true,
            })
        }
        "agent_typing_stop" => id_of(&body, "channelId").map(|channel_id| EventKind::AgentTyping {
            channel_id,
            active: false,
        }),
        "user_settings_updated" | "my_tasks_preferences_updated" | "feature_flags_updated" => {
            Some(EventKind::SettingsUpdated)
        }
        "external_sync_refresh" => Some(EventKind::ExternalSyncRefresh),
        "connected" => Some(EventKind::Connected),
        "ping" => Some(EventKind::Ping),
        _ => None,
    }
    // A payload that will not decode is the same to a reader as a type nobody knows: the next sync
    // pass carries the change either way, so it is recorded as unknown rather than thrown.
    .unwrap_or_else(|| EventKind::Unknown(kind.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(body: serde_json::Value) -> Option<T> {
    serde_json::from_value(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The format the current deployment sends.
    #[test]
    fn an_unnamed_frame_carries_its_type_inside() {
        let event = parse(
            "data: {\"type\":\"task_created\",\"data\":{\"id\":\"t1\",\"title\":\"Buy milk\"}}\n\n",
        )
        .expect("parses");
        match event.kind {
            EventKind::TaskCreated(task) => assert_eq!(task.title, "Buy milk"),
            other => panic!("expected a created task, got {other:?}"),
        }
    }

    /// The format an older deployment sends. Reading only one format looks like a stream that
    /// connects and delivers nothing.
    #[test]
    fn a_named_frame_carries_its_type_on_its_own_line() {
        let event =
            parse("event: task_updated\ndata: {\"id\":\"t1\",\"title\":\"Buy oat milk\"}\n\n")
                .expect("parses");
        match event.kind {
            EventKind::TaskUpdated(task) => assert_eq!(task.title, "Buy oat milk"),
            other => panic!("expected an updated task, got {other:?}"),
        }
    }

    /// A pretty-printed payload arrives as several `data:` lines, and joining them is the only way
    /// it parses. Taking the last line is how a large event silently stops working.
    #[test]
    fn a_payload_split_over_several_data_lines_is_rejoined() {
        let event = parse("data: {\"type\":\"task_deleted\",\ndata: \"data\":{\"id\":\"t1\"}}\n\n")
            .expect("parses");
        assert_eq!(event.kind, EventKind::TaskDeleted("t1".into()));
    }

    #[test]
    fn keep_alives_and_comment_lines_are_nothing_to_act_on() {
        assert!(parse(": keep-alive\n\n").is_none());
        assert!(parse("\n\n").is_none());
        assert!(
            parse("event: ping\n\n").is_none(),
            "no data means nothing to read"
        );
    }

    /// Both spellings have been on the wire. They are the same event.
    #[test]
    fn a_comment_arrives_under_either_of_its_two_names() {
        for kind in ["comment_added", "comment_created"] {
            let frame = format!(
                "data: {{\"type\":\"{kind}\",\"data\":{{\"id\":\"c1\",\"taskId\":\"t1\"}}}}\n\n"
            );
            match parse(&frame).expect("parses").kind {
                EventKind::CommentAdded(comment) => assert_eq!(comment.id, "c1"),
                other => panic!("expected a comment, got {other:?}"),
            }
        }
    }

    #[test]
    fn typing_events_carry_their_channel_and_direction() {
        let start =
            parse("data: {\"type\":\"agent_typing_start\",\"data\":{\"channelId\":\"c1\"}}\n\n")
                .expect("parses");
        assert_eq!(
            start.kind,
            EventKind::AgentTyping {
                channel_id: "c1".into(),
                active: true
            }
        );
        let stop =
            parse("data: {\"type\":\"agent_typing_stop\",\"data\":{\"channelId\":\"c1\"}}\n\n")
                .expect("parses");
        assert_eq!(
            stop.kind,
            EventKind::AgentTyping {
                channel_id: "c1".into(),
                active: false
            }
        );
    }

    /// A type from a newer server is recorded, not thrown. Whatever it was about arrives on the
    /// next sync pass.
    #[test]
    fn an_unknown_type_is_carried_so_it_can_be_counted() {
        let event = parse("data: {\"type\":\"somethingLater\",\"data\":{}}\n\n").expect("parses");
        assert_eq!(event.kind, EventKind::Unknown("somethingLater".into()));
    }

    /// A payload that will not decode is, to a reader, the same as a type nobody knows.
    #[test]
    fn a_payload_that_will_not_decode_is_unknown_rather_than_a_failure() {
        let event = parse("data: {\"type\":\"task_created\",\"data\":{\"title\":\"no id\"}}\n\n")
            .expect("parses");
        assert_eq!(event.kind, EventKind::Unknown("task_created".into()));
    }

    #[test]
    fn a_frame_that_is_not_json_is_nothing_to_act_on() {
        assert!(parse("data: not json at all\n\n").is_none());
    }
}
