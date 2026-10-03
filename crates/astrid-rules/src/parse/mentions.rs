//! `@person`, `#list` and `!task` in a comment box (task 3271a0c5).
//!
//! Ports `astrid-web/hooks/use-chat-mentions.ts` and the `mentionableUsers` rule in
//! `components/task-detail/CommentInputBar.tsx`. On Windows the comment box was a plain TextBox,
//! so `@astrid` typed there was just text — and that is the one way to hand a task to the agent
//! from inside it.
//!
//! Three rules live here, because each is something the two clients must agree on byte for byte:
//!
//! - **When a popup opens.** The trigger character closest to the caret, at the start of the
//!   text or after whitespace, with no whitespace between it and the caret. A finished
//!   `@[Jon](id)` earlier in the line does not re-open anything.
//! - **What goes into the text.** `@[Name](id)`, `#[Name](id)` or `![Name](id)`, replacing the
//!   trigger and what was typed after it, plus one space unless one is already there. The server
//!   resolves exactly that form and nothing else — see `lib/markdown.ts`.
//! - **Who and what is offered.** People: the task's creator and assignee, the owners, members
//!   and admins of the lists it is on, and the account's agents — never the reader. Lists: real
//!   ones, ten at most. Tasks: those in the open list first, then the rest, unfinished before
//!   finished and newest edit first, fifteen at most.
//!
//! Positions are UTF-16 code units, because that is what a TextBox's caret counts in and what
//! the web's `substring` counts in; a comment with an emoji in it must not land the pill one
//! character off.

use serde::{Deserialize, Serialize};

use crate::model::{Task, TaskList, User};

/// Which kind of thing a trigger asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerKind {
    Mention,
    List,
    Task,
}

impl TriggerKind {
    fn sigil(self) -> char {
        match self {
            TriggerKind::Mention => '@',
            TriggerKind::List => '#',
            TriggerKind::Task => '!',
        }
    }

    fn of(sigil: u16) -> Option<Self> {
        match sigil {
            0x40 => Some(TriggerKind::Mention),
            0x23 => Some(TriggerKind::List),
            0x21 => Some(TriggerKind::Task),
            _ => None,
        }
    }
}

/// A trigger the caret is inside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub kind: TriggerKind,
    /// Where the sigil sits, in UTF-16 units.
    pub start: usize,
    /// What has been typed after it, as typed.
    pub query: String,
}

fn is_space(unit: u16) -> bool {
    char::from_u32(unit as u32).is_some_and(char::is_whitespace)
}

/// The trigger the caret is inside, if any — web's `handleTextChange`.
pub fn find_trigger(text: &str, caret: usize) -> Option<Trigger> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let caret = caret.min(units.len());
    let before = &units[..caret];
    let mut best: Option<Trigger> = None;
    for (index, unit) in before.iter().enumerate().rev() {
        let Some(kind) = TriggerKind::of(*unit) else {
            continue;
        };
        // Only the LAST occurrence of each sigil counts, as `lastIndexOf` reads it.
        if best.as_ref().is_some_and(|held| held.kind == kind) {
            continue;
        }
        if index > 0 && !is_space(before[index - 1]) {
            continue;
        }
        let search = &before[index + 1..];
        if search.iter().any(|unit| is_space(*unit)) {
            continue;
        }
        let candidate = Trigger {
            kind,
            start: index,
            query: String::from_utf16_lossy(search),
        };
        match &best {
            Some(held) if held.start >= candidate.start => {}
            _ => best = Some(candidate),
        }
        // Everything earlier is further from the caret; only a different sigil could still
        // matter, and only if it were closer, which it cannot be.
        if best.as_ref().is_some_and(|held| held.start == index) && index < before.len() {
            // keep scanning: a different sigil earlier is never closer, so we can stop.
            break;
        }
    }
    best
}

