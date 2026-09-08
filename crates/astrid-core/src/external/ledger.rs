//! What was deleted here, so its twin over there can follow.
//!
//! Ports `SyncDeletionLedger` from `astrid-ios/Astrid App/Core/Sync/SyncDeletionPolicy.swift`.
//!
//! ## Why a ledger at all
//!
//! The server's link row cascades away with the task. So by the time a sync pass runs, a task
//! deleted here is simply gone and there is nothing left to say which remote item it was mirroring
//! — the evidence has to be captured **at delete time** or not at all. That is the whole reason
//! this exists, and it is the only reason a deletion needs anything more than the Outbox.
//!
//! ## Two stores, not one
//!
//! A pending deletion is work to do: the remote twin to remove. A tombstone is a fact to remember:
//! never import this id again, or a pull would undo the deletion on every pass forever.
//!
//! They are capped separately, and that separation is load-bearing. Apple merged the server's
//! tombstones — deletions from web and other devices — into the same store and found that a large
//! merge evicted this device's own, which let a deleted task come back. So: a small store for what
//! this machine deleted, a larger one for what the server reported, and the union is what a pull
//! consults.

use crate::store::{Result, Store};

/// How many of this machine's own tombstones to keep.
const OWN_CAP: usize = 500;

/// How many of the server's. Larger, because it merges everybody's.
const SERVER_CAP: usize = 5000;

fn pending_key(provider: &str) -> String {
    format!("sync.pendingDeletes.{provider}")
}

fn own_key(provider: &str) -> String {
    format!("sync.tombstones.own.{provider}")
}

fn server_key(provider: &str) -> String {
    format!("sync.tombstones.server.{provider}")
}

fn read(store: &Store, key: &str) -> Vec<serde_json::Value> {
    store
        .metadata(key)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<Vec<serde_json::Value>>(&json).ok())
        .unwrap_or_default()
}

fn write(store: &Store, key: &str, values: &[serde_json::Value]) -> Result<()> {
    store.set_metadata(
        key,
        &serde_json::to_string(values).unwrap_or_else(|_| "[]".into()),
    )
}

/// Note that a task was deleted here, and which remote item it was mirroring.
///
/// Both stores at once: the pending list so the next pass removes the twin, and the tombstone so a
/// pull never brings it back in the meantime.
pub fn record_deletion(
    store: &Store,
    provider: &str,
    remote_id: &str,
    container_id: &str,
) -> Result<()> {
    let key = pending_key(provider);
    let mut pending = read(store, &key);
    if !pending
        .iter()
        .any(|entry| entry.get("remoteId").and_then(|v| v.as_str()) == Some(remote_id))
    {
        pending.push(serde_json::json!({
            "remoteId": remote_id,
            "containerId": container_id,
        }));
        write(store, &key, &pending)?;
    }
    record_tombstone(store, provider, remote_id)
}

/// Remember an id this machine deleted, oldest evicted first.
///
/// Ordered rather than a set, because a cap on an unordered collection evicts whatever the hash
/// puts last — which can be the id added a second ago.
pub fn record_tombstone(store: &Store, provider: &str, remote_id: &str) -> Result<()> {
    let key = own_key(provider);
    let mut ids = read(store, &key);
    if ids.iter().any(|id| id.as_str() == Some(remote_id)) {
        return Ok(());
    }
    ids.push(serde_json::Value::String(remote_id.to_string()));
    if ids.len() > OWN_CAP {
        ids.drain(..ids.len() - OWN_CAP);
    }
    write(store, &key, &ids)
}

/// Merge the tombstones the server reported — deletions from web and other devices.
///
/// Into their own store, never into this machine's: a large merge must not evict a local deletion,
/// which is how a deleted task comes back.
pub fn merge_server_tombstones(store: &Store, provider: &str, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let key = server_key(provider);
    let mut held = read(store, &key);
    let existing: Vec<&str> = held.iter().filter_map(|id| id.as_str()).collect();
    let mut added: Vec<serde_json::Value> = ids
        .iter()
        .filter(|id| !existing.contains(&id.as_str()))
        .map(|id| serde_json::Value::String(id.clone()))
        .collect();
    held.append(&mut added);
    if held.len() > SERVER_CAP {
        held.drain(..held.len() - SERVER_CAP);
    }
    write(store, &key, &held)
}

