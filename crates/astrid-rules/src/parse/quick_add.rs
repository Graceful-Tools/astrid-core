//! The one add-task control: what it offers while you type.
//!
//! Ported from `astrid-web/lib/quick-add.ts` (task f699462a), which is already the canonical home
//! for these rules — the component there is a rendering concern and everything that differs
//! between placements is decided in that module. This crate takes the same split: the shell draws
//! a text box and a dropdown, and asks here what should be in them.
//!
//! The `#list` autocomplete came from a separate desktop input that was retired; keeping it in one
//! testable place is what let the merged control keep the behaviour rather than reimplement it.

use crate::model::TaskList;

/// Where the control is drawn. Only the inline placement has room for a labelled button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Inline,
    FixedBottom,
}

/// How many columns the window is showing. Decides which prompt the control uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    OneColumn,
    TwoColumn,
    ThreeColumn,
}

/// The width at which the create button has room for its words. Below it the button is the
/// icon-only square the narrow bar uses.
pub const ADD_TASK_LABEL_MIN_WIDTH: f64 = 420.0;

/// Whether the create button shows its label.
///
/// `container_width` is `None` until the control has been measured. Assuming "wide" before then
/// makes the label appear and vanish on the next frame in a narrow column, so an unmeasured
/// control stays icon-only.
pub fn shows_add_label(placement: Placement, container_width: Option<f64>) -> bool {
    placement == Placement::Inline
        && container_width.is_some_and(|width| width >= ADD_TASK_LABEL_MIN_WIDTH)
}

/// A `#tag` still being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashtagQuery {
    /// The text after the `#`, lower-cased. Empty for a bare `#`.
    pub query: String,
    /// The byte index of the `#` itself, so a selection can replace from there.
    pub start: usize,
}

/// Find the hashtag the caret is inside, if any.
///
/// **Only a hashtag running to the end of the value counts.** A finished `#groceries` earlier in
/// the title must not re-open the dropdown every time a later word is typed.
pub fn find_hashtag(value: &str) -> Option<HashtagQuery> {
    let start = value.rfind('#')?;
    let fragment = &value[start + 1..];
    // Whitespace after the `#` means the tag has been finished and moved on from.
    if fragment.chars().any(char::is_whitespace) {
        return None;
    }
    Some(HashtagQuery {
        query: fragment.to_lowercase(),
        start,
    })
}

/// The lists to offer for a `#` query, best first, at most `limit`.
///
/// A name is matched as typed, and with its spaces turned into `-` or `_`, because that is how
/// people type a two-word list name after a `#`.
pub fn lists_for_hashtag<'a>(
    lists: &'a [TaskList],
    query: &str,
    limit: usize,
) -> Vec<&'a TaskList> {
    let needle = query.to_lowercase();
    lists
        .iter()
        // A virtual list ("Today", "Assigned") is a view, not somewhere a task lands. So is a
        // board column.
        .filter(|list| !list.is_virtual.unwrap_or(false) && list.is_domain_list())
        .filter(|list| {
            let name = list.name.to_lowercase();
            name.contains(&needle)
                || name.replace(' ', "-").contains(&needle)
                || name.replace(' ', "_").contains(&needle)
        })
        .take(limit)
        .collect()
}

/// Replace the fragment being typed with the list's `#dashed-tag`, plus a space.
pub fn apply_hashtag(value: &str, list_name: &str) -> String {
    let Some(found) = find_hashtag(value) else {
        return value.to_string();
    };
    format!("{}#{} ", &value[..found.start], dashed(list_name))
}

fn dashed(name: &str) -> String {
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// Which prompt the control shows.
///
/// A key and its parameters rather than a sentence: the shell resolves it against its resources,
/// so this stays free of any i18n runtime — the same split web makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub key: &'static str,
    /// The list's name, for the prompt that names it.
    pub list_name: Option<String>,
}

