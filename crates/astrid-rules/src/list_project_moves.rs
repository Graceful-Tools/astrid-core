//! Moving a list into or out of a project, and making a list inside one (astrid-ios AITD-680).
//!
//! Until this, the only control any client offered was *Create project board*, which always made
//! a **new** project, so a person with one project and several lists ended up with several
//! projects (Jon, 2026-11-15). The server could already move a list (`PUT /api/v1/lists/{id}`
//! with `projectId`, or `null` to detach); nothing asked it, and nothing decided who may.
//!
//! **Who may, D68 in `docs/CONTRACTS.md`:**
//!
//! - **Moving a list into or out of a project** needs owner or admin **of the list** *and* owner
//!   or admin **of every project it touches** — the one it joins and the one it leaves. Project
//!   membership cascades onto every list in a project, so attaching a list changes who can see
//!   it; a plain project member must not be able to pull somebody's list in and hand the whole
//!   project access to it, and a list's admin must not be able to drop it into a project they do
//!   not run.
//! - **Creating a list in a project** needs owner, admin or member of the project: the new list is
//!   the creator's own, so nothing of anybody else's is exposed.
//! - Viewers and non-members: nothing.
//!
//! The list side is the person's role **on the list itself** — ownership and list membership —
//! not the role the project cascades onto it: a project admin who is nobody on the list may not
//! carry it off. A board's status list is a column, never a movable list.
//!
//! Pure: the caller passes the person, the list and the projects it can see.

use serde::Serialize;

use crate::model::project::Project;
use crate::model::TaskList;
use crate::permissions::{access_of, role_in_list, ListRole};

/// A person's standing in a project, for the decisions here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectRole {
    Viewer,
    Member,
    Admin,
    Owner,
}

/// What a list's "Project" control may offer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListProjectMoves {
    /// May the list be detached from the project it is in? `false` for a list in no project.
    pub can_leave: bool,
    /// The projects it may move into — never its current one — ordered by name.
    pub destinations: Vec<String>,
    /// The projects this person may create a new list in, ordered by name.
    pub creatable_in: Vec<String>,
}

/// The person's role in `project`, or `None` for no standing at all.
///
/// The owner is `ownerId`. A membership row's role is read case-blind, as list roles are
/// (`permissions`, D6), and a row with a missing or unrecognised role is still a member:
/// presence is membership.
pub fn project_role(user_id: &str, project: &Project) -> Option<ProjectRole> {
    if user_id.is_empty() {
        return None;
    }
    if project.owner_id.as_deref() == Some(user_id)
        || project
            .owner
            .as_ref()
            .is_some_and(|owner| owner.id == user_id)
    {
        return Some(ProjectRole::Owner);
    }
    let membership = project
        .members
        .iter()
        .flatten()
        .find(|member| member.user_id == user_id)?;
    Some(match membership.role.to_lowercase().as_str() {
        "owner" => ProjectRole::Owner,
        "admin" => ProjectRole::Admin,
        "viewer" => ProjectRole::Viewer,
        _ => ProjectRole::Member,
    })
}

fn runs(user_id: &str, project: &Project) -> bool {
    project_role(user_id, project) >= Some(ProjectRole::Admin)
}

/// May this person create a list in `project`?
pub fn can_create_in(user_id: &str, project: &Project) -> bool {
    project_role(user_id, project) >= Some(ProjectRole::Member)
}

/// Owner or admin of the list by its own ownership and membership — never by the project's
/// cascade, which `access_of` leaves out because the list model does not carry it.
fn may_move_list(user_id: &str, list: &TaskList) -> bool {
    !list.is_status_list()
        && matches!(
            role_in_list(user_id, &access_of(list)),
            Some(ListRole::Owner) | Some(ListRole::Admin)
        )
}

