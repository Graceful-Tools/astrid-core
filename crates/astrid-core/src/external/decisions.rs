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

/// The due date to adopt from Google's date-only `due`, or `None` to leave the local task alone.
///
/// Google has no time of day, so a timed local due can never be clobbered by its mirror — every
/// push comes back as a date, and adopting it turned a 3pm task into an all-day one after one round
/// trip. An unchanged date is no change. (Apple `GoogleDueMapping.adoptedDue`.)
pub fn adopted_due(
    remote_due: Option<chrono::DateTime<chrono::Utc>>,
    local_due: Option<chrono::DateTime<chrono::Utc>>,
    local_is_all_day: bool,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let remote_due = remote_due?;
    if !(local_is_all_day || local_due.is_none()) || local_due == Some(remote_due) {
        return None;
    }
    Some(remote_due)
}

/// The date-only value Google is sent for a local due: the day at UTC midnight.
///
/// An all-day due is stored at UTC midnight, so its UTC day is the day. A timed due is an instant,
/// and the person's own day is the day — an 8pm due west of UTC is tomorrow in UTC and must not
/// move a day in Google. (Apple `GoogleDueMapping.pushDueString`.)
pub fn push_due(
    due: chrono::DateTime<chrono::Utc>,
    is_all_day: bool,
    zone: chrono_tz::Tz,
) -> String {
    use chrono::TimeZone;
    let day = if is_all_day {
        due.date_naive()
    } else {
        due.with_timezone(&zone).date_naive()
    };
    let midnight =
        chrono::Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).expect("midnight exists"));
    crate::model::date::format(midnight)
}

// ── Watermarks: echo suppression and last-write-wins ───────────────────────────────────────────
//
// Ported from Apple's `SyncSuppression` (AWTD2-56). Every task link on the server carries two
// stamps: `remoteUpdatedAt` guards the pull, `astridUpdatedAt` guards the push. A wrong comparison
// here is either an echo loop or a silently dropped edit, so the comparisons are kept here, tested,
// and in one place.
//
// Compared to the millisecond, because that is what the wire carries: a stamp read back from the
// server has lost the sub-millisecond part of the instant this machine wrote, and a nanosecond
// comparison would read every task as changed since its own watermark — a push on every pass.

fn millis(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

/// PULL: apply a remote change only if it is strictly newer than the remote watermark written at
/// the last push or pull. A missing stamp on either side cannot prove an echo — apply.
pub fn should_apply_remote(
    remote_updated_at: Option<DateTime<Utc>>,
    watermark: Option<DateTime<Utc>>,
) -> bool {
    match (remote_updated_at, watermark) {
        (Some(remote), Some(watermark)) => millis(remote) > millis(watermark),
        _ => true,
    }
}

/// CONFLICT: a pulled change that passed the echo watermark can still race a fresher local edit —
/// one still sitting in the Outbox. Last write wins: the remote applies only when it is provably
/// newer than the local task. An unprovable remote stamp never clobbers local state, and a tie
/// keeps local.
pub fn remote_wins(
    remote_updated_at: Option<DateTime<Utc>>,
    local_updated_at: Option<DateTime<Utc>>,
) -> bool {
    match (remote_updated_at, local_updated_at) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(remote), Some(local)) => millis(remote) > millis(local),
    }
}

/// PUSH: send a local change only if it is strictly newer than the local watermark written when
/// the task was last pushed or pulled. Its negation is "unchanged since the last pass", which is
/// what [`should_adopt_remote_completion`] and the pull's last-write-wins both need.
pub fn should_push_local(
    local_updated_at: Option<DateTime<Utc>>,
    watermark: Option<DateTime<Utc>>,
) -> bool {
    match (local_updated_at, watermark) {
        (Some(local), Some(watermark)) => millis(local) > millis(watermark),
        _ => true,
    }
}

