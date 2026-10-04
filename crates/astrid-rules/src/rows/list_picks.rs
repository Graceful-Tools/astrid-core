//! Which lists a task can be put in, for the detail pane's list editor (task d3f3b111).
//!
//! Ports the rules behind astrid-web's Lists field (`TaskFieldEditors.tsx`, `editingLists`, and
//! `handleCreateNewList` in `task-detail.tsx`). On Windows the Lists row drew chips and nothing
//! else, so the single most common edit after the title — moving a task between lists — could
//! only be done by re-creating the task.
//!
//! What is a rule here, and why:
//!
//! - **What is offered.** Every list but a virtual one ("Today" is somewhere to look, not
//!   somewhere a task lives) — iOS's `InlineListsPicker` rule, followed since 2026-10-03
//!   (`docs/CONTRACTS.md` D28). Board columns and labels are lists a task can be in, so they are
//!   offered; the web's `selectableLists` also cuts columns. Minus the lists the task is already
//!   in, filtered by what was typed, and no more than the first ten — the web's `.slice(0, 10)`.
//! - **When a create is offered.** A typed name no list already has. Offering to create "Home"
//!   beside an existing "Home" is how an account comes to have two.
//! - **What a list created from here looks like.** Its privacy follows the task's current lists,
//!   PUBLIC over SHARED over PRIVATE, so a task in a shared list does not quietly gain a private
//!   one nobody else can see. Its colour is one of the web's eight, so a list made on Windows
//!   looks like one made anywhere.
//!
//! Nothing here writes. The commands that do — add, remove, create-and-add — are ordinary
//! `TaskService` and `ListService` writes through the Outbox, which is what keeps this working on
//! a train.

use serde::Serialize;

use crate::model::{Privacy, TaskList};

/// The colours the web offers a new list — `LIST_COLOR_PALETTE` in `lib/brand/colors.ts`.
///
/// Choices, not brand values: a person picking red means red on every deployment, and the brand
/// accent is deliberately not spliced in.
pub const LIST_COLOR_PALETTE: [&str; 8] = [
    "#ef4444", // red
    "#f97316", // orange
    "#eab308", // yellow
    "#22c55e", // green
    "#06b6d4", // cyan
    "#3b82f6", // blue
    "#8b5cf6", // violet
    "#ec4899", // pink
];

/// A colour for a list the user did not colour themselves — the web's `randomListColor`.
///
/// The randomness is std's per-process hash keys rather than the OS's entropy source: a list
/// colour needs variety, not secrecy, and this crate reads nothing from the machine so that it
/// builds for WebAssembly. (On `wasm32-unknown-unknown` the keys are fixed and so is the pick.)
pub fn random_list_color() -> &'static str {
    use std::hash::{BuildHasher, Hasher};
    let bits = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    LIST_COLOR_PALETTE[(bits % LIST_COLOR_PALETTE.len() as u64) as usize]
}

/// No more than this many suggestions, as the web shows.
pub const MAX_OPTIONS: usize = 10;

/// One list as the picker draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPick {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// What the editor shows for one task and one search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPicks {
    /// The lists the task is in, in the task's own order.
    pub selected: Vec<ListPick>,
    /// The lists it could be added to that match the search.
    pub options: Vec<ListPick>,
    /// The name to offer creating, when what was typed is not a list yet.
    pub create_name: Option<String>,
}

/// Whether a task can be filed in this list at all: anything but a virtual list.
///
/// iOS's `InlineListsPicker` rule (D28, resolved toward iOS). `ListService::destinations` asks
/// this too, so there is one answer.
pub fn is_destination(list: &TaskList) -> bool {
    !list.is_virtual.unwrap_or(false)
}

fn pick(list: &TaskList) -> ListPick {
    ListPick {
        id: list.id.clone(),
        name: list.name.clone(),
        color: list.display_color().to_string(),
    }
}