/// May the list leave the project it is in (or, in none, is there nothing to leave)?
fn may_leave(user_id: &str, list: &TaskList, from: Option<&Project>) -> bool {
    match (list.project_id.as_deref(), from) {
        (None, _) => true,
        (Some(id), Some(project)) => project.id == id && runs(user_id, project),
        (Some(_), None) => false,
    }
}

/// May this person move `list` from the project it is in to `to` (`None` detaches)?
///
/// `from` is the list's current project as the caller holds it; a list in a project the caller
/// cannot find cannot be moved, because nobody can show it may leave. Staying put is not a move.
pub fn can_move(
    user_id: &str,
    list: &TaskList,
    from: Option<&Project>,
    to: Option<&Project>,
) -> bool {
    if list.project_id.as_deref() == to.map(|project| project.id.as_str()) {
        return false;
    }
    may_move_list(user_id, list)
        && may_leave(user_id, list, from)
        && to.is_none_or(|project| runs(user_id, project))
}

/// Projects by name, case-blind, then by id so equal names keep one order everywhere.
fn by_name(mut projects: Vec<&Project>) -> Vec<String> {
    projects.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    projects
        .into_iter()
        .map(|project| project.id.clone())
        .collect()
}

/// Everything a list's project control offers, at once. With no list, only `creatableIn`.
pub fn moves(user_id: &str, list: Option<&TaskList>, projects: &[Project]) -> ListProjectMoves {
    let creatable_in = by_name(
        projects
            .iter()
            .filter(|project| can_create_in(user_id, project))
            .collect(),
    );
    let Some(list) = list else {
        return ListProjectMoves {
            creatable_in,
            ..Default::default()
        };
    };
    let from = list
        .project_id
        .as_deref()
        .and_then(|id| projects.iter().find(|project| project.id == id));
    let destinations = by_name(
        projects
            .iter()
            .filter(|project| can_move(user_id, list, from, Some(project)))
            .collect(),
    );
    ListProjectMoves {
        can_leave: list.project_id.is_some() && can_move(user_id, list, from, None),
        destinations,
        creatable_in,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(id: &str, name: &str, owner: &str, members: serde_json::Value) -> Project {
        serde_json::from_value(
            json!({ "id": id, "name": name, "ownerId": owner, "members": members }),
        )
        .expect("a project")
    }

    fn list(project_id: Option<&str>, owner: &str, members: serde_json::Value) -> TaskList {
        serde_json::from_value(json!({
            "id": "l1", "name": "Groceries", "ownerId": owner, "projectId": project_id,
            "listMembers": members,
        }))
        .expect("a list")
    }

    fn projects() -> Vec<Project> {
        vec![
            project("pz", "Zebra", "me", json!([])),
            project(
                "pa",
                "alpha",
                "boss",
                json!([{ "userId": "me", "role": "ADMIN" }]),
            ),
            project(
                "pm",
                "Mid",
                "boss",
                json!([{ "userId": "me", "role": "member" }]),
            ),
            project(
                "pv",
                "Viewers",
                "boss",
                json!([{ "userId": "me", "role": "viewer" }]),
            ),
            project("px", "Elsewhere", "boss", json!([])),
        ]
    }

    #[test]
    fn project_roles_read_owner_admin_member_viewer_and_presence_aitd_680() {
        let all = projects();
        assert_eq!(project_role("me", &all[0]), Some(ProjectRole::Owner));
        assert_eq!(project_role("me", &all[1]), Some(ProjectRole::Admin));
        assert_eq!(project_role("me", &all[2]), Some(ProjectRole::Member));
        assert_eq!(project_role("me", &all[3]), Some(ProjectRole::Viewer));
        assert_eq!(project_role("me", &all[4]), None);
        let unlabelled = project("pu", "U", "boss", json!([{ "userId": "me" }]));
        assert_eq!(
            project_role("me", &unlabelled),
            Some(ProjectRole::Member),
            "presence is membership, as it is on a list"
        );
    }

    #[test]
    fn a_list_owner_may_move_their_list_into_projects_they_run_by_name_aitd_680() {
        let answer = moves("me", Some(&list(None, "me", json!([]))), &projects());
        assert!(
            !answer.can_leave,
            "a list in no project has nothing to leave"
        );
        assert_eq!(answer.destinations, vec!["pa", "pz"]);
        assert_eq!(answer.creatable_in, vec!["pa", "pm", "pz"]);
    }

    #[test]
    fn a_plain_list_member_moves_nothing_but_may_still_create_aitd_680() {
        let mine = list(
            None,
            "someone",
            json!([{ "userId": "me", "role": "member" }]),
        );
        let answer = moves("me", Some(&mine), &projects());
        assert!(answer.destinations.is_empty());
        assert_eq!(answer.creatable_in, vec!["pa", "pm", "pz"]);
    }

    #[test]
    fn a_list_admin_may_move_a_list_they_do_not_own_aitd_680() {
        let theirs = list(
            None,
            "someone",
            json!([{ "userId": "me", "role": "admin" }]),
        );
        assert_eq!(
            moves("me", Some(&theirs), &projects()).destinations,
            vec!["pa", "pz"]
        );
    }

    #[test]
    fn leaving_needs_admin_of_the_project_left_aitd_680() {
        let all = projects();
        let in_mine = moves("me", Some(&list(Some("pz"), "me", json!([]))), &all);
        assert!(in_mine.can_leave);
        assert_eq!(in_mine.destinations, vec!["pa"], "never its own project");

        // In a project where I am a plain member: I can neither detach it nor move it on.
        let in_theirs = moves("me", Some(&list(Some("pm"), "me", json!([]))), &all);
        assert!(!in_theirs.can_leave);
        assert!(in_theirs.destinations.is_empty());

        // In a project I cannot see at all.
        let unseen = moves("me", Some(&list(Some("gone"), "me", json!([]))), &all);
        assert!(!unseen.can_leave);
        assert!(unseen.destinations.is_empty());
    }

    #[test]
    fn a_project_admin_who_is_nobody_on_the_list_cannot_carry_it_off_aitd_680() {
        let theirs = list(Some("pz"), "someone", json!([]));
        let answer = moves("me", Some(&theirs), &projects());
        assert!(!answer.can_leave);
        assert!(answer.destinations.is_empty());
    }

    #[test]
    fn a_status_list_is_a_column_and_never_moves_aitd_680() {
        let mut column = list(Some("pz"), "me", json!([]));
        column.list_type = Some("status".into());
        let answer = moves("me", Some(&column), &projects());
        assert!(!answer.can_leave);
        assert!(answer.destinations.is_empty());
    }

    #[test]
    fn can_move_checks_both_ends_aitd_680() {
        let all = projects();
        let mine = list(Some("pz"), "me", json!([]));
        assert!(can_move("me", &mine, Some(&all[0]), Some(&all[1])));
        assert!(can_move("me", &mine, Some(&all[0]), None));
        assert!(!can_move("me", &mine, Some(&all[0]), Some(&all[2])));
        assert!(
            !can_move("me", &mine, None, Some(&all[1])),
            "its project unknown"
        );
        let loose = list(None, "me", json!([]));
        assert!(can_move("me", &loose, None, Some(&all[1])));
        assert!(
            !can_move("me", &loose, None, None),
            "nowhere to nowhere is not a move"
        );
    }

    #[test]
    fn creating_in_a_project_needs_membership_not_admin_aitd_680() {
        let all = projects();
        assert!(can_create_in("me", &all[2]));
        assert!(!can_create_in("me", &all[3]), "a viewer");
        assert!(!can_create_in("me", &all[4]), "a stranger");
    }

    #[test]
    fn no_list_still_answers_where_one_may_be_created_aitd_680() {
        let answer = moves("me", None, &projects());
        assert!(!answer.can_leave);
        assert!(answer.destinations.is_empty());
        assert_eq!(answer.creatable_in, vec!["pa", "pm", "pz"]);
    }
}
