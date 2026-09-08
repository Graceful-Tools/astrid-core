//! The decisions an external-sync pass makes, away from the plumbing that surrounds them.
//!
//! Ported from `astrid-ios/Astrid App/Core/Sync/` — `GoogleTasksPull`, `SyncContainerGuard`,
//! `SyncDeletionPolicy`, `SyncOrphanPrune` and `CompletionDriftPolicy` — with their tests as the
//! specification, which is what those files ask for in their own words: "sync bugs cost users
//! their data, and a decision you cannot run in a test is a decision nobody checks."
//!
//! Every one of these was extracted on Apple *after* something went wrong, and the comments there
//! name the failure. They are repeated here rather than summarised, because the reason is the
//! rule:
//!
//! - **A remote deletion is tombstone-driven, never inferred from absence.** Absence can mean "not
//!   loaded", and a mass delete from a truncated page is not recoverable from a client.
//! - **A push is refused across containers.** A task in two linked lists has one link per
//!   container, and pushing the wrong one patches an unrelated issue in somebody's repository.
//! - **A tombstone says "do not bring this back", not "never touch this again".** A task deleted
//!   and then recreated has to stay syncable.
//! - **Absence only means "deleted" for a record the server has acknowledged.** Anything created
//!   offline is absent from every response by definition.

use chrono::{DateTime, Utc};

/// What a pulled remote item means for the local twin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullOutcome {
    /// The remote says it is gone and we hold a twin — delete the twin.
    DeleteLocalTwin,
    /// The remote says gone, and there is nothing on our side to remove.
    IgnoreDeletion,
    /// We deleted this ourselves and told the remote. Importing it again would undo the deletion,
    /// and would do so on every pass forever.
    SkipResurrection,
    /// An ordinary create or update.
    Apply,
}

/// What to do with one pulled item.
pub fn pull_outcome(
    is_remote_deleted: bool,
    has_link: bool,
    has_local_task: bool,
    is_tombstoned: bool,
) -> PullOutcome {
    if is_remote_deleted {
        // A missing link and an already-deleted local task both leave nothing to remove.
        return if has_link && has_local_task {
            PullOutcome::DeleteLocalTwin
        } else {
            PullOutcome::IgnoreDeletion
        };
    }
    // Refused only when the link is gone too: a tombstone says "do not bring this back", not
    // "never touch this again", and a task deleted and recreated must stay syncable.
    if is_tombstoned && !has_link {
        return PullOutcome::SkipResurrection;
    }
    PullOutcome::Apply
}

/// The key a pulled subtask's parent resolves against.
///
/// Scoped to the container because Google reuses short task ids between task lists — an unscoped
/// key lets a subtask in one list adopt a parent in another. And Google sends an empty string
/// rather than omitting the field, so `""` must mean "no parent" or every top-level task becomes
/// the child of an id that does not exist.
pub fn parent_key(container_id: &str, raw_parent: Option<&str>) -> Option<String> {
    let parent = raw_parent?.trim();
    (!parent.is_empty()).then(|| format!("{container_id}:{parent}"))
}

/// Whether a link may be pushed during this container's pass.
///
/// A link records the container it was made against. A task in two linked lists appears in both
/// passes, and pushing the wrong link patches a remote item in the wrong place — for GitHub the
/// number collides with a real, unrelated issue and its title, body and state are clobbered; for
/// Google the id 404s and the push retries forever.
pub fn may_push(link_container_id: &str, pass_container_id: &str) -> bool {
    link_container_id == pass_container_id
}

/// One end of a mirrored pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub task_id: String,
    pub remote_id: String,
}

/// The links whose local task the user deleted — their remote twins go next.
pub fn remote_deletions<'a>(links: &'a [Link], tombstoned_task_ids: &[String]) -> Vec<&'a Link> {
    links
        .iter()
        .filter(|link| tombstoned_task_ids.contains(&link.task_id))
        .collect()
}