/// Whether a remote twin may be created for a local task with none.
///
/// Only when a complete remote listing proves no twin exists. A failed listing and a truncated one
/// both mean "unknown", never "empty" — the twin may simply be past the end of the page, and
/// creating then is the duplicate. (Apple `SyncAdoptionSafety.mayCreateRemote`.)
pub fn may_create_remote(
    full_listing_available: bool,
    listing_truncated: bool,
    matching_twin_exists: bool,
) -> bool {
    full_listing_available && !listing_truncated && !matching_twin_exists
}

/// Order pulled items so a parent is applied before its children, and a child created in the same
/// pass can resolve its parent's fresh link. Anything cyclic or unresolvable is appended at the
/// end — created top-level rather than dropped. (Apple `SyncPullOrdering.parentsFirst`.)
pub fn parents_first<T: Clone>(
    items: &[T],
    id: impl Fn(&T) -> String,
    parent_id: impl Fn(&T) -> Option<String>,
) -> Vec<T> {
    let mut remaining: Vec<T> = items.to_vec();
    let mut remaining_ids: std::collections::HashSet<String> = items.iter().map(&id).collect();
    let mut ordered = Vec::with_capacity(items.len());
    while !remaining.is_empty() {
        let (ready, waiting): (Vec<T>, Vec<T>) = remaining.into_iter().partition(|item| {
            parent_id(item)
                .is_none_or(|parent| parent.is_empty() || !remaining_ids.contains(&parent))
        });
        if ready.is_empty() {
            ordered.extend(waiting);
            break;
        }
        for item in &ready {
            remaining_ids.remove(&id(item));
        }
        ordered.extend(ready);
        remaining = waiting;
    }
    ordered
}

/// How often a linked list's complete listing is fetched for absence deletion when nothing else in
/// the pass needed one. (Apple `FullPullThrottle`, 300 seconds.)
pub const FULL_PULL_INTERVAL_SECS: i64 = 300;

/// Whether the throttled complete listing is due. Only a successful, complete listing moves the
/// stamp, at the call site.
pub fn full_pull_due(last_success: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_success.is_none_or(|last| (now - last).num_seconds() >= FULL_PULL_INTERVAL_SECS)
}

/// How many completed remote items one pass imports. Completed history trickles in; it must never
/// delay the live items. (Apple `CompletedBackfill`, budget 20.)
pub const BACKFILL_BUDGET: usize = 20;

/// A remote item, as the completed backfill sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillCandidate {
    pub remote_id: String,
    pub completed: bool,
    pub deleted: bool,
    /// RFC 3339, which sorts as text.
    pub updated_at: String,
}

/// The completed remote items to import this pass, newest first: completed, not deleted, not
/// already linked, not deleted here. (Apple `CompletedBackfill.select`.)
pub fn backfill_selection<'a>(
    items: &'a [BackfillCandidate],
    is_linked: impl Fn(&str) -> bool,
    tombstoned: &[String],
    budget: usize,
) -> Vec<&'a BackfillCandidate> {
    let mut chosen: Vec<&BackfillCandidate> = items
        .iter()
        .filter(|item| {
            item.completed
                && !item.deleted
                && !is_linked(&item.remote_id)
                && !tombstoned.contains(&item.remote_id)
        })
        .collect();
    chosen.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    chosen.truncate(budget);
    chosen
}

/// Local tasks a pulled item may adopt by title instead of making a second one.
///
/// Only an unambiguous title adopts: two local tasks called "Buy milk" and one remote one is a
/// guess, and a guess that links the wrong pair is worse than a duplicate somebody can delete. A
/// task adopted once is consumed, so one local task never answers for two remote items in a pass.
/// (Apple's same-title adoption and `BackfillAdoptionIndex`.)
#[derive(Debug, Default)]
pub struct TitleIndex {
    by_title: std::collections::HashMap<String, Vec<String>>,
}

