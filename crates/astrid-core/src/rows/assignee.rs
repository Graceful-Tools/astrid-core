//! Who a task can be assigned to, and in what order.
//!
//! Ports `astrid-ios/Astrid App/Core/Layout/AssigneeOptions.swift` and `AssigneeResolver.swift`
//! (tasks 1484ea4a and 42013da7), together with what the Mac's `MacAssigneeOptions` adds.
//!
//! Both Apple platforms had already learnt that this is a rule and not a view detail: on iOS the
//! picker built its option list inline, so "who can this be assigned to" differed with whatever
//! the surrounding screen happened to have loaded, and the board could not offer an AI agent at
//! all. Here it is one function, so the detail pane, the row picker and later the board cannot
//! drift.
//!
//! ## The two Apple platforms disagree about the order
//!
//! iOS sorts agents first, then you, then everyone by name. The Mac offers "no one" first, sorts
//! you first, and has no agents at all — its picker predates agents being assignable. This takes
//! iOS's ordering, because that is the one written to stop the surfaces drifting, and the Mac's
//! unassigned row, because unassigned is a real choice and a picker that cannot express it cannot
//! clear an assignee. Recorded in `docs/CONTRACTS.md` as D8.
//!
//! ## AI agents are not list members
//!
//! They come from the account, not from the task's lists, so they are offered unconditionally —
//! including when the task's lists are not loaded at all, which is the board's case: its cards
//! carry a project list plus a status list, and neither is guaranteed to be present.
//!
//! Nothing here returns English. Unassigned is [`AssigneeOption::user_id`] being `None`, and the
//! shell names it from its `.resw` resources.

use serde::Serialize;

use crate::model::{TaskList, User};

/// One row of the picker. `user_id: None` is the unassigned choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssigneeOption {
    pub user_id: Option<String>,
    /// What to show where a name goes, or `None` for the unassigned row — which the shell names
    /// from its resources rather than being handed a word.
    pub name: Option<String>,
    /// Avatar fallback. Empty for the unassigned row, which has its own glyph.
    pub initials: String,
    pub image: Option<String>,
    pub is_current_user: bool,
    pub is_agent: bool,
}

impl AssigneeOption {
    /// The unassigned row, which every picker offers first.
    pub fn unassigned() -> Self {
        AssigneeOption {
            user_id: None,
            name: None,
            initials: String::new(),
            image: None,
            is_current_user: false,
            is_agent: false,
        }
    }

    fn person(user: &User, current_user_id: Option<&str>) -> Self {
        AssigneeOption {
            user_id: Some(user.id.clone()),
            name: Some(user.display_name().to_string()),
            initials: user.initials(),
            image: user.image.clone(),
            is_current_user: current_user_id == Some(user.id.as_str()),
            is_agent: user.is_agent(),
        }
    }
}

/// Everything the picker needs to know about the surface asking.
#[derive(Debug, Default)]
pub struct AssigneeSources<'a> {
    /// The lists this surface has loaded. Only those the task is on contribute members.
    pub lists: &'a [TaskList],
    /// The lists the task is on.
    pub task_list_ids: &'a [String],
    /// People found some other way — a scoped search, or someone just added by email (8ffe30ce).
    pub discovered: &'a [User],
    /// The account's AI agents. Not list members; see the module note.
    pub agents: &'a [User],
    /// Who holds the task now, when that is someone the lists do not carry. Without this the
    /// picker cannot represent its own current value.
    pub current_assignee: Option<&'a User>,
    pub current_user: Option<&'a User>,
}

