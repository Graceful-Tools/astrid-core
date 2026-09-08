//! Which lists still need a counterpart, and which existing one to adopt instead of duplicating.
//!
//! Ports `astrid-ios/Astrid App/Core/Sync/GoogleAutoLink.swift`.
//!
//! The whole point of the adoption rules is that a person who has "Groceries" in Astrid and
//! "Groceries" in Google Tasks ends up with **one** list, not two called the same thing. Getting
//! that wrong is not a small bug: it duplicates somebody's list on every pass until they notice.
//!
//! Pure planning. Nothing here creates or links anything; it says what should be created or
//! linked, which is the part worth testing.

use serde::{Deserialize, Serialize};

/// How lists get linked.
///
/// Stored on the server in the integration's metadata, so the choice follows the account rather
/// than the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// One list at a time, chosen by hand.
    Manual,
    /// Every remote list mirrors in; new ones are picked up each pass.
    AllGoogleToAstrid,
    /// Every Astrid list mirrors out; new ones are picked up each pass.
    AllAstridToGoogle,
    /// Both, with same-name pairs adopting each other rather than duplicating.
    AllBidirectional,
}

/// A list on either side: an id and a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRef {
    pub id: String,
    pub name: String,
}

/// A remote list that needs an Astrid one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptOrCreateHere {
    pub tasklist_id: String,
    /// An existing unlinked Astrid list to adopt, when one matches by name.
    pub adopt_list_id: Option<String>,
    /// What to call the new list, when there is nothing to adopt.
    pub new_list_name: String,
}

/// An Astrid list that needs a remote one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptOrCreateThere {
    pub list_id: String,
    pub adopt_tasklist_id: Option<String>,
    pub new_tasklist_name: String,
}

/// Whether the "My Tasks ↔ the default remote list" phase runs.
///
/// In any all-lists mode, unless an older setup already linked the default list by hand — in which
/// case that link stays authoritative and this phase must not sync the same thing twice.
pub fn my_tasks_phase_active(
    mode: SyncMode,
    default_tasklist_id: Option<&str>,
    linked_tasklist_ids: &[String],
) -> bool {
    if mode == SyncMode::Manual {
        return false;
    }
    match default_tasklist_id {
        Some(id) => !linked_tasklist_ids.iter().any(|linked| linked == id),
        None => false,
    }
}

/// Which remote lists auto-linking may consider.
///
/// A mode that mirrors *in* can turn the default list into a visible Astrid list. A mode that only
/// mirrors *out* keeps it reserved for the My Tasks phase, unless an older link already exists.
pub fn candidates<'a>(
    tasklists: &'a [ListRef],
    default_tasklist_id: Option<&str>,
    linked_tasklist_ids: &[String],
    include_unlinked_default: bool,
) -> Vec<&'a ListRef> {
    tasklists
        .iter()
        .filter(|tasklist| {
            include_unlinked_default
                || Some(tasklist.id.as_str()) != default_tasklist_id
                || linked_tasklist_ids.contains(&tasklist.id)
        })
        .collect()
}

/// What an Astrid list mirroring a remote one is called.
pub fn astrid_name(tasklist_name: &str, suffix: &str) -> String {
    let suffix = suffix.trim();
    if suffix.is_empty() {
        tasklist_name.to_string()
    } else {
        format!("{tasklist_name} {suffix}")
    }
}

/// Every unlinked remote list needs an Astrid counterpart.
///
/// Adopts an unlinked local list whose name matches — with or without the suffix — and each
/// adoptable list is consumed once, so two remote lists of the same name cannot both adopt it.
pub fn google_to_astrid(
    tasklists: &[ListRef],
    linked_tasklist_ids: &[String],
    unlinked_lists: &[ListRef],
    suffix: &str,
) -> Vec<AdoptOrCreateHere> {
    let mut adoptable: Vec<&ListRef> = unlinked_lists.iter().collect();
    tasklists
        .iter()
        .filter(|tasklist| !linked_tasklist_ids.contains(&tasklist.id))
        .map(|tasklist| {
            let target = astrid_name(&tasklist.name, suffix);
            let found = adoptable
                .iter()
                .position(|list| list.name == target || list.name == tasklist.name);
            let adopted = found.map(|index| adoptable.remove(index));
            AdoptOrCreateHere {
                tasklist_id: tasklist.id.clone(),
                adopt_list_id: adopted.map(|list| list.id.clone()),
                new_list_name: target,
            }
        })
        .collect()
}

/// Every unlinked Astrid list needs a remote counterpart.
///
/// A list that has never reached the server is skipped: its id is a temporary one, and creating a
/// remote twin for it would link something that is about to be given a different id.
pub fn astrid_to_google(
    lists: &[ListRef],
    linked_list_ids: &[String],
    unlinked_tasklists: &[ListRef],
) -> Vec<AdoptOrCreateThere> {
    let mut adoptable: Vec<&ListRef> = unlinked_tasklists.iter().collect();
    lists
        .iter()
        .filter(|list| !linked_list_ids.contains(&list.id) && !crate::model::is_temp_id(&list.id))
        .map(|list| {
            let found = adoptable
                .iter()
                .position(|tasklist| tasklist.name == list.name);
            let adopted = found.map(|index| adoptable.remove(index));
            AdoptOrCreateThere {
                list_id: list.id.clone(),
                adopt_tasklist_id: adopted.map(|tasklist| tasklist.id.clone()),
                new_tasklist_name: list.name.clone(),
            }
        })
        .collect()
}