/// The links whose remote item is gone — their local twins go next.
///
/// `full_remote_ids` is `None` when the listing failed, and `truncated` says the page was cut
/// short. Either one means absence proves nothing, and a client that deleted on that basis would
/// wipe somebody's list from a dropped request. An explicit deleted flag needs no listing at all.
pub fn local_deletions<'a>(
    links: &'a [Link],
    full_remote_ids: Option<&[String]>,
    truncated: bool,
    explicitly_deleted: &[String],
) -> Vec<&'a Link> {
    links
        .iter()
        .filter(|link| {
            if explicitly_deleted.contains(&link.remote_id) {
                return true;
            }
            match full_remote_ids {
                Some(ids) if !truncated => !ids.contains(&link.remote_id),
                _ => false,
            }
        })
        .collect()
}

/// Whether to take the remote's completion for a linked pair.
///
/// Completing locally is safe when the local task never recorded a completion — a sync-created row
/// that drifted — or has not been touched since the last pass. Un-completing is destructive and
/// only ever applies to an untouched task.
///
/// A repeating task is the exception that cost Apple a bug: one that has just rolled forward also
/// has no completion recorded and is legitimately incomplete, so the escape would re-complete it
/// against a stale snapshot and march its due date forward again.
pub fn should_adopt_remote_completion(
    remote_completed: bool,
    local_completed: bool,
    local_completed_at: Option<DateTime<Utc>>,
    local_unchanged: bool,
    is_repeating: bool,
) -> bool {
    if remote_completed == local_completed {
        return false;
    }
    if remote_completed {
        if is_repeating {
            return local_unchanged;
        }
        return local_unchanged || local_completed_at.is_none();
    }
    local_unchanged
}

/// The sync states in which absence from a full response really does mean "deleted elsewhere".
///
/// `synced` — the server acknowledged it, and now it is not there. `pending_delete` — we asked for
/// it to go and it is gone. Everything else is kept, `pending` above all: that is a local create
/// the server cannot know about yet.
pub const PRUNABLE_STATUSES: [&str; 2] = ["synced", "pending_delete"];

/// A cached record, as the pruner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedRecord {
    pub id: String,
    /// `None` is unknown, and unknown is never pruned.
    pub sync_status: Option<String>,
}

/// Which cached records a **complete** server response says are gone.
///
/// Only ever call this with a response covering the whole collection. Handed a filtered or
/// paginated one, "not in the response" stops meaning "deleted" — and the delete half of caching
/// is the dangerous half.
pub fn orphan_ids(cached: &[CachedRecord], server_ids: &[String]) -> Vec<String> {
    cached
        .iter()
        .filter(|record| {
            !server_ids.contains(&record.id)
                // A temporary id is a local create the server has never heard of; it is absent
                // from every response by definition.
                && !crate::model::is_temp_id(&record.id)
                && record
                    .sync_status
                    .as_deref()
                    .is_some_and(|status| PRUNABLE_STATUSES.contains(&status))
        })
        .map(|record| record.id.clone())
        .collect()
}