/// Build the picker's rows: unassigned, then agents, then you, then everyone else by name.
pub fn options(sources: &AssigneeSources<'_>) -> Vec<AssigneeOption> {
    let mut people: Vec<User> = Vec::new();
    let mut note = |user: &User| match people.iter_mut().find(|held| held.id == user.id) {
        // The richest record wins: a hydrated member beats the bare id a board card carries.
        Some(held) if held.name.is_none() => *held = user.clone(),
        Some(_) => {}
        None => people.push(user.clone()),
    };

    let task_lists: Vec<&TaskList> = sources
        .lists
        .iter()
        .filter(|list| sources.task_list_ids.contains(&list.id))
        .collect();
    for list in &task_lists {
        if let Some(owner) = &list.owner {
            note(owner);
        }
        for member in list.list_members.iter().flatten() {
            match &member.user {
                Some(user) => note(user),
                // A member whose user never hydrated is still a person, never a raw id on screen:
                // a minimal record still renders initials and still resolves a cached photo.
                None => note(&User::new(&member.user_id)),
            }
        }
    }

    for user in sources.discovered {
        note(user);
    }
    for agent in sources.agents {
        note(agent);
    }
    if let Some(assignee) = sources.current_assignee {
        note(assignee);
    }
    // A task with no resolvable lists — "My Tasks", or a board card whose lists this screen has
    // not loaded — still has to offer you, or the picker comes up empty.
    if task_lists.is_empty() {
        if let Some(me) = sources.current_user {
            note(me);
        }
    }

    let current_user_id = sources.current_user.map(|user| user.id.as_str());
    let mut rows: Vec<AssigneeOption> = people
        .iter()
        .map(|user| AssigneeOption::person(user, current_user_id))
        .collect();
    rows.sort_by(|a, b| {
        let name = |option: &AssigneeOption| option.name.clone().unwrap_or_default().to_lowercase();
        b.is_agent
            .cmp(&a.is_agent)
            .then_with(|| b.is_current_user.cmp(&a.is_current_user))
            .then_with(|| name(a).cmp(&name(b)))
            // Id as the tiebreaker, so the order is stable rather than the order they arrived in.
            .then_with(|| a.user_id.cmp(&b.user_id))
    });

    let mut all = vec![AssigneeOption::unassigned()];
    all.append(&mut rows);
    all
}