/// Both directions in one pass.
///
/// The inward phase runs first and its adoptions count as links for the outward one — otherwise a
/// same-name pair would be adopted going in and then created again going out.
pub fn bidirectional(
    tasklists: &[ListRef],
    lists: &[ListRef],
    linked_tasklist_ids: &[String],
    linked_list_ids: &[String],
    suffix: &str,
) -> (Vec<AdoptOrCreateHere>, Vec<AdoptOrCreateThere>) {
    let unlinked_lists: Vec<ListRef> = lists
        .iter()
        .filter(|list| !linked_list_ids.contains(&list.id))
        .cloned()
        .collect();
    let inward = google_to_astrid(tasklists, linked_tasklist_ids, &unlinked_lists, suffix);

    let mut linked_after: Vec<String> = linked_list_ids.to_vec();
    linked_after.extend(
        inward
            .iter()
            .filter_map(|action| action.adopt_list_id.clone()),
    );
    let outward = astrid_to_google(lists, &linked_after, &[]);
    (inward, outward)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, name: &str) -> ListRef {
        ListRef {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn the_my_tasks_phase_is_off_in_manual_mode_and_when_the_default_is_linked() {
        assert!(!my_tasks_phase_active(SyncMode::Manual, Some("d"), &[]));
        assert!(my_tasks_phase_active(
            SyncMode::AllBidirectional,
            Some("d"),
            &[]
        ));
        assert!(!my_tasks_phase_active(
            SyncMode::AllBidirectional,
            Some("d"),
            &["d".to_string()]
        ));
        assert!(!my_tasks_phase_active(
            SyncMode::AllGoogleToAstrid,
            None,
            &[]
        ));
    }

    /// A mode that only mirrors out keeps the default list for the My Tasks phase.
    #[test]
    fn the_default_list_is_reserved_unless_the_mode_mirrors_in() {
        let tasklists = vec![list("default", "My Tasks"), list("other", "Work")];
        let kept = candidates(&tasklists, Some("default"), &[], false);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "other");

        let all = candidates(&tasklists, Some("default"), &[], true);
        assert_eq!(all.len(), 2);
    }

    /// An older hand-made link keeps the default list in play.
    #[test]
    fn a_default_list_that_is_already_linked_stays_a_candidate() {
        let tasklists = vec![list("default", "My Tasks")];
        let kept = candidates(&tasklists, Some("default"), &["default".to_string()], false);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn a_suffix_is_appended_and_a_blank_one_is_not() {
        assert_eq!(astrid_name("Work", "(Google)"), "Work (Google)");
        assert_eq!(astrid_name("Work", "   "), "Work");
        assert_eq!(astrid_name("Work", ""), "Work");
    }

    /// The rule the whole module exists for: one list, not two called the same thing.
    #[test]
    fn a_same_name_list_is_adopted_rather_than_duplicated() {
        let tasklists = vec![list("g1", "Groceries")];
        let unlinked = vec![list("l1", "Groceries")];
        let actions = google_to_astrid(&tasklists, &[], &unlinked, "");
        assert_eq!(actions[0].adopt_list_id.as_deref(), Some("l1"));
    }

    /// With a suffix, either spelling matches: the list somebody already made by hand is usually
    /// the plain one.
    #[test]
    fn adoption_matches_with_or_without_the_suffix() {
        let tasklists = vec![list("g1", "Work"), list("g2", "Home")];
        let unlinked = vec![list("l1", "Work (Google)"), list("l2", "Home")];
        let actions = google_to_astrid(&tasklists, &[], &unlinked, "(Google)");
        assert_eq!(actions[0].adopt_list_id.as_deref(), Some("l1"));
        assert_eq!(actions[1].adopt_list_id.as_deref(), Some("l2"));
    }

    /// One adoptable list cannot be adopted twice.
    #[test]
    fn an_adoptable_list_is_consumed() {
        let tasklists = vec![list("g1", "Work"), list("g2", "Work")];
        let unlinked = vec![list("l1", "Work")];
        let actions = google_to_astrid(&tasklists, &[], &unlinked, "");
        assert_eq!(actions[0].adopt_list_id.as_deref(), Some("l1"));
        assert_eq!(actions[1].adopt_list_id, None);
        assert_eq!(actions[1].new_list_name, "Work");
    }

    #[test]
    fn an_already_linked_remote_list_needs_nothing() {
        let tasklists = vec![list("g1", "Work")];
        assert!(google_to_astrid(&tasklists, &["g1".to_string()], &[], "").is_empty());
    }

    /// A list the server has never seen has a temporary id, and linking it would attach a remote
    /// twin to something about to be renamed by the server.
    #[test]
    fn a_list_that_has_not_synced_yet_is_not_mirrored_out() {
        let lists = vec![list("temp_abc", "New"), list("l1", "Work")];
        let actions = astrid_to_google(&lists, &[], &[]);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].list_id, "l1");
    }

    /// Both directions: a same-name pair adopts once and is not created again on the way back.
    #[test]
    fn a_same_name_pair_is_not_created_twice_in_bidirectional_mode() {
        let tasklists = vec![list("g1", "Groceries")];
        let lists = vec![list("l1", "Groceries")];
        let (inward, outward) = bidirectional(&tasklists, &lists, &[], &[], "");

        assert_eq!(inward[0].adopt_list_id.as_deref(), Some("l1"));
        assert!(
            outward.is_empty(),
            "the list adopted on the way in must not be created on the way out"
        );
    }

    /// A list with no counterpart still goes out.
    #[test]
    fn an_unmatched_list_is_still_mirrored_out() {
        let tasklists = vec![list("g1", "Groceries")];
        let lists = vec![list("l1", "Groceries"), list("l2", "Work")];
        let (_, outward) = bidirectional(&tasklists, &lists, &[], &[], "");
        assert_eq!(outward.len(), 1);
        assert_eq!(outward[0].list_id, "l2");
    }
}