/// Put a chosen item into the text — web's `insertAutocompleteItem`.
///
/// Answers the new text and where the caret goes, in UTF-16 units. With no trigger at the caret
/// the text comes back untouched.
pub fn insert(
    text: &str,
    caret: usize,
    kind: TriggerKind,
    label: &str,
    id: &str,
) -> (String, usize) {
    let Some(trigger) = find_trigger(text, caret) else {
        return (text.to_string(), caret);
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let query_len = trigger.query.encode_utf16().count();
    let before = &units[..trigger.start];
    let after = &units[(trigger.start + 1 + query_len).min(units.len())..];
    let reference = format!("{}[{}]({})", kind.sigil(), label, id);
    let separator = if after.first().is_some_and(|unit| *unit == 0x20) {
        ""
    } else {
        " "
    };
    let inserted = format!("{reference}{separator}");
    let mut new_units = before.to_vec();
    new_units.extend(inserted.encode_utf16());
    let new_caret = new_units.len();
    new_units.extend_from_slice(after);
    (String::from_utf16_lossy(&new_units), new_caret)
}

/// One row of the popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub kind: TriggerKind,
    pub id: String,
    /// What is drawn, and what goes into the reference.
    pub label: String,
    /// A person's email, a list's sharing, a task's first list.
    pub secondary: Option<String>,
    pub is_agent: bool,
    pub completed: bool,
}

/// What the popup draws from.
pub struct Sources<'a> {
    /// The task the comment is on.
    pub task: &'a Task,
    /// Every list the cache has, for the task's own and for `#`.
    pub lists: &'a [TaskList],
    /// Every task the cache has, for `!`.
    pub tasks: &'a [Task],
    /// Everyone the cache knows; the agents among them are offered.
    pub users: &'a [User],
    /// The reader, who is never offered to themselves.
    pub me: Option<&'a str>,
    /// The list open on screen, whose tasks come first for `!`.
    pub selected_list_id: Option<&'a str>,
}

/// The rows for a trigger and what was typed after it.
pub fn suggestions(kind: TriggerKind, query: &str, sources: &Sources<'_>) -> Vec<Suggestion> {
    let needle = query.trim().to_lowercase();
    match kind {
        TriggerKind::Mention => mentionable(sources)
            .into_iter()
            .filter(|user| {
                needle.is_empty()
                    || user
                        .name
                        .as_deref()
                        .is_some_and(|name| name.to_lowercase().contains(&needle))
                    || user
                        .email
                        .as_deref()
                        .is_some_and(|email| email.to_lowercase().contains(&needle))
            })
            .map(|user| Suggestion {
                kind,
                id: user.id.clone(),
                label: user.display_name().to_string(),
                secondary: match (&user.name, &user.email) {
                    (Some(name), Some(email)) if !name.trim().is_empty() => Some(email.clone()),
                    _ => None,
                },
                is_agent: user.is_agent(),
                completed: false,
            })
            .collect(),
        TriggerKind::List => sources
            .lists
            .iter()
            .filter(|list| !list.is_virtual.unwrap_or(false) && list.is_domain_list())
            .filter(|list| needle.is_empty() || list.name.to_lowercase().contains(&needle))
            .take(10)
            .map(|list| Suggestion {
                kind,
                id: list.id.clone(),
                label: list.name.clone(),
                secondary: match list.privacy {
                    Some(crate::model::Privacy::Shared) => Some("Shared".into()),
                    Some(crate::model::Privacy::Public) => Some("Public".into()),
                    _ => None,
                },
                is_agent: false,
                completed: false,
            })
            .collect(),
        TriggerKind::Task => {
            let matching = sources
                .tasks
                .iter()
                .filter(|task| needle.is_empty() || task.title.to_lowercase().contains(&needle));
            let (mut in_list, mut elsewhere): (Vec<&Task>, Vec<&Task>) =
                matching.partition(|task| {
                    sources.selected_list_id.is_some_and(|selected| {
                        task.effective_list_ids().iter().any(|id| id == selected)
                    })
                });
            let order = |a: &&Task, b: &&Task| {
                a.completed
                    .cmp(&b.completed)
                    .then_with(|| b.updated_at.cmp(&a.updated_at))
            };
            in_list.sort_by(order);
            elsewhere.sort_by(order);
            in_list
                .into_iter()
                .chain(elsewhere)
                .take(15)
                .map(|task| Suggestion {
                    kind,
                    id: task.id.clone(),
                    label: task.title.clone(),
                    secondary: task
                        .effective_list_ids()
                        .first()
                        .and_then(|id| sources.lists.iter().find(|list| &list.id == id))
                        .map(|list| list.name.clone()),
                    is_agent: false,
                    completed: task.completed,
                })
                .collect()
        }
    }
}

