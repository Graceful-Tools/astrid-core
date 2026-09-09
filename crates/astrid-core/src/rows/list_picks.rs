//! Which lists a task can be put in, for the detail pane's list editor (task d3f3b111).
//!
//! Ports the rules behind astrid-web's Lists field (`TaskFieldEditors.tsx`, `editingLists`, and
//! `handleCreateNewList` in `task-detail.tsx`). On Windows the Lists row drew chips and nothing
//! else, so the single most common edit after the title — moving a task between lists — could
//! only be done by re-creating the task.
//!
//! What is a rule here, and why:
//!
//! - **What is offered.** Only destinations: never a virtual list ("Today" is somewhere to look,
//!   not somewhere a task lives) and never a board column (a state, not a place). The web's
//!   `selectableLists` makes the same cut. Minus the lists the task is already in, filtered by
//!   what was typed, and no more than the first ten — the web's `.slice(0, 10)`.
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
pub fn random_list_color() -> &'static str {
    let index = (uuid::Uuid::new_v4().as_u128() % LIST_COLOR_PALETTE.len() as u128) as usize;
    LIST_COLOR_PALETTE[index]
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

/// Whether a task can be filed in this list at all.
///
/// The same cut `ListService::destinations` makes, stated once more here because this module
/// is handed every list the store has and has to make it itself.
pub fn is_destination(list: &TaskList) -> bool {
    list.is_domain_list() && !list.is_virtual.unwrap_or(false)
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
    /// column or a view (task d3f3b111).
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
            vec!["garden", "work"],
            "by name, minus the selected"
        );
        assert_eq!(picks.create_name, None, "nothing typed, nothing to create");
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
