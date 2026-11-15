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
//!
//! The Projects section (AWTD-1681) is the one exception: a project is not a list, so its entry is
//! a [`SidebarProject`] — the project's name and colour, and its lists as [`TaskList`]s. Which
//! projects, in what order, and which lists sit under each are decided in [`sections`] so the
//! shells adopting the Project view (web AWTD-1660, Windows, the Apple apps) cannot disagree about
//! a list in a stale nested copy, a project with no lists, or a favourite inside a project
//! (docs/CONTRACTS.md D70).

use std::collections::{HashMap, HashSet};

use crate::model::{Project, TaskList};

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
    /// Everything else that can hold a task, by name. A list in a project is here too, as on
    /// web: the Projects section is a second way in, not a move.
    pub lists: Vec<TaskList>,
    /// Projects, each with its lists (AWTD-1681). `None` when no section is drawn — Project Mode
    /// is off for this reader, or they can see no project (web draws no heading then either).
    /// Never an empty array.
    pub projects: Option<Vec<SidebarProject>>,
}

/// One project in the sidebar's Projects section (AWTD-1681): tapping it opens the project's
/// board, and its chevron expands to `lists`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SidebarProject {
    pub id: String,
    /// The project's name, else its key, else empty — web's `project.name || project.key || ""`.
    pub name: String,
    /// [`Project::display_color`]: the project's colour, or the default blue when it has none.
    pub color: String,
    /// The domain lists whose own `projectId` is this project, by name, badged like every other
    /// entry. Empty for a project with no lists, which still shows so it can get its first.
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
///
/// `projects` is `None` when the reader does not have Project Mode (the `project_mode` flag), and
/// otherwise every project they can see — `GET /api/v1/projects`, which answers the projects they
/// own or belong to. What the Projects section draws from it is [`project_section`]'s.
pub fn sections(
    lists: &[TaskList],
    my_tasks: Option<TaskList>,
    badges: &HashMap<String, i64>,
    projects: Option<&[Project]>,
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

    let projects = projects.and_then(|projects| project_section(projects, &places));

    let mut rest: Vec<TaskList> = places
        .into_iter()
        .filter(|list| !list.is_favorite.unwrap_or(false))
        .collect();
    rest.sort_by_key(|list| list.name.to_lowercase());

    Sections {
        my_tasks: my_tasks.map(|entry| badged(&entry)),
        projects,
        favorites,
        lists: rest,
    }
}

