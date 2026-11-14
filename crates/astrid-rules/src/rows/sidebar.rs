//! The sidebar: which places it offers, in what order, with what number beside each.
//!
//! Every native shell used to assemble this itself, and one had already diverged: Android's
//! `SidebarViewModel.kt` hid only board columns and labels, so GitHub milestones and iterations
//! appeared as lists you could open and file a task into (AATD-89). Windows filtered and sorted in
//! C# with `CurrentCultureIgnoreCase`, and iOS sorted again in Swift. Three answers to one
//! product question, which is the shape of bug rule 8 of README.md exists to prevent.
//!
//! The question answered here is *which places, in what order* — not how a row is drawn. A
//! section is a list of [`TaskList`]s, as the clients already bind them, so adopting this is
//! deleting a filter and a sort rather than taking a new model.

use std::collections::HashMap;

use crate::model::TaskList;

/// The sidebar's sections, in the order they are drawn down the side.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Sections {
    /// My Tasks: what you are holding, wherever it lives. Above the favourites, because it is the
    /// view the app opens on rather than one of the lists. `None` when the caller did not ask for
    /// it — it is synthesised and has no row on the server, so only the caller can say.
    pub my_tasks: Option<TaskList>,
    /// Favourites, in the order the user arranged them. The web's "Saved Filters" section.
    pub favorites: Vec<TaskList>,
    /// Everything else that can hold a task, by name.
    pub lists: Vec<TaskList>,
}

/// Assemble the sidebar: drop what is not a place, split the favourites off, order both, and put
/// each entry's badge where the shells already read a number.
///
/// `badges` is the sidebar's number per list id — `listBadges`' answer. A list it does not mention
/// keeps whatever `task_count` it arrived with; the server's total is the wrong number for a
/// sidebar (task 8d37457b), so a caller that wants the badge drawn passes the badges.
///
/// Favourites fall back to the name when the order is missing, because a favourite saved before
/// ordering existed has no order at all and leaving those in whatever order they arrived reshuffles
/// the sidebar on every launch. This is [`crate::model::TaskList::is_domain_list`]'s list of
/// flavours and `ListService::favorites`' order, in one place both doors can reach.
pub fn sections(
    lists: &[TaskList],
    my_tasks: Option<TaskList>,
    badges: &HashMap<String, i64>,
) -> Sections {
    let badged = |list: &TaskList| {
        let mut entry = list.clone();
        if let Some(count) = badges.get(&entry.id) {
            entry.task_count = Some(*count);
        }
        entry
    };

    let places: Vec<TaskList> = lists
        .iter()
        .filter(|list| list.is_domain_list())
        .map(badged)
        .collect();

    let mut favorites: Vec<TaskList> = places
        .iter()
        .filter(|list| list.is_favorite.unwrap_or(false))
        .cloned()
        .collect();
    favorites.sort_by(|a, b| {
        a.favorite_order
            .unwrap_or(i64::MAX)
            .cmp(&b.favorite_order.unwrap_or(i64::MAX))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let mut rest: Vec<TaskList> = places
        .into_iter()
        .filter(|list| !list.is_favorite.unwrap_or(false))
        .collect();
    rest.sort_by_key(|list| list.name.to_lowercase());

    Sections {
        my_tasks: my_tasks.map(|entry| badged(&entry)),
        favorites,
        lists: rest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, name: &str) -> TaskList {
        TaskList::new(id, name)
    }

    fn flavoured(id: &str, name: &str, list_type: &str) -> TaskList {
        let mut entry = list(id, name);
        entry.list_type = Some(list_type.into());
        entry
    }

    fn favorite(id: &str, name: &str, order: Option<i64>) -> TaskList {
        let mut entry = list(id, name);
        entry.is_favorite = Some(true);
        entry.favorite_order = order;
        entry
    }

    fn names(lists: &[TaskList]) -> Vec<&str> {
        lists.iter().map(|list| list.name.as_str()).collect()
    }

    /// Task e22621e2: a milestone and an iteration are tags, not places. Android showed them,
    /// because it hid only `status` and `label` — the live bug this command exists to end.
    #[test]
    fn tags_and_columns_are_not_places() {
        let lists = vec![
            list("l1", "Home"),
            flavoured("l2", "Doing", "status"),
            flavoured("l3", "Bugs", "label"),
            flavoured("l4", "v2.1", "milestone"),
            flavoured("l5", "Sprint 4", "iteration"),
        ];

        let sidebar = sections(&lists, None, &HashMap::new());

        assert_eq!(names(&sidebar.lists), ["Home"]);
        assert!(sidebar.favorites.is_empty());
    }

    #[test]
    fn favorites_come_out_in_the_arranged_order_and_the_rest_by_name() {
        let lists = vec![
            list("l1", "work"),
            list("l2", "Home"),
            favorite("l3", "Zoo", Some(0)),
            favorite("l4", "Alps", Some(1)),
            favorite("l5", "beach", None),
            favorite("l6", "Attic", None),
        ];

        let sidebar = sections(&lists, None, &HashMap::new());

        assert_eq!(names(&sidebar.favorites), ["Zoo", "Alps", "Attic", "beach"]);
        assert_eq!(names(&sidebar.lists), ["Home", "work"]);
    }

    /// The number a sidebar is scanned for is the open tasks, not every task the list has ever
    /// held. It arrives where the shells already read one, so adopting this deletes a second call.
    #[test]
    fn a_badge_replaces_the_servers_total() {
        let mut home = list("l1", "Home");
        home.task_count = Some(124);
        let mut quiet = list("l2", "Quiet");
        quiet.task_count = Some(7);
        let mut my_tasks = TaskList::new(crate::filters::my_tasks::VIRTUAL_ID, "My Tasks");
        my_tasks.is_virtual = Some(true);

        let badges = HashMap::from([
            ("l1".to_string(), 12),
            (crate::filters::my_tasks::VIRTUAL_ID.to_string(), 3),
        ]);
        let sidebar = sections(&[home, quiet], Some(my_tasks), &badges);

        assert_eq!(sidebar.lists[0].task_count, Some(12));
        // Not counted: left as it arrived rather than silently zeroed.
        assert_eq!(sidebar.lists[1].task_count, Some(7));
        assert_eq!(
            sidebar.my_tasks.expect("asked for").task_count,
            Some(3),
            "My Tasks takes its badge like any other entry"
        );
    }

    /// A favourited saved filter is a place to go: web's sidebar draws it in the favourites
    /// section, and it is virtual rather than a tag.
    #[test]
    fn a_virtual_saved_filter_can_be_a_favorite() {
        let mut saved = favorite("virtual:today", "Today", Some(0));
        saved.is_virtual = Some(true);

        let sidebar = sections(&[saved], None, &HashMap::new());

        assert_eq!(names(&sidebar.favorites), ["Today"]);
    }

    #[test]
    fn the_sections_are_named_as_the_wire_names_them() {
        let json = serde_json::to_value(sections(&[list("l1", "Home")], None, &HashMap::new()))
            .expect("serialises");

        assert!(json.get("myTasks").is_some());
        assert!(json.get("favorites").is_some());
        assert_eq!(json["lists"][0]["name"], "Home");
    }
}