impl TitleIndex {
    /// `(task id, title)` for every task that may be adopted.
    pub fn new(candidates: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut by_title: std::collections::HashMap<String, Vec<String>> = Default::default();
        for (task_id, title) in candidates {
            by_title.entry(title).or_default().push(task_id);
        }
        TitleIndex { by_title }
    }

    /// The one task with this title, consumed — or `None` when there is none or more than one.
    pub fn take_unique(&mut self, title: &str) -> Option<String> {
        match self.by_title.get(title) {
            Some(ids) if ids.len() == 1 => self.by_title.remove(title)?.pop(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    // ── AWTD2-56: the watermark rules, ported with Apple's `SyncProviderLogicTests` ──────────

    fn at(text: &str) -> Option<DateTime<Utc>> {
        date::parse(text)
    }

    /// AWTD2-56: a pull applies only what is newer than the watermark; equal is our own echo.
    #[test]
    fn awtd2_56_a_pull_skips_its_own_echo() {
        let t0 = at("2026-09-07T12:00:00Z");
        let t1 = at("2026-09-07T12:00:01Z");
        assert!(should_apply_remote(t1, t0));
        assert!(
            !should_apply_remote(t0, t0),
            "equal is the echo of our write"
        );
        assert!(!should_apply_remote(t0, t1), "older is stale");
        assert!(should_apply_remote(None, t0));
        assert!(should_apply_remote(t0, None));
    }

    /// AWTD2-56: a push sends only what changed since the watermark.
    #[test]
    fn awtd2_56_a_push_sends_only_what_changed_since_the_watermark() {
        let t0 = at("2026-09-07T12:00:00Z");
        let t1 = at("2026-09-07T12:00:01Z");
        assert!(should_push_local(t1, t0));
        assert!(!should_push_local(t0, t0));
        assert!(!should_push_local(t0, t1));
        assert!(should_push_local(None, t0));
        assert!(should_push_local(t0, None));
    }

    /// AWTD2-56: the wire carries milliseconds, so a stamp read back from the server equals the
    /// instant written even when that instant had nanoseconds.
    #[test]
    fn awtd2_56_watermarks_compare_at_the_precision_the_wire_carries() {
        let written = at("2026-09-07T12:00:00.123Z")
            .map(|at| at + chrono::Duration::nanoseconds(456_789))
            .expect("an instant");
        let read_back = date::parse(&date::format(written));
        assert!(!should_push_local(Some(written), read_back));
        assert!(!should_apply_remote(Some(written), read_back));
    }

    /// AWTD2-56: last write wins, and an unprovable or tied remote never clobbers local.
    #[test]
    fn awtd2_56_a_stale_remote_never_clobbers_a_fresher_local_edit() {
        let t0 = at("2026-09-07T12:00:00Z");
        let t1 = at("2026-09-07T12:00:01Z");
        assert!(remote_wins(t1, t0));
        assert!(!remote_wins(t0, t1));
        assert!(!remote_wins(t0, t0), "a tie keeps local");
        assert!(!remote_wins(None, t0));
        assert!(remote_wins(t0, None));
    }

    /// AWTD2-56: creating a twin needs a complete listing that proves there is none.
    #[test]
    fn awtd2_56_a_twin_is_created_only_after_a_complete_listing() {
        assert!(may_create_remote(true, false, false));
        assert!(!may_create_remote(false, false, false), "failed listing");
        assert!(!may_create_remote(true, true, false), "truncated listing");
        assert!(!may_create_remote(true, false, true), "a twin exists");
    }

    /// AWTD2-56: parents before children, whatever order Google lists them in; a cycle is kept.
    #[test]
    fn awtd2_56_parents_are_applied_before_their_children() {
        let items = vec![
            ("child", Some("parent")),
            ("parent", None),
            ("orphan", Some("gone")),
        ];
        let ordered = parents_first(
            &items,
            |item| item.0.to_string(),
            |item| item.1.map(str::to_string),
        );
        let names: Vec<&str> = ordered.iter().map(|item| item.0).collect();
        assert_eq!(names, vec!["parent", "orphan", "child"]);

        let cycle = vec![("a", Some("b")), ("b", Some("a"))];
        assert_eq!(
            parents_first(&cycle, |i| i.0.to_string(), |i| i.1.map(str::to_string)).len(),
            2,
            "a cycle is emitted, not dropped"
        );
    }

    #[test]
    fn awtd2_56_the_complete_listing_is_throttled() {
        let t0 = at("2026-09-07T12:00:00Z").expect("an instant");
        assert!(full_pull_due(None, t0));
        assert!(full_pull_due(Some(t0), t0 + chrono::Duration::seconds(300)));
        assert!(!full_pull_due(
            Some(t0),
            t0 + chrono::Duration::seconds(299)
        ));
    }

    /// AWTD2-56: completed history comes in newest first, under a budget, never twice.
    #[test]
    fn awtd2_56_backfill_takes_completed_unlinked_items_newest_first() {
        let item = |id: &str, completed: bool, deleted: bool, at: &str| BackfillCandidate {
            remote_id: id.into(),
            completed,
            deleted,
            updated_at: at.into(),
        };
        let items = vec![
            item("b", true, false, "2026-09-01T00:00:00Z"),
            item("a", true, false, "2026-09-03T00:00:00Z"),
            item("open", false, false, "2026-09-04T00:00:00Z"),
            item("gone", true, true, "2026-09-04T00:00:00Z"),
            item("c", true, false, "2026-09-02T00:00:00Z"),
            item("linked", true, false, "2026-09-05T00:00:00Z"),
            item("dead", true, false, "2026-09-05T00:00:00Z"),
        ];
        let chosen = backfill_selection(&items, |id| id == "linked", &["dead".to_string()], 2);
        let ids: Vec<&str> = chosen.iter().map(|item| item.remote_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    /// AWTD2-56: adoption by title only when unambiguous, and only once.
    #[test]
    fn awtd2_56_only_an_unambiguous_title_is_adopted_and_only_once() {
        let mut index = TitleIndex::new([
            ("a".to_string(), "One".to_string()),
            ("b".to_string(), "Same".to_string()),
            ("c".to_string(), "Same".to_string()),
        ]);
        assert_eq!(index.take_unique("One").as_deref(), Some("a"));
        assert_eq!(index.take_unique("One"), None, "consumed");
        assert_eq!(index.take_unique("Same"), None, "ambiguous");
        assert_eq!(index.take_unique("Missing"), None);
    }

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

    #[test]
    fn a_timed_due_is_never_replaced_by_googles_date() {
        let at = |text: &str| crate::model::date::parse(text);
        let google = at("2026-09-08T00:00:00Z");
        assert_eq!(adopted_due(google, at("2026-09-07T22:00:00Z"), false), None);
        assert_eq!(
            adopted_due(google, at("2026-09-07T00:00:00Z"), true),
            google
        );
        assert_eq!(adopted_due(google, None, false), google);
        assert_eq!(
            adopted_due(google, google, true),
            None,
            "unchanged is no change"
        );
        assert_eq!(adopted_due(None, at("2026-09-07T00:00:00Z"), true), None);
    }

    #[test]
    fn google_is_sent_the_persons_own_day() {
        let evening = crate::model::date::parse("2026-09-08T03:00:00Z").expect("an instant");
        // 8pm on the 7th in Los Angeles is the 8th in UTC; Google hears the 7th.
        assert_eq!(
            push_due(evening, false, chrono_tz::America::Los_Angeles),
            "2026-09-07T00:00:00Z"
        );
        let all_day = crate::model::date::parse("2026-09-08T00:00:00Z").expect("an instant");
        assert_eq!(
            push_due(all_day, true, chrono_tz::America::Los_Angeles),
            "2026-09-08T00:00:00Z"
        );
    }
}