/// Who can be mentioned on a task — web's `mentionableUsers`, in its order: the agents, the
/// creator, the assignee, then each list's owner, members and admins; never the reader.
fn mentionable(sources: &Sources<'_>) -> Vec<User> {
    let mut people: Vec<User> = Vec::new();
    let mut note = |user: &User| {
        if people.iter().any(|held| held.id == user.id) {
            return;
        }
        people.push(user.clone());
    };
    for agent in sources.users.iter().filter(|user| user.is_agent()) {
        note(agent);
    }
    if let Some(creator) = &sources.task.creator {
        note(creator);
    }
    if let Some(assignee) = &sources.task.assignee {
        note(assignee);
    }
    let on = sources.task.effective_list_ids();
    let task_lists: Vec<&TaskList> = sources
        .task
        .lists
        .iter()
        .flatten()
        .chain(
            sources
                .lists
                .iter()
                .filter(|list| on.iter().any(|id| id == &list.id)),
        )
        .collect();
    for list in task_lists {
        if let Some(owner) = &list.owner {
            note(owner);
        }
        for member in list.members.iter().flatten() {
            note(member);
        }
        for member in list.list_members.iter().flatten() {
            if let Some(user) = &member.user {
                note(user);
            }
        }
        for admin in list.admins.iter().flatten() {
            note(admin);
        }
    }
    people
        .into_iter()
        .filter(|user| sources.me != Some(user.id.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn caret_at_end(text: &str) -> usize {
        text.encode_utf16().count()
    }

    /// `@astrid` typed at the end opens the mention popup, with what was typed as the query
    /// (task 3271a0c5).
    #[test]
    fn a_sigil_at_the_caret_opens_a_popup_task_3271a0c5() {
        let text = "hey @ast";
        let found = find_trigger(text, caret_at_end(text)).expect("a trigger");
        assert_eq!(found.kind, TriggerKind::Mention);
        assert_eq!(found.start, 4);
        assert_eq!(found.query, "ast");

        let text = "#";
        let found = find_trigger(text, 1).expect("a bare sigil counts");
        assert_eq!(found.kind, TriggerKind::List);
        assert_eq!(found.query, "");
    }

    #[test]
    fn a_sigil_inside_a_word_or_past_a_space_is_not_a_trigger() {
        assert!(
            find_trigger("email me@example.com", 20).is_none(),
            "inside a word"
        );
        assert!(
            find_trigger("@jon said hi", 12).is_none(),
            "moved on past a space"
        );
        assert!(find_trigger("nothing here", 12).is_none());
    }

    /// The trigger closest to the caret wins, and only the last of each sigil is looked at.
    #[test]
    fn the_closest_trigger_wins() {
        let text = "@jon see #hea";
        let found = find_trigger(text, caret_at_end(text)).expect("a trigger");
        assert_eq!(found.kind, TriggerKind::List);
        assert_eq!(found.query, "hea");
    }

    /// The caret, not the end of the text, decides: text after the caret is not the query.
    #[test]
    fn only_the_text_before_the_caret_is_the_query() {
        let found = find_trigger("@jo later words", 3).expect("a trigger");
        assert_eq!(found.query, "jo");
    }

    /// The exact form the server resolves, replacing the trigger and the query, plus a space
    /// unless one is already there. Positions are UTF-16 units, so an emoji before the caret
    /// does not shift the pill.
    #[test]
    fn choosing_inserts_the_reference_the_server_understands() {
        let (text, caret) = insert(
            "hey @ast",
            8,
            TriggerKind::Mention,
            "Astrid",
            "ai-agent-astrid",
        );
        assert_eq!(text, "hey @[Astrid](ai-agent-astrid) ");
        assert_eq!(caret, text.encode_utf16().count());

        let (text, caret) = insert("see #hea please", 8, TriggerKind::List, "Health", "l1");
        assert_eq!(text, "see #[Health](l1) please", "one space, not two");
        // The caret sits right after the pill; the existing space is the separator.
        assert_eq!(caret, "see #[Health](l1)".encode_utf16().count());

        let with_emoji = "❤️ !bu";
        let (text, caret) = insert(
            with_emoji,
            caret_at_end(with_emoji),
            TriggerKind::Task,
            "Buy milk",
            "t1",
        );
        assert_eq!(text, "❤️ ![Buy milk](t1) ");
        assert_eq!(caret, text.encode_utf16().count());

        let (text, caret) = insert("no trigger", 5, TriggerKind::Mention, "X", "x");
        assert_eq!((text.as_str(), caret), ("no trigger", 5));
    }

    fn user(id: &str, name: &str, agent: bool) -> User {
        serde_json::from_value(
            json!({ "id": id, "name": name, "email": format!("{id}@x.io"), "isAIAgent": agent }),
        )
        .expect("a user")
    }

    fn list(id: &str, name: &str) -> TaskList {
        serde_json::from_value(json!({ "id": id, "name": name })).expect("a list")
    }

    fn task(id: &str, title: &str) -> Task {
        serde_json::from_value(json!({ "id": id, "title": title })).expect("a task")
    }

    /// The people the web offers, in its order, without the reader, filtered by what was typed.
    #[test]
    fn mentions_are_the_task_s_people_and_the_agent_never_the_reader() {
        let mut on_task = task("t1", "Plan");
        on_task.creator = Some(user("carol", "Carol", false));
        on_task.assignee = Some(user("me", "Me", false));
        on_task.list_ids = Some(vec!["l1".into()]);
        let mut work = list("l1", "Work");
        work.owner = Some(user("owen", "Owen", false));
        work.list_members = Some(vec![serde_json::from_value(json!({
            "userId": "dana", "role": "member", "user": { "id": "dana", "name": "Dana", "email": "dana@x.io" }
        }))
        .expect("a member")]);
        work.admins = Some(vec![user("carol", "Carol", false)]);
        let astrid = user("ai-agent-astrid", "Astrid", true);
        let sources = Sources {
            task: &on_task,
            lists: std::slice::from_ref(&work),
            tasks: &[],
            users: &[astrid.clone(), user("me", "Me", false)],
            me: Some("me"),
            selected_list_id: Some("l1"),
        };

        let all = suggestions(TriggerKind::Mention, "", &sources);
        let ids: Vec<&str> = all.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["ai-agent-astrid", "carol", "owen", "dana"]);
        assert!(all[0].is_agent);
        assert_eq!(all[1].secondary.as_deref(), Some("carol@x.io"));

        let typed = suggestions(TriggerKind::Mention, "DA", &sources);
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].label, "Dana");
    }

    #[test]
    fn lists_are_real_ones_ten_at_most() {
        let mut lists: Vec<TaskList> = (0..12)
            .map(|i| list(&format!("l{i}"), &format!("List {i}")))
            .collect();
        let mut today = list("today", "Today");
        today.is_virtual = Some(true);
        lists.push(today);
        let mut column = list("ready", "Ready");
        column.list_type = Some("status".into());
        lists.push(column);
        let on_task = task("t1", "Plan");
        let sources = Sources {
            task: &on_task,
            lists: &lists,
            tasks: &[],
            users: &[],
            me: None,
            selected_list_id: None,
        };

        let all = suggestions(TriggerKind::List, "", &sources);
        assert_eq!(all.len(), 10);
        assert!(all.iter().all(|row| row.id != "today" && row.id != "ready"));

        let one = suggestions(TriggerKind::List, "list 1", &sources);
        assert_eq!(
            one.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
            vec!["List 1", "List 10", "List 11"]
        );
    }

    /// The open list's tasks first, unfinished before finished, newest edit first, fifteen at most.
    #[test]
    fn tasks_come_from_the_open_list_first_unfinished_first_newest_first() {
        let mut tasks = Vec::new();
        let mut make = |id: &str, title: &str, list_id: &str, completed: bool, updated: &str| {
            let mut t = task(id, title);
            t.list_ids = Some(vec![list_id.into()]);
            t.completed = completed;
            t.updated_at = updated.parse().ok();
            tasks.push(t);
        };
        make("a", "Buy apples", "other", false, "2026-09-01T00:00:00Z");
        make("b", "Buy bread", "l1", true, "2026-09-05T00:00:00Z");
        make("c", "Buy cheese", "l1", false, "2026-09-02T00:00:00Z");
        make("d", "Buy dates", "l1", false, "2026-09-04T00:00:00Z");
        make("e", "Sell eggs", "l1", false, "2026-09-06T00:00:00Z");
        let lists = vec![list("l1", "Groceries"), list("other", "Elsewhere")];
        let on_task = task("t1", "Plan");
        let sources = Sources {
            task: &on_task,
            lists: &lists,
            tasks: &tasks,
            users: &[],
            me: None,
            selected_list_id: Some("l1"),
        };

        let rows = suggestions(TriggerKind::Task, "buy", &sources);
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["d", "c", "b", "a"]);
        assert_eq!(rows[0].secondary.as_deref(), Some("Groceries"));
        assert!(rows[2].completed);
    }

    #[test]
    fn the_wire_shape_names_the_kind() {
        let trigger = find_trigger("@a", 2).expect("a trigger");
        let json = serde_json::to_value(trigger).unwrap();
        assert_eq!(json["kind"], "mention");
        assert_eq!(json["start"], 0);
    }
}
