//! What to do when the cache and the server disagree about a task.
//!
//! Ported from `astrid-ios/Astrid App/Core/Sync/ConflictResolver.swift`.
//!
//! The rules are field by field rather than whole-record, because the two kinds of disagreement
//! are genuinely different:
//!
//! - **What the person just did** — completing a task — wins over the server. They watched it
//!   happen; a sync that undid it in front of them is the worst thing this code can do.
//! - **What somebody else did** — assigning it, moving it between lists — is the server's, because
//!   the local copy has no way to know what happened on the other side and guessing costs a
//!   collaborator their change.
//! - **Everything in between** — title, description, due date, priority — goes to whichever was
//!   edited more recently.
//!
//! In practice this rarely fires: the Outbox delivers local writes in order and the server echoes
//! them back, so the two agree. It fires when a write was queued offline and something else
//! changed the same task in the meantime — which is exactly when getting it wrong is most visible.

use chrono::{DateTime, Utc};

use crate::model::Task;

/// Whether these two versions actually disagree.
///
/// A version with no timestamp on either side is not a conflict: it is a payload thin enough that
/// nothing can be compared, and inventing a disagreement there would resolve fields nobody
/// changed.
pub fn has_conflict(local: &Task, server: &Task) -> bool {
    match (local.updated_at, server.updated_at) {
        (Some(local_at), Some(server_at)) => local_at != server_at,
        _ => false,
    }
}

/// Merge a local task with the server's version.
pub fn resolve(local: &Task, server: &Task) -> Task {
    if !has_conflict(local, server) {
        return server.clone();
    }

    let mut resolved = server.clone();
    let local_is_newer = is_newer(local.updated_at, server.updated_at);

    // Completion: local wins, but only in the direction the person acted in. A local copy that
    // still says "not done" must not un-complete something completed elsewhere — that is a stale
    // cache, not an intention.
    if local.completed && !server.completed {
        resolved.completed = true;
        resolved.completed_at = local.completed_at.or(resolved.completed_at);
        resolved.completed_source = local.completed_source.clone();
    }

    if local_is_newer {
        resolved.title = local.title.clone();
        resolved.description = local.description.clone();
        resolved.due_date_time = local.due_date_time;
        resolved.is_all_day = local.is_all_day;
        resolved.priority = local.priority;
    }

    // Lists and assignment stay the server's: they are how other people's changes arrive, and a
    // local copy cannot tell "I moved this" from "I have not heard yet".
    resolved
}