/// The picker's rows for a task in `task_list_ids`, given every list and what was typed.
pub fn picks(task_list_ids: &[String], lists: &[TaskList], query: &str) -> ListPicks {
    let selected: Vec<ListPick> = task_list_ids
        .iter()
        .filter_map(|id| lists.iter().find(|list| &list.id == id))
        .filter(|list| is_destination(list))
        .map(pick)
        .collect();

    let needle = query.trim().to_lowercase();
    let mut candidates: Vec<&TaskList> = lists
        .iter()
        .filter(|list| is_destination(list) && !task_list_ids.contains(&list.id))
        .filter(|list| needle.is_empty() || list.name.to_lowercase().contains(&needle))
        .collect();
    // By name, so the same account offers the same order on every machine rather than the
    // order its cache happened to return.
    candidates.sort_by_key(|list| list.name.to_lowercase());
    let options = candidates.into_iter().take(MAX_OPTIONS).map(pick).collect();

    let trimmed = query.trim();
    let taken = lists
        .iter()
        .any(|list| is_destination(list) && list.name.trim().eq_ignore_ascii_case(trimmed));
    let create_name = (!trimmed.is_empty() && !taken).then(|| trimmed.to_string());

    ListPicks {
        selected,
        options,
        create_name,
    }
}

/// One row of the Apple pickers: a list, and whether the task is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToggle {
    pub id: String,
    pub name: String,
    pub color: String,
    pub is_selected: bool,
}

/// What the Apple pickers show (AITD-461, `docs/CONTRACTS.md` D50 — iOS's `InlineListsPicker`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToggles {
    /// The lists the task is in, as chips, in the order below.
    pub selected: Vec<ListToggle>,
    /// Every list it can be filed in that matches the search, selected ones included.
    pub options: Vec<ListToggle>,
}