/// Placement and layout pick the prompt.
///
/// Three columns name the list, because several lists are on screen at once and the input alone
/// would be ambiguous about where the task is going.
pub fn placeholder(
    placement: Placement,
    layout: Option<Layout>,
    list_name: Option<&str>,
) -> Placeholder {
    if placement == Placement::FixedBottom {
        return Placeholder {
            key: "tasks.addTaskPlaceholder",
            list_name: None,
        };
    }
    match layout {
        Some(Layout::ThreeColumn) => match list_name.filter(|name| *name != "My Tasks") {
            Some(name) => Placeholder {
                key: "tasks.addTaskToList",
                list_name: Some(name.to_string()),
            },
            None => Placeholder {
                key: "tasks.addTaskToCurrentList",
                list_name: None,
            },
        },
        Some(Layout::TwoColumn) => Placeholder {
            key: "tasks.addTaskShort",
            list_name: None,
        },
        _ => Placeholder {
            key: "tasks.addNewTask",
            list_name: None,
        },
    }
}

/// Strip the `#tags` out of a typed title, leaving what the task is actually called.
///
/// The tags chose the lists; leaving them in the title means every task created this way is named
/// after its own filing.
pub fn title_without_tags(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|word| !word.starts_with('#') || word.len() == 1)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the web's quick add does with `#tags` when smart parsing is on (`parseTaskInput` in
/// `lib/task-manager-utils.ts`, task 6ac2639a): each `#word` that names a list — case-insensitively,
/// with the list's spaces typed as `-`, `_` or nothing — files the task there, and every tag is then
/// taken out of the title, whether or not it named anything. A `#` inside a word (`C#`) is not a
/// tag, and neither is a bare `#`. The open list is for the caller to append afterwards, as the web
/// appends its selected list after the tagged ones.
pub fn extract_lists(title: &str, lists: &[TaskList]) -> (String, Vec<String>) {
    let mut ids: Vec<String> = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    for word in title.split_whitespace() {
        let Some(tag) = word.strip_prefix('#').filter(|tag| !tag.is_empty()) else {
            kept.push(word);
            continue;
        };
        let tag = tag.to_lowercase();
        let named = lists
            .iter()
            .filter(|list| !list.is_virtual.unwrap_or(false) && list.is_domain_list())
            .find(|list| {
                let name = list.name.to_lowercase();
                let words: Vec<&str> = name.split_whitespace().collect();
                name == tag
                    || words.join("-") == tag
                    || words.join("_") == tag
                    || words.concat() == tag
            });
        if let Some(list) = named {
            if !ids.contains(&list.id) {
                ids.push(list.id.clone());
            }
        }
    }
    (kept.join(" "), ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, name: &str) -> TaskList {
        TaskList::new(id, name)
    }

    /// The web's rule, to the letter: a tag names a list however its spaces were typed, every tag
    /// leaves the title, and what is not a tag stays (task 6ac2639a).
    #[test]
    fn hashtags_file_the_task_and_leave_the_title_task_6ac2639a() {
        let lists = [list("h", "Health"), list("s", "Side Projects")];
        assert_eq!(
            extract_lists("Pushups #health", &lists),
            ("Pushups".to_string(), vec!["h".to_string()])
        );
        assert_eq!(
            extract_lists("Read #side-projects docs #Health", &lists),
            (
                "Read docs".to_string(),
                vec!["s".to_string(), "h".to_string()]
            )
        );
        assert_eq!(
            extract_lists("#side_projects #sideprojects", &lists).1,
            vec!["s".to_string()]
        );
        // A tag that names nothing is still taken out; a `#` in a word, or alone, is not a tag.
        assert_eq!(
            extract_lists("Learn C# #nothing # now", &lists),
            ("Learn C# # now".to_string(), vec![])
        );
        let mut board = list("b", "Doing");
        board.list_type = Some("status".into());
        assert_eq!(
            extract_lists("Fix #doing", &[board]).1,
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_label_waits_until_the_control_has_been_measured() {
        assert!(!shows_add_label(Placement::Inline, None));
        assert!(!shows_add_label(Placement::Inline, Some(419.0)));
        assert!(shows_add_label(Placement::Inline, Some(420.0)));
        assert!(!shows_add_label(Placement::FixedBottom, Some(1200.0)));
    }

    /// A finished tag earlier in the title must not re-open the dropdown on every later keystroke.
    #[test]
    fn only_a_tag_still_being_typed_opens_the_dropdown() {
        assert_eq!(
            find_hashtag("Buy milk #gro"),
            Some(HashtagQuery {
                query: "gro".into(),
                start: 9
            })
        );
        assert_eq!(
            find_hashtag("#groceries"),
            Some(HashtagQuery {
                query: "groceries".into(),
                start: 0
            })
        );
        assert!(find_hashtag("Buy milk #groceries tomorrow").is_none());
        assert!(find_hashtag("Buy milk").is_none());
    }

    /// A bare `#` offers everything, which is how you browse rather than search.
    #[test]
    fn a_bare_hash_is_an_empty_query_rather_than_no_query() {
        let found = find_hashtag("Buy milk #").expect("a query");
        assert_eq!(found.query, "");
    }

    #[test]
    fn a_two_word_list_matches_however_it_is_typed() {
        let lists = vec![list("l1", "Weekend Plans"), list("l2", "Work")];
        for query in ["weekend", "weekend-p", "weekend_p", "plans"] {
            let matched = lists_for_hashtag(&lists, query, 5);
            assert_eq!(matched.len(), 1, "for {query}");
            assert_eq!(matched[0].id, "l1");
        }
    }

    /// A view is not somewhere a task lands, and neither is a board column.
    #[test]
    fn virtual_lists_and_board_columns_are_not_offered() {
        let lists = vec![
            list("l1", "Today does not exist"),
            serde_json::from_value(serde_json::json!({
                "id": "v1", "name": "Today", "isVirtual": true
            }))
            .expect("decodes"),
            serde_json::from_value(serde_json::json!({
                "id": "s1", "name": "Today doing", "listType": "status"
            }))
            .expect("decodes"),
        ];
        let matched = lists_for_hashtag(&lists, "today", 5);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "l1");
    }

    #[test]
    fn the_dropdown_is_capped() {
        let lists: Vec<TaskList> = (0..10)
            .map(|index| list(&format!("l{index}"), &format!("List {index}")))
            .collect();
        assert_eq!(lists_for_hashtag(&lists, "list", 5).len(), 5);
    }

    #[test]
    fn choosing_a_list_replaces_the_fragment_with_its_tag() {
        assert_eq!(
            apply_hashtag("Buy milk #week", "Weekend Plans"),
            "Buy milk #weekend-plans "
        );
        assert_eq!(apply_hashtag("Buy milk", "Weekend Plans"), "Buy milk");
    }

    #[test]
    fn three_columns_name_the_list_and_the_others_do_not() {
        assert_eq!(
            placeholder(Placement::Inline, Some(Layout::ThreeColumn), Some("Home")),
            Placeholder {
                key: "tasks.addTaskToList",
                list_name: Some("Home".into())
            }
        );
        // "My Tasks" is not a list somebody filed anything into, so naming it says nothing.
        assert_eq!(
            placeholder(
                Placement::Inline,
                Some(Layout::ThreeColumn),
                Some("My Tasks")
            )
            .key,
            "tasks.addTaskToCurrentList"
        );
        assert_eq!(
            placeholder(Placement::Inline, Some(Layout::TwoColumn), Some("Home")).key,
            "tasks.addTaskShort"
        );
        assert_eq!(
            placeholder(Placement::Inline, Some(Layout::OneColumn), Some("Home")).key,
            "tasks.addNewTask"
        );
        assert_eq!(
            placeholder(
                Placement::FixedBottom,
                Some(Layout::ThreeColumn),
                Some("Home")
            )
            .key,
            "tasks.addTaskPlaceholder"
        );
    }

    /// The tags chose the lists. Leaving them in means every task created this way is named after
    /// its own filing.
    #[test]
    fn the_tags_come_out_of_the_title() {
        assert_eq!(
            title_without_tags("Buy milk #groceries #urgent"),
            "Buy milk"
        );
        assert_eq!(title_without_tags("Buy milk"), "Buy milk");
        // A lone `#` is punctuation somebody typed, not a tag.
        assert_eq!(
            title_without_tags("Read issue # today"),
            "Read issue # today"
        );
    }
}