fn is_newer(local: Option<DateTime<Utc>>, server: Option<DateTime<Utc>>) -> bool {
    match (local, server) {
        (Some(local), Some(server)) => local > server,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Merge two versions of a comment thread.
///
/// Comments are additive: two people commenting at once is not a conflict, it is a conversation.
/// The merge keeps every distinct comment, preferring the server's copy of one that exists on both
/// sides, and dedupes on the idempotency key as well as the id — a comment posted offline appears
/// on the server under a real id and locally under a `temp_` one, and matching only on id shows it
/// twice.
pub fn merge_comments(
    local: &[crate::model::Comment],
    server: &[crate::model::Comment],
) -> Vec<crate::model::Comment> {
    let mut merged: Vec<crate::model::Comment> = server.to_vec();
    let server_ids: std::collections::HashSet<&str> =
        server.iter().map(|comment| comment.id.as_str()).collect();
    let server_keys: std::collections::HashSet<&str> = server
        .iter()
        .filter_map(|comment| comment.client_request_id.as_deref())
        .collect();

    for comment in local {
        let already_there = server_ids.contains(comment.id.as_str())
            || comment
                .client_request_id
                .as_deref()
                .map(|key| server_keys.contains(key) || server_ids.contains(key))
                .unwrap_or(false);
        if !already_there {
            merged.push(comment.clone());
        }
    }

    merged.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{date, Comment, Priority};

    fn at(instant: &str) -> DateTime<Utc> {
        date::parse(instant).expect("an instant")
    }

    fn task(id: &str, updated: &str) -> Task {
        let mut task = Task::new(id, "Buy milk");
        task.updated_at = Some(at(updated));
        task
    }

    #[test]
    fn identical_timestamps_are_not_a_conflict_and_the_server_version_is_taken() {
        let local = task("t1", "2026-09-07T12:00:00Z");
        let mut server = task("t1", "2026-09-07T12:00:00Z");
        server.title = "Buy oat milk".into();

        assert!(!has_conflict(&local, &server));
        assert_eq!(resolve(&local, &server).title, "Buy oat milk");
    }

    /// A payload thin enough to carry no timestamps cannot be compared, and inventing a
    /// disagreement would resolve fields nobody touched.
    #[test]
    fn a_version_with_no_timestamp_is_not_a_conflict() {
        assert!(!has_conflict(&Task::new("t1", "x"), &Task::new("t1", "y")));
    }

    /// The rule that matters most. Someone watched the checkbox tick; a sync that unticks it is
    /// the worst thing this code can do.
    #[test]
    fn a_completion_the_person_just_made_survives_an_older_server_version() {
        let mut local = task("t1", "2026-09-07T12:00:00Z");
        local.completed = true;
        local.completed_at = Some(at("2026-09-07T12:00:00Z"));
        // The server is NEWER and still says not completed — someone else touched the title.
        let mut server = task("t1", "2026-09-07T12:05:00Z");
        server.title = "Buy oat milk".into();

        let resolved = resolve(&local, &server);
        assert!(resolved.completed, "the completion must survive");
        assert_eq!(
            resolved.title, "Buy oat milk",
            "and the newer title with it"
        );
    }

    /// The other direction is a stale cache, not an intention: a local copy that has not heard
    /// about a completion must not undo it.
    #[test]
    fn a_stale_local_copy_does_not_un_complete_something() {
        let local = task("t1", "2026-09-07T12:00:00Z");
        let mut server = task("t1", "2026-09-07T12:05:00Z");
        server.completed = true;

        assert!(resolve(&local, &server).completed);
    }

    #[test]
    fn the_more_recent_edit_wins_the_fields_either_side_can_change() {
        let mut local = task("t1", "2026-09-07T12:05:00Z");
        local.title = "Local title".into();
        local.priority = Priority::High;
        local.due_date_time = Some(at("2026-09-10T12:00:00Z"));

        let mut server = task("t1", "2026-09-07T12:00:00Z");
        server.title = "Server title".into();
        server.priority = Priority::Low;

        let resolved = resolve(&local, &server);
        assert_eq!(resolved.title, "Local title");
        assert_eq!(resolved.priority, Priority::High);
        assert_eq!(resolved.due_date_time, Some(at("2026-09-10T12:00:00Z")));
    }

    /// Assignment and list membership are how other people's changes arrive. A local copy cannot
    /// tell "I moved this" from "I have not heard yet", so it does not get to decide.
    #[test]
    fn assignment_and_membership_stay_the_servers_even_when_local_is_newer() {
        let mut local = task("t1", "2026-09-07T12:05:00Z");
        local.assignee_id = Some("me".into());
        local.list_ids = Some(vec!["l1".into()]);

        let mut server = task("t1", "2026-09-07T12:00:00Z");
        server.assignee_id = Some("someone-else".into());
        server.list_ids = Some(vec!["l2".into()]);

        let resolved = resolve(&local, &server);
        assert_eq!(resolved.assignee_id.as_deref(), Some("someone-else"));
        assert_eq!(resolved.effective_list_ids(), vec!["l2"]);
    }

    fn comment(id: &str, key: Option<&str>, at_instant: &str) -> Comment {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "taskId": "t1",
            "content": id,
            "clientRequestId": key,
            "createdAt": at_instant,
        }))
        .expect("decodes")
    }

    /// Two people commenting at once is a conversation, not a conflict.
    #[test]
    fn comments_from_both_sides_are_kept_in_order() {
        let local = vec![comment("c-local", None, "2026-09-07T12:01:00Z")];
        let server = vec![
            comment("c-server", None, "2026-09-07T12:00:00Z"),
            comment("c-later", None, "2026-09-07T12:02:00Z"),
        ];
        let ids: Vec<String> = merge_comments(&local, &server)
            .into_iter()
            .map(|comment| comment.id)
            .collect();
        assert_eq!(ids, vec!["c-server", "c-local", "c-later"]);
    }

    /// A comment posted offline is on the server under a real id and here under a temporary one.
    /// Matching only on id shows it twice, which is the shape of the bug users describe as "my
    /// comment posted twice".
    #[test]
    fn a_comment_that_has_come_back_under_its_real_id_is_not_added_again() {
        let local = vec![comment(
            "temp_abc",
            Some("temp_abc"),
            "2026-09-07T12:00:00Z",
        )];
        let server = vec![comment("c1", Some("temp_abc"), "2026-09-07T12:00:00Z")];
        let merged = merge_comments(&local, &server);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "c1");
    }
}