/// iOS's list picker: a checklist rather than a set of suggestions.
///
/// Every destination ([`is_destination`]) is a row, the task's own lists among them marked
/// selected, so one tap files or unfiles. In the sidebar's order — favourites first, then by name
/// without regard to case (iOS's `ListOrdering`), the id breaking ties. What was typed is matched
/// as typed, without regard to case, anywhere in the name. No cap and no "create": neither Apple
/// picker offers one.
pub fn toggles(task_list_ids: &[String], lists: &[TaskList], query: &str) -> ListToggles {
    let mut ordered: Vec<&TaskList> = lists.iter().filter(|list| is_destination(list)).collect();
    ordered.sort_by(|a, b| {
        let favourite = |list: &TaskList| list.is_favorite.unwrap_or(false);
        favourite(b)
            .cmp(&favourite(a))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    let toggle = |list: &TaskList| ListToggle {
        id: list.id.clone(),
        name: list.name.clone(),
        color: list.display_color().to_string(),
        is_selected: task_list_ids.contains(&list.id),
    };
    let needle = query.to_lowercase();
    ListToggles {
        selected: ordered
            .iter()
            .filter(|list| task_list_ids.contains(&list.id))
            .map(|list| toggle(list))
            .collect(),
        options: ordered
            .iter()
            .filter(|list| needle.is_empty() || list.name.to_lowercase().contains(&needle))
            .map(|list| toggle(list))
            .collect(),
    }
}

/// The privacy a list created from a task's editor gets: PUBLIC over SHARED over PRIVATE,
/// from the lists the task is already in — the web's `handleCreateNewList`.
pub fn privacy_for_new_list<'a>(selected: impl IntoIterator<Item = &'a TaskList>) -> Privacy {
    let mut privacy = Privacy::Private;
    for list in selected {
        match list.privacy {
            Some(Privacy::Public) => return Privacy::Public,
            Some(Privacy::Shared) => privacy = Privacy::Shared,
            _ => {}
        }
    }
    privacy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, name: &str) -> TaskList {
        TaskList::new(id, name)
    }

    fn with_privacy(mut list: TaskList, privacy: Privacy) -> TaskList {
        list.privacy = Some(privacy);
        list
    }

    fn ids(picks: &[ListPick]) -> Vec<&str> {
        picks.iter().map(|pick| pick.id.as_str()).collect()
    }

    /// The editor offers the lists a task is not in, keeps the ones it is, and never offers a
    /// view (task d3f3b111). A board column is offered since D28 followed iOS.
    #[test]
    fn the_task_s_lists_are_selected_and_the_rest_are_offered_task_d3f3b111() {
        let mut column = list("ready", "Ready");
        column.list_type = Some("status".into());
        let mut today = list("today", "Today");
        today.is_virtual = Some(true);
        let lists = vec![
            list("work", "Work"),
            list("home", "Home"),
            column,
            today,
            list("garden", "Garden"),
        ];

        let picks = picks(&["home".to_string()], &lists, "");

        assert_eq!(ids(&picks.selected), vec!["home"]);
        assert_eq!(
            ids(&picks.options),
            vec!["garden", "ready", "work"],
            "by name, minus the selected and the view"
        );
        assert_eq!(picks.create_name, None, "nothing typed, nothing to create");
    }

    /// D28, resolved toward iOS (`InlineListsPicker.filteredLists`): only virtual lists are
    /// refused. A board column and a label are lists a task can be in, so they are offered.
    #[test]
    fn d28_list_picker_excludes_virtual_lists_only() {
        let mut column = list("ready", "Ready");
        column.list_type = Some("status".into());
        let mut label = list("bug", "Bug");
        label.list_type = Some("label".into());
        let mut today = list("today", "Today");
        today.is_virtual = Some(true);

        assert!(is_destination(&column));
        assert!(is_destination(&label));
        assert!(is_destination(&list("home", "Home")));
        assert!(!is_destination(&today));

        let picks = picks(&[], &[column, label, today, list("home", "Home")], "");
        assert_eq!(ids(&picks.options), vec!["bug", "home", "ready"]);
    }

    #[test]
    fn typing_narrows_the_offer_case_insensitively() {
        let lists = vec![
            list("work", "Work"),
            list("home", "Home"),
            list("homework", "Homework"),
        ];

        let picks = picks(&[], &lists, "HOME");

        assert_eq!(ids(&picks.options), vec!["home", "homework"]);
    }

    #[test]
    fn a_name_no_list_has_is_offered_for_creation_and_an_existing_one_is_not() {
        let lists = vec![list("home", "Home")];

        assert_eq!(
            picks(&[], &lists, "Garden ").create_name.as_deref(),
            Some("Garden")
        );
        assert_eq!(
            picks(&[], &lists, "home").create_name,
            None,
            "Home exists, whatever the case"
        );
        assert_eq!(picks(&[], &lists, "   ").create_name, None);
    }

    #[test]
    fn no_more_than_ten_are_offered() {
        let lists: Vec<TaskList> = (0..25)
            .map(|i| list(&format!("l{i}"), &format!("List {i:02}")))
            .collect();

        assert_eq!(picks(&[], &lists, "").options.len(), MAX_OPTIONS);
    }

    /// A list the task points at but the cache has never heard of is not drawn: there is no name
    /// to draw it with, and the id alone would be a chip reading like a bug.
    #[test]
    fn an_unknown_list_id_is_not_a_chip() {
        let picks = picks(&["gone".to_string()], &[list("home", "Home")], "");
        assert!(picks.selected.is_empty());
    }

    #[test]
    fn privacy_follows_the_task_s_lists_public_over_shared_over_private() {
        let private = list("a", "A");
        let shared = with_privacy(list("b", "B"), Privacy::Shared);
        let public = with_privacy(list("c", "C"), Privacy::Public);

        assert_eq!(privacy_for_new_list([&private]), Privacy::Private);
        assert_eq!(privacy_for_new_list([&private, &shared]), Privacy::Shared);
        assert_eq!(
            privacy_for_new_list([&shared, &public, &private]),
            Privacy::Public
        );
        assert_eq!(privacy_for_new_list([]), Privacy::Private);
    }

    #[test]
    fn a_random_colour_is_one_of_the_web_s_eight() {
        for _ in 0..50 {
            assert!(LIST_COLOR_PALETTE.contains(&random_list_color()));
        }
    }
}