/// Every id a pull must refuse: this machine's deletions and the server's, together.
pub fn tombstoned(store: &Store, provider: &str) -> Vec<String> {
    read(store, &own_key(provider))
        .into_iter()
        .chain(read(store, &server_key(provider)))
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// The remote twins still waiting to be removed, as `(remote id, container id)`.
pub fn pending(store: &Store, provider: &str) -> Vec<(String, String)> {
    read(store, &pending_key(provider))
        .into_iter()
        .filter_map(|entry| {
            Some((
                entry.get("remoteId")?.as_str()?.to_string(),
                entry.get("containerId")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// One twin removed. The tombstone stays: the deletion is still a fact.
pub fn clear_pending(store: &Store, provider: &str, remote_id: &str) -> Result<()> {
    let key = pending_key(provider);
    let held: Vec<serde_json::Value> = read(store, &key)
        .into_iter()
        .filter(|entry| entry.get("remoteId").and_then(|v| v.as_str()) != Some(remote_id))
        .collect();
    write(store, &key, &held)
}

/// Remote lists auto-linking must never offer again.
///
/// Deleting a mirrored list is the same shape of problem as deleting a mirrored task: in an
/// all-lists mode the next pass would see an unlinked remote list and helpfully make it again. So
/// the refusal is written down here, and pushed up to the account on the next pass so the other
/// devices stop offering it too.
fn excluded_key(provider: &str) -> String {
    format!("sync.excluded.{provider}")
}

pub fn exclude(store: &Store, provider: &str, container_id: &str) -> Result<()> {
    let key = excluded_key(provider);
    let mut ids = read(store, &key);
    if ids.iter().any(|id| id.as_str() == Some(container_id)) {
        return Ok(());
    }
    ids.push(serde_json::Value::String(container_id.to_string()));
    if ids.len() > OWN_CAP {
        ids.drain(..ids.len() - OWN_CAP);
    }
    write(store, &key, &ids)
}

/// Every remote list this device has said no to.
pub fn excluded(store: &Store, provider: &str) -> Vec<String> {
    read(store, &excluded_key(provider))
        .into_iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// The map a deletion is looked up in: local task id → its remote twin.
///
/// This is the other half of "capture at delete time". Deleting a task is a local, offline,
/// synchronous act — there is no asking the server which remote item it was, and by the time a
/// pass could ask, the server's link row has cascaded away with the task. So every pass writes the
/// links it fetched down here, and a deletion reads them.
fn cache_key(provider: &str) -> String {
    format!("sync.taskLinkCache.{provider}")
}

fn read_cache(store: &Store, provider: &str) -> serde_json::Map<String, serde_json::Value> {
    store
        .metadata(&cache_key(provider))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Write down one container's links, as `(local task id, remote id)`.
///
/// Merged rather than replaced: a pass covers one container, and replacing would throw away the
/// links of every other linked list — whose tasks would then delete silently on one side only.
pub fn remember_links(
    store: &Store,
    provider: &str,
    container_id: &str,
    links: impl IntoIterator<Item = (String, String)>,
) -> Result<()> {
    let mut cache = read_cache(store, provider);
    for (task_id, remote_id) in links {
        cache.insert(
            task_id,
            serde_json::Value::String(format!("{remote_id}/{container_id}")),
        );
    }
    store.set_metadata(
        &cache_key(provider),
        &serde_json::to_string(&cache).unwrap_or_else(|_| "{}".into()),
    )
}

/// The remote twin of a local task, as `(remote id, container id)`, if this device knows of one.
pub fn twin(store: &Store, provider: &str, task_id: &str) -> Option<(String, String)> {
    let cache = read_cache(store, provider);
    let entry = cache.get(task_id)?.as_str()?;
    let (remote_id, container_id) = entry.split_once('/')?;
    Some((remote_id.to_string(), container_id.to_string()))
}

/// The local task mirroring a remote item, as far as this device knows.
///
/// The other direction of [`twin`], and the reason it exists: a task pulled while offline has a
/// temporary id and cannot be linked on the server yet, so without a local answer the next pass
/// would see no twin and pull the same item in a second time. The id is resolved on the way out —
/// the temporary row is replaced by a real one when the Outbox gets through, and the cache would
/// otherwise be pointing at a task that no longer exists.
pub fn local_task_for(store: &Store, provider: &str, remote_id: &str) -> Option<String> {
    let prefix = format!("{remote_id}/");
    let cache = read_cache(store, provider);
    let (task_id, _) = cache.iter().find(|(_, entry)| {
        entry
            .as_str()
            .is_some_and(|entry| entry.starts_with(&prefix))
    })?;
    let resolved = store
        .resolve_id(task_id)
        .unwrap_or_else(|_| task_id.to_string());
    store.task(&resolved).ok().flatten().map(|task| task.id)
}

/// Drop one task's link, once its deletion has been written down.
pub fn forget_link(store: &Store, provider: &str, task_id: &str) -> Result<()> {
    let mut cache = read_cache(store, provider);
    if cache.remove(task_id).is_none() {
        return Ok(());
    }
    store.set_metadata(
        &cache_key(provider),
        &serde_json::to_string(&cache).unwrap_or_else(|_| "{}".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::in_memory().expect("opens")
    }

    /// The cache is what makes a delete-time capture possible: deleting is local, offline and
    /// synchronous, so the twin has to have been written down by an earlier pass.
    #[test]
    fn a_pass_writes_down_the_links_a_later_deletion_reads() {
        let store = store();
        remember_links(
            &store,
            "google",
            "tasklist-1",
            [("t1".to_string(), "r1".to_string())],
        )
        .expect("remembers");

        assert_eq!(
            twin(&store, "google", "t1"),
            Some(("r1".to_string(), "tasklist-1".to_string()))
        );
        assert_eq!(twin(&store, "google", "t2"), None);
    }

    /// A pass covers one container. Replacing rather than merging would throw away every other
    /// linked list's twins, and their tasks would then delete on one side only.
    #[test]
    fn remembering_one_container_keeps_anothers() {
        let store = store();
        remember_links(&store, "google", "c1", [("t1".into(), "r1".into())]).expect("remembers");
        remember_links(&store, "google", "c2", [("t2".into(), "r2".into())]).expect("remembers");

        assert_eq!(
            twin(&store, "google", "t1"),
            Some(("r1".to_string(), "c1".to_string()))
        );
        assert_eq!(
            twin(&store, "google", "t2"),
            Some(("r2".to_string(), "c2".to_string()))
        );
    }

    #[test]
    fn forgetting_a_link_leaves_the_rest() {
        let store = store();
        remember_links(
            &store,
            "google",
            "c1",
            [("t1".into(), "r1".into()), ("t2".into(), "r2".into())],
        )
        .expect("remembers");
        forget_link(&store, "google", "t1").expect("forgets");

        assert_eq!(twin(&store, "google", "t1"), None);
        assert!(twin(&store, "google", "t2").is_some());
    }

    /// The evidence has to be captured at delete time: the server's link row cascades away with
    /// the task, so by the next pass there is nothing left to say what it was mirroring.
    #[test]
    fn a_deletion_records_the_twin_and_the_tombstone_together() {
        let store = store();
        record_deletion(&store, "google", "r1", "tasklist-1").expect("records");

        assert_eq!(
            pending(&store, "google"),
            vec![("r1".to_string(), "tasklist-1".to_string())]
        );
        assert!(tombstoned(&store, "google").contains(&"r1".to_string()));
    }

    /// Removing the twin finishes the work; the fact of the deletion stays, or a pull brings it
    /// back on the next pass.
    #[test]
    fn clearing_the_work_keeps_the_fact() {
        let store = store();
        record_deletion(&store, "google", "r1", "c1").expect("records");
        clear_pending(&store, "google", "r1").expect("clears");

        assert!(pending(&store, "google").is_empty());
        assert!(tombstoned(&store, "google").contains(&"r1".to_string()));
    }

    /// The separation that stops a deleted task coming back: a large merge of the server's
    /// tombstones must not evict this machine's own.
    #[test]
    fn a_large_server_merge_does_not_evict_a_local_deletion() {
        let store = store();
        record_tombstone(&store, "google", "mine").expect("records");

        let theirs: Vec<String> = (0..SERVER_CAP + 100)
            .map(|n| format!("theirs-{n}"))
            .collect();
        merge_server_tombstones(&store, "google", &theirs).expect("merges");

        assert!(
            tombstoned(&store, "google").contains(&"mine".to_string()),
            "this machine's own deletion survived the merge"
        );
    }

    /// Oldest first, because a cap on an unordered collection can evict the id added a second ago.
    #[test]
    fn the_oldest_tombstone_is_the_one_evicted() {
        let store = store();
        for n in 0..OWN_CAP + 2 {
            record_tombstone(&store, "google", &format!("id-{n}")).expect("records");
        }
        let held = tombstoned(&store, "google");
        assert!(!held.contains(&"id-0".to_string()));
        assert!(held.contains(&format!("id-{}", OWN_CAP + 1)));
    }

    /// Deleting a mirrored list has to be remembered, or an all-lists mode makes it again on
    /// the next pass — and again after that.
    #[test]
    fn a_list_somebody_deleted_stops_being_offered() {
        let store = store();
        exclude(&store, "google", "c1").expect("excludes");
        exclude(&store, "google", "c1").expect("excludes");

        assert_eq!(excluded(&store, "google"), vec!["c1".to_string()]);
        assert!(excluded(&store, "github").is_empty());
    }

    #[test]
    fn one_provider_does_not_see_anothers() {
        let store = store();
        record_deletion(&store, "google", "r1", "c1").expect("records");
        assert!(tombstoned(&store, "github").is_empty());
        assert!(pending(&store, "github").is_empty());
    }

    #[test]
    fn recording_the_same_deletion_twice_records_it_once() {
        let store = store();
        record_deletion(&store, "google", "r1", "c1").expect("records");
        record_deletion(&store, "google", "r1", "c1").expect("records");
        assert_eq!(pending(&store, "google").len(), 1);
        assert_eq!(tombstoned(&store, "google").len(), 1);
    }
}