/// Whether a remote item is already gone, from the status a delete came back with.
///
/// Branching on the status rather than on the text of an error, which changes the moment a message
/// is reworded or a build is localised.
pub fn remote_already_gone(status: u16) -> bool {
    status == 404 || status == 410
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn link(task: &str, remote: &str) -> Link {
        Link {
            task_id: task.into(),
            remote_id: remote.into(),
        }
    }

    #[test]
    fn a_remote_deletion_removes_the_twin_when_there_is_one() {
        assert_eq!(
            pull_outcome(true, true, true, false),
            PullOutcome::DeleteLocalTwin
        );
        assert_eq!(
            pull_outcome(true, false, true, false),
            PullOutcome::IgnoreDeletion
        );
        assert_eq!(
            pull_outcome(true, true, false, false),
            PullOutcome::IgnoreDeletion
        );
    }

    /// A tombstone says "do not bring this back", not "never touch this again".
    #[test]
    fn a_tombstoned_item_with_a_link_is_still_synced() {
        assert_eq!(
            pull_outcome(false, false, false, true),
            PullOutcome::SkipResurrection
        );
        assert_eq!(
            pull_outcome(false, true, true, true),
            PullOutcome::Apply,
            "a task deleted and then recreated must stay syncable"
        );
    }

    /// Google reuses short task ids between lists, and sends "" rather than omitting the field.
    #[test]
    fn a_parent_key_is_scoped_and_an_empty_parent_is_no_parent() {
        assert_eq!(
            parent_key("list-1", Some("abc")).as_deref(),
            Some("list-1:abc")
        );
        assert_eq!(parent_key("list-1", Some("")), None);
        assert_eq!(parent_key("list-1", Some("   ")), None);
        assert_eq!(parent_key("list-1", None), None);
    }

    /// A task in two linked lists has one link per container, and pushing the wrong one patches an
    /// unrelated issue in somebody's repository.
    #[test]
    fn a_link_from_another_container_is_not_pushed() {
        assert!(may_push("owner/repo", "owner/repo"));
        assert!(!may_push("owner/other", "owner/repo"));
    }

    #[test]
    fn a_deleted_task_takes_its_remote_twin_with_it() {
        let links = vec![link("t1", "r1"), link("t2", "r2")];
        let found = remote_deletions(&links, &["t2".to_string()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].remote_id, "r2");
    }

    /// The rule that stops a dropped request wiping somebody's list.
    #[test]
    fn absence_only_deletes_when_the_listing_was_complete() {
        let links = vec![link("t1", "r1")];

        // A failed listing proves nothing.
        assert!(local_deletions(&links, None, false, &[]).is_empty());
        // A truncated one proves nothing either.
        assert!(local_deletions(&links, Some(&[]), true, &[]).is_empty());
        // A complete listing that does not mention it does.
        assert_eq!(local_deletions(&links, Some(&[]), false, &[]).len(), 1);
        // And an explicit deleted flag needs no listing at all.
        assert_eq!(
            local_deletions(&links, None, true, &["r1".to_string()]).len(),
            1
        );
    }

    #[test]
    fn a_completion_that_matches_is_not_a_change() {
        assert!(!should_adopt_remote_completion(
            true, true, None, true, false
        ));
        assert!(!should_adopt_remote_completion(
            false, false, None, true, false
        ));
    }

    /// A sync-created row that never recorded a completion is repaired even if it looks touched.
    #[test]
    fn a_row_with_no_completion_recorded_adopts_the_remotes() {
        assert!(should_adopt_remote_completion(
            true, false, None, false, false
        ));
    }

    /// A repeating task that just rolled forward also has no completion recorded, and is
    /// legitimately incomplete — so it adopts only when genuinely untouched.
    #[test]
    fn a_repeating_task_is_not_re_completed_by_a_stale_snapshot() {
        assert!(!should_adopt_remote_completion(
            true, false, None, false, true
        ));
        assert!(should_adopt_remote_completion(
            true, false, None, true, true
        ));
    }

    /// Un-completing is destructive, so it only ever applies to an untouched task.
    #[test]
    fn un_completing_needs_an_untouched_task() {
        let completed = date::parse("2026-09-07T09:00:00Z");
        assert!(should_adopt_remote_completion(
            false, true, completed, true, false
        ));
        assert!(!should_adopt_remote_completion(
            false, true, completed, false, false
        ));
    }

    #[test]
    fn only_acknowledged_records_are_pruned() {
        let cached = vec![
            CachedRecord {
                id: "synced".into(),
                sync_status: Some("synced".into()),
            },
            CachedRecord {
                id: "pending".into(),
                sync_status: Some("pending".into()),
            },
            CachedRecord {
                id: "unknown".into(),
                sync_status: None,
            },
            CachedRecord {
                id: "temp_local".into(),
                sync_status: Some("synced".into()),
            },
        ];
        assert_eq!(orphan_ids(&cached, &[]), vec!["synced"]);
    }

    #[test]
    fn a_record_the_server_still_has_is_not_pruned() {
        let cached = vec![CachedRecord {
            id: "t1".into(),
            sync_status: Some("synced".into()),
        }];
        assert!(orphan_ids(&cached, &["t1".to_string()]).is_empty());
    }

    #[test]
    fn a_gone_remote_is_recognised_by_its_status() {
        assert!(remote_already_gone(404));
        assert!(remote_already_gone(410));
        assert!(!remote_already_gone(500));
        assert!(!remote_already_gone(200));
    }
}