/// Turn an assignee id into the richest [`User`] any of the pools holds for it.
///
/// The avatar used to be resolved from the current list's members alone, so picking anyone that
/// list did not hold resolved to nothing and the view kept drawing whoever it had drawn last —
/// the "sometimes the profile photo doesn't change" report (42013da7). Every pool is consulted in
/// the order given, and an unknown id still produces a user rather than nothing.
///
/// The id being asked for always wins over the task's embedded assignee: a stale embedded record
/// is precisely how the previous person stays on screen.
pub fn resolve(id: Option<&str>, pools: &[&[User]], task_assignee: Option<&User>) -> Option<User> {
    let id = id.filter(|id| !id.is_empty())?;
    for pool in pools {
        if let Some(found) = pool.iter().find(|user| user.id == id) {
            return Some(found.clone());
        }
    }
    if let Some(assignee) = task_assignee.filter(|assignee| assignee.id == id) {
        return Some(assignee.clone());
    }
    Some(User::new(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::list::ListMember;

    fn user(id: &str, name: &str) -> User {
        User {
            name: Some(name.into()),
            email: Some(format!("{id}@astrid.cc")),
            ..User::new(id)
        }
    }

    fn agent(id: &str, name: &str) -> User {
        User {
            is_ai_agent: Some(true),
            ..user(id, name)
        }
    }

    fn member(list_id: &str, user_id: &str, user: Option<&User>) -> ListMember {
        ListMember {
            id: Some(format!("lm-{user_id}")),
            list_id: Some(list_id.into()),
            user_id: user_id.into(),
            role: "MEMBER".into(),
            created_at: None,
            updated_at: None,
            user: user.cloned(),
        }
    }

    fn list(id: &str, owner: Option<&User>, members: &[User]) -> TaskList {
        TaskList {
            owner: owner.cloned(),
            list_members: Some(
                members
                    .iter()
                    .map(|person| member(id, &person.id, Some(person)))
                    .collect(),
            ),
            ..TaskList::new(id, format!("List {id}"))
        }
    }

    fn ids(options: &[AssigneeOption]) -> Vec<&str> {
        options
            .iter()
            .filter_map(|option| option.user_id.as_deref())
            .collect()
    }

    /// THE BUG (1484ea4a): agents belong in the list even when the task's lists resolve to
    /// nothing, which is the board's case.
    #[test]
    fn agents_are_offered_even_when_the_tasks_lists_are_not_loaded() {
        let me = user("me", "Jon");
        let list_ids = vec!["project-list".to_string(), "status-doing".to_string()];
        let agents = vec![agent("agent-claude", "Claude")];
        let found = options(&AssigneeSources {
            task_list_ids: &list_ids,
            agents: &agents,
            current_user: Some(&me),
            ..Default::default()
        });
        assert!(ids(&found).contains(&"agent-claude"));
    }

    /// Task 8ffe30ce: people found by a scoped search fill the picker before the lists load.
    #[test]
    fn people_found_some_other_way_are_offered() {
        let me = user("me", "Jon");
        let lists = vec![TaskList::new("project-list", "Project")];
        let list_ids = vec!["project-list".to_string(), "status-doing".to_string()];
        let discovered = vec![user("dana", "Dana")];
        let found = options(&AssigneeSources {
            lists: &lists,
            task_list_ids: &list_ids,
            discovered: &discovered,
            current_user: Some(&me),
            ..Default::default()
        });
        assert!(ids(&found).contains(&"dana"));
    }

    /// Agents first, then you, then everyone else by name. Stating the order here is what stops
    /// one surface sorting differently from another.
    #[test]
    fn agents_come_first_then_you_then_the_rest_by_name() {
        let me = user("me", "Jon");
        let lists = vec![list(
            "l",
            Some(&me),
            &[user("zoe", "Zoe"), user("amy", "Amy")],
        )];
        let list_ids = vec!["l".to_string()];
        let agents = vec![agent("agent-b", "Beta"), agent("agent-a", "Alpha")];
        let found = options(&AssigneeSources {
            lists: &lists,
            task_list_ids: &list_ids,
            agents: &agents,
            current_user: Some(&me),
            ..Default::default()
        });
        assert_eq!(ids(&found), vec!["agent-a", "agent-b", "me", "amy", "zoe"]);
    }

    /// Unassigned is a real choice: a picker that cannot express it cannot clear an assignee.
    #[test]
    fn unassigned_is_always_offered_first() {
        let found = options(&AssigneeSources::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].user_id, None);
        // No word — the shell names it from its resources.
        assert_eq!(found[0].name, None);
    }

    /// A task with no lists at all still offers you, or "My Tasks" has an empty picker.
    #[test]
    fn a_task_with_no_lists_still_offers_you() {
        let me = user("me", "Jon");
        let found = options(&AssigneeSources {
            current_user: Some(&me),
            ..Default::default()
        });
        assert_eq!(ids(&found), vec!["me"]);
    }

    #[test]
    fn a_person_on_two_of_the_tasks_lists_is_offered_once() {
        let me = user("me", "Jon");
        let dana = user("dana", "Dana");
        let lists = vec![
            list("a", Some(&me), std::slice::from_ref(&dana)),
            list("b", Some(&me), std::slice::from_ref(&dana)),
        ];
        let list_ids = vec!["a".to_string(), "b".to_string()];
        let found = options(&AssigneeSources {
            lists: &lists,
            task_list_ids: &list_ids,
            current_user: Some(&me),
            ..Default::default()
        });
        assert_eq!(ids(&found).iter().filter(|id| **id == "dana").count(), 1);
    }

    /// Someone assigned from outside this list — added by email, a member of another list — must
    /// still appear, or the picker cannot show who holds the task.
    #[test]
    fn the_current_assignee_appears_even_when_they_are_not_a_member() {
        let outsider = user("outsider", "Outside Person");
        let lists = vec![list("l", None, &[user("u1", "Adam")])];
        let list_ids = vec!["l".to_string()];
        let found = options(&AssigneeSources {
            lists: &lists,
            task_list_ids: &list_ids,
            current_assignee: Some(&outsider),
            ..Default::default()
        });
        assert!(ids(&found).contains(&"outsider"));
    }

    #[test]
    fn a_member_who_is_also_the_assignee_appears_once() {
        let adam = user("u1", "Adam");
        let lists = vec![list("l", None, std::slice::from_ref(&adam))];
        let list_ids = vec!["l".to_string()];
        let found = options(&AssigneeSources {
            lists: &lists,
            task_list_ids: &list_ids,
            current_assignee: Some(&adam),
            ..Default::default()
        });
        assert_eq!(ids(&found), vec!["u1"]);
    }

    /// A member whose user record never hydrated is still a person. The Mac put the raw id on
    /// screen as if it were somebody's name.
    #[test]
    fn an_unhydrated_member_is_never_labelled_with_a_raw_id() {
        let raw = "8f14e45f-ceea-467a-9f8b-2d3c7f9a1b2c";
        let lists = vec![TaskList {
            list_members: Some(vec![member("l", raw, None)]),
            ..TaskList::new("l", "List")
        }];
        let list_ids = vec!["l".to_string()];
        let found = options(&AssigneeSources {
            lists: &lists,
            task_list_ids: &list_ids,
            ..Default::default()
        });
        let person = &found[1];
        assert_ne!(person.name.as_deref(), Some(raw));
        assert!(!person.initials.is_empty());
    }

    /// A hydrated record beats a bare one for the same person, whichever order they arrive in.
    #[test]
    fn the_richest_record_of_a_person_wins() {
        let lists = vec![TaskList {
            list_members: Some(vec![member("l", "dana", None)]),
            ..TaskList::new("l", "List")
        }];
        let list_ids = vec!["l".to_string()];
        let discovered = vec![user("dana", "Dana")];
        let found = options(&AssigneeSources {
            lists: &lists,
            task_list_ids: &list_ids,
            discovered: &discovered,
            ..Default::default()
        });
        assert_eq!(found[1].name.as_deref(), Some("Dana"));
    }

    /// THE BUG (42013da7): an id no pool holds must still resolve to that person, or the view
    /// keeps whoever it drew last.
    #[test]
    fn an_unknown_id_still_resolves_to_someone() {
        let others = vec![user("someone-else", "Else")];
        let resolved = resolve(Some("new-person"), &[others.as_slice()], None).expect("a user");
        assert_eq!(resolved.id, "new-person");
    }

    #[test]
    fn the_richest_pool_wins() {
        let members = vec![User {
            image: Some("https://example.com/dana.jpg".into()),
            ..user("u1", "Dana")
        }];
        let embedded = User::new("u1");
        let resolved = resolve(Some("u1"), &[members.as_slice()], Some(&embedded)).expect("a user");
        assert_eq!(resolved.name.as_deref(), Some("Dana"));
        assert_eq!(
            resolved.image.as_deref(),
            Some("https://example.com/dana.jpg")
        );
    }

    #[test]
    fn it_falls_back_to_the_tasks_own_assignee() {
        let sam = user("u2", "Sam");
        let resolved = resolve(Some("u2"), &[], Some(&sam)).expect("a user");
        assert_eq!(resolved.name.as_deref(), Some("Sam"));
    }

    /// A stale embedded assignee must never win over the id being asked for — that is the exact
    /// shape of "the photo shows the previous person".
    #[test]
    fn a_stale_embedded_assignee_is_ignored() {
        let previous = user("old-person", "Previous");
        let resolved = resolve(Some("new-person"), &[], Some(&previous)).expect("a user");
        assert_eq!(resolved.id, "new-person");
        assert_ne!(resolved.name.as_deref(), Some("Previous"));
    }

    /// An agent assignee is not a list member, so without its own pool it resolved to a bare id.
    #[test]
    fn it_resolves_from_the_agent_pool_too() {
        let members: Vec<User> = Vec::new();
        let agents = vec![agent("agent-1", "Astrid")];
        let resolved = resolve(
            Some("agent-1"),
            &[members.as_slice(), agents.as_slice()],
            None,
        )
        .expect("a user");
        assert_eq!(resolved.initials(), "AS");
    }

    #[test]
    fn unassigned_resolves_to_nobody() {
        let members = vec![user("u1", "Adam")];
        assert!(resolve(None, &[members.as_slice()], None).is_none());
        // An empty id is unassigned, not a user whose id is the empty string.
        assert!(resolve(Some(""), &[], None).is_none());
    }
}