/// The Projects section (AWTD-1681, docs/CONTRACTS.md D70), over the places [`sections`] already
/// filtered and badged. The five decisions, each one web's `ProjectsSection.tsx` makes:
///
/// 1. **Every project the reader can see is a row**, including one with no lists: it must stay
///    reachable to be given its first. No projects at all draws no section.
/// 2. **By name**, case folded, then id — the project's name, the same for every reader. There is
///    no per-reader project order on the server to honour.
/// 3. **A list sits under the project its own `projectId` names**, and only that one. Never
///    [`Project::lists`] (a nested copy, stale whenever a list has moved since the projects were
///    read) and never a project's saved board rows (a list dragged onto another project's board is
///    a row there, not a member). Repeats of one list collapse to the first; a list naming a
///    project the reader cannot see is only in Lists. Ordered by name, like Lists.
/// 4. **A favourited list shows under its project as well as in Favorites** — the two sections
///    answer different questions, and web draws both.
/// 5. **No project is hidden or archived**: the server's `Project` has no such field, so there is
///    nothing to filter. One arriving later is a decision to make here, not in a shell.
fn project_section(projects: &[Project], places: &[TaskList]) -> Option<Vec<SidebarProject>> {
    let mut seen_projects = HashSet::new();
    let mut section: Vec<SidebarProject> = projects
        .iter()
        .filter(|project| seen_projects.insert(project.id.as_str()))
        .map(|project| {
            let mut seen_lists = HashSet::new();
            let mut lists: Vec<TaskList> = places
                .iter()
                .filter(|list| list.project_id.as_deref() == Some(project.id.as_str()))
                .filter(|list| seen_lists.insert(list.id.as_str()))
                .cloned()
                .collect();
            lists.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.id.cmp(&b.id))
            });
            let name = Some(project.name.as_str())
                .filter(|name| !name.is_empty())
                .or(project.key.as_deref())
                .unwrap_or_default()
                .to_string();
            SidebarProject {
                id: project.id.clone(),
                name,
                color: project.display_color().to_string(),
                lists,
            }
        })
        .collect();
    if section.is_empty() {
        return None;
    }
    section.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Some(section)
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

        let sidebar = sections(&lists, None, &HashMap::new(), None);

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

        let sidebar = sections(&lists, None, &HashMap::new(), None);

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
        let sidebar = sections(&[home, quiet], Some(my_tasks), &badges, None);

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

        let sidebar = sections(&[saved], None, &HashMap::new(), None);

        assert_eq!(names(&sidebar.favorites), ["Today"]);
    }

    fn project(id: &str, name: &str) -> Project {
        serde_json::from_value(serde_json::json!({ "id": id, "name": name })).expect("a project")
    }

    fn in_project(id: &str, name: &str, project_id: &str) -> TaskList {
        let mut entry = list(id, name);
        entry.project_id = Some(project_id.into());
        entry
    }

    fn project_names(sidebar: &Sections) -> Vec<&str> {
        sidebar
            .projects
            .as_deref()
            .expect("a Projects section")
            .iter()
            .map(|project| project.name.as_str())
            .collect()
    }

    fn lists_under<'a>(sidebar: &'a Sections, project_id: &str) -> Vec<&'a str> {
        let project = sidebar
            .projects
            .as_deref()
            .expect("a Projects section")
            .iter()
            .find(|project| project.id == project_id)
            .expect("the project is a row");
        names(&project.lists)
    }

    /// AWTD-1681: without Project Mode there is no section, whatever projects are cached; with it
    /// and no projects there is no section either — web draws no heading over nothing.
    #[test]
    fn no_project_mode_or_no_projects_draws_no_section_awtd_1681() {
        let lists = vec![in_project("l1", "Web", "p1")];
        let projects = vec![project("p1", "Astrid")];

        assert_eq!(sections(&lists, None, &HashMap::new(), None).projects, None);
        assert_eq!(
            sections(&lists, None, &HashMap::new(), Some(&[])).projects,
            None
        );
        assert!(sections(&lists, None, &HashMap::new(), Some(&projects))
            .projects
            .is_some());
    }

    /// AWTD-1681: a project with no lists is still a row, so it can be opened and given its first.
    #[test]
    fn a_project_with_no_lists_is_still_a_row_awtd_1681() {
        let projects = vec![project("p1", "Empty")];

        let sidebar = sections(
            &[list("l1", "Home")],
            None,
            &HashMap::new(),
            Some(&projects),
        );

        assert_eq!(project_names(&sidebar), ["Empty"]);
        assert!(lists_under(&sidebar, "p1").is_empty());
    }

    /// AWTD-1681: projects by name, case folded; lists under each by name; a project with no name
    /// takes its key, as web's `project.name || project.key` does.
    #[test]
    fn projects_and_their_lists_are_ordered_by_name_awtd_1681() {
        let mut keyed = project("p3", "");
        keyed.key = Some("MOB".into());
        let projects = vec![project("p1", "zeta"), project("p2", "Alpha"), keyed];
        let lists = vec![
            in_project("l1", "web", "p2"),
            in_project("l2", "API", "p2"),
            in_project("l3", "Docs", "p1"),
        ];

        let sidebar = sections(&lists, None, &HashMap::new(), Some(&projects));

        assert_eq!(project_names(&sidebar), ["Alpha", "MOB", "zeta"]);
        assert_eq!(lists_under(&sidebar, "p2"), ["API", "web"]);
        assert_eq!(lists_under(&sidebar, "p1"), ["Docs"]);
    }

    /// AWTD-1681: a list sits under the project its own `projectId` names. A project's nested
    /// `lists` is a copy from when projects were last read — here it still claims a list that has
    /// since moved to another project — and is not consulted, so the list appears once.
    #[test]
    fn a_list_claimed_by_two_projects_sits_under_its_own_project_awtd_1681() {
        let mut stale: Project = project("p1", "Old home");
        stale.lists = Some(vec![in_project("moved", "Moved", "p1")]);
        let projects = vec![stale, project("p2", "New home")];
        let lists = vec![
            in_project("moved", "Moved", "p2"),
            in_project("moved", "Moved", "p2"),
        ];

        let sidebar = sections(&lists, None, &HashMap::new(), Some(&projects));

        assert!(lists_under(&sidebar, "p1").is_empty());
        assert_eq!(
            lists_under(&sidebar, "p2"),
            ["Moved"],
            "once, though it arrived twice"
        );
    }

    /// AWTD-1681: a favourite inside a project is in Favorites and under its project; the Lists
    /// section keeps a project's other lists, as web's does.
    #[test]
    fn a_favourite_in_a_project_shows_in_favorites_and_under_the_project_awtd_1681() {
        let mut starred = in_project("fav", "Starred", "p1");
        starred.is_favorite = Some(true);
        starred.favorite_order = Some(0);
        let lists = vec![starred, in_project("plain", "Plain", "p1")];
        let projects = vec![project("p1", "Astrid")];

        let sidebar = sections(&lists, None, &HashMap::new(), Some(&projects));

        assert_eq!(names(&sidebar.favorites), ["Starred"]);
        assert_eq!(lists_under(&sidebar, "p1"), ["Plain", "Starred"]);
        assert_eq!(names(&sidebar.lists), ["Plain"]);
    }

    /// AWTD-1681: a board's columns and labels belong to the project but are not places, and a
    /// list naming a project the reader cannot see is only in Lists. Badges reach the lists under
    /// a project as they reach every other entry.
    #[test]
    fn only_places_sit_under_a_project_and_carry_their_badge_awtd_1681() {
        let mut column = flavoured("col", "Doing", "status");
        column.project_id = Some("p1".into());
        let mut label = flavoured("lab", "Bug", "label");
        label.project_id = Some("p1".into());
        let lists = vec![
            column,
            label,
            in_project("web", "Web", "p1"),
            in_project("hidden", "Elsewhere", "p-unseen"),
        ];
        let projects = vec![project("p1", "Astrid")];
        let badges = HashMap::from([("web".to_string(), 4)]);

        let sidebar = sections(&lists, None, &badges, Some(&projects));

        assert_eq!(lists_under(&sidebar, "p1"), ["Web"]);
        assert_eq!(
            sidebar.projects.as_deref().expect("a section")[0].lists[0].task_count,
            Some(4)
        );
        assert_eq!(names(&sidebar.lists), ["Elsewhere", "Web"]);
    }

    #[test]
    fn the_sections_are_named_as_the_wire_names_them() {
        let json =
            serde_json::to_value(sections(&[list("l1", "Home")], None, &HashMap::new(), None))
                .expect("serialises");

        assert!(json.get("myTasks").is_some());
        assert!(json.get("favorites").is_some());
        assert_eq!(json["lists"][0]["name"], "Home");
        assert!(
            json.get("projects")
                .is_some_and(|projects| projects.is_null()),
            "the section is present on the wire, null when not drawn (AWTD-1681)"
        );
    }
}
