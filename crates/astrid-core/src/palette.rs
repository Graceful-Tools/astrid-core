//! Everything one box can find: commands, lists, tasks.
//!
//! Ports `astrid-ios/Astrid Mac/Commands/FuzzyMatch.swift` and what
//! `CommandPaletteView` searches with it.
//!
//! ## Fuzzy, and the same fuzzy
//!
//! The matcher is a subsequence match with three bonuses — a run of consecutive characters, a
//! character at the start of a word, a character at the start of the text. It is ported exactly
//! rather than improved, because the *ranking* is what a palette is: typing `tdy` and getting
//! "Today" first is the difference between a palette and a list of everything. Somebody moving
//! between the Mac and this app types the same three letters and expects the same first row.
//!
//! ## Three kinds of answer, in one order
//!
//! Commands first, then lists, then tasks. A palette that ranked a task above the command that
//! creates one would make the keyboard shortcut for "new task" unreachable by the very box that
//! exists to reach it.

use serde::Serialize;

use crate::keyboard;
use crate::model::{Task, TaskList};

/// How well `query` matches `text`, or `None` when it does not match at all.
///
/// Every character of the query must appear in the text, in order. The score is higher for a
/// closer match; the numbers are the Mac's and are not worth re-tuning here, because a different
/// ranking is a different product on two platforms.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    if query.len() > text.len() {
        return None;
    }

    let mut index = 0;
    let mut score = 0;
    let mut streak = 0;
    // Deliberately below any real index, so the first match cannot be counted as consecutive.
    let mut previous: i64 = -2;

    for (position, character) in text.iter().enumerate() {
        if index >= query.len() || *character != query[index] {
            continue;
        }
        score += 1;
        if position as i64 == previous + 1 {
            streak += 1;
            score += streak * 2;
        } else {
            streak = 0;
        }
        if position == 0 {
            score += 8;
        } else if !text[position - 1].is_alphanumeric() {
            score += 4;
        }
        previous = position as i64;
        index += 1;
    }

    (index == query.len()).then_some(score)
}

/// What kind of thing a row is, so the shell knows what pressing it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowKind {
    /// A keyboard action, from the shared scheme.
    Command,
    List,
    Task,
}

/// One row of the palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteRow {
    pub kind: RowKind,
    /// The list id, the task id, or — for a command — the action name from the shared keyboard
    /// table, which is exactly what the shell's shortcut dispatcher already carries out.
    pub id: String,
    /// What to show. For a command it is the title the shared keyboard table carries, which is
    /// the same wording web's shortcut sheet uses.
    pub title: String,
    /// The keys that would do this, for a command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
    /// Which list a task is in, so two tasks with one name are told apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
}

/// How many of each kind to answer with.
///
/// A palette is read from the top; a hundred rows is a list, and a list is what the search box is
/// for.
const PER_KIND: usize = 8;

/// Search everything.
///
/// An empty query answers with the first few of each kind rather than with nothing: an empty
/// palette teaches nobody what it can do.
pub fn search(query: &str, lists: &[TaskList], tasks: &[Task]) -> Vec<PaletteRow> {
    let mut rows = Vec::new();

    let mut commands: Vec<(i32, PaletteRow)> = keyboard::ALL
        .iter()
        .filter_map(|shortcut| {
            // Matched on the title the shared table carries — the same words web shows in its
            // shortcut sheet, so the palette and that sheet cannot describe one action two ways.
            score(query, shortcut.title).map(|score| {
                (
                    score,
                    PaletteRow {
                        kind: RowKind::Command,
                        id: keyboard::action_name(shortcut.action).to_string(),
                        title: shortcut.title.to_string(),
                        keys: shortcut.keys.first().map(|key| key.to_string()),
                        subtitle: None,
                    },
                )
            })
        })
        .collect();
    // Best first. Reversed by negating rather than by comparing backwards, which reads the
    // same and keeps a stable sort — two equal scores stay in the order they were found.
    commands.sort_by_key(|(score, _)| -score);
    rows.extend(commands.into_iter().take(PER_KIND).map(|(_, row)| row));

    let mut matched_lists: Vec<(i32, PaletteRow)> = lists
        .iter()
        .filter(|list| list.is_domain_list())
        .filter_map(|list| {
            score(query, &list.name).map(|score| {
                (
                    score,
                    PaletteRow {
                        kind: RowKind::List,
                        id: list.id.clone(),
                        title: list.name.clone(),
                        keys: None,
                        subtitle: None,
                    },
                )
            })
        })
        .collect();
    // Best first. Reversed by negating rather than by comparing backwards, which reads the
    // same and keeps a stable sort — two equal scores stay in the order they were found.
    matched_lists.sort_by_key(|(score, _)| -score);
    rows.extend(matched_lists.into_iter().take(PER_KIND).map(|(_, row)| row));

    let mut matched_tasks: Vec<(i32, PaletteRow)> = tasks
        .iter()
        // Finished tasks are not what somebody is looking for in a palette; search is where those
        // live.
        .filter(|task| !task.completed)
        .filter_map(|task| {
            score(query, &task.title).map(|score| {
                let list = task
                    .effective_list_ids()
                    .first()
                    .and_then(|id| lists.iter().find(|list| &list.id == id))
                    .map(|list| list.name.clone());
                (
                    score,
                    PaletteRow {
                        kind: RowKind::Task,
                        id: task.id.clone(),
                        title: task.title.clone(),
                        keys: None,
                        subtitle: list,
                    },
                )
            })
        })
        .collect();
    // Best first. Reversed by negating rather than by comparing backwards, which reads the
    // same and keeps a stable sort — two equal scores stay in the order they were found.
    matched_tasks.sort_by_key(|(score, _)| -score);
    rows.extend(matched_tasks.into_iter().take(PER_KIND).map(|(_, row)| row));

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, name: &str) -> TaskList {
        TaskList::new(id, name)
    }

    fn task(id: &str, title: &str) -> Task {
        Task::new(id, title)
    }

    /// Every character, in order, or nothing.
    #[test]
    fn a_query_must_appear_in_order() {
        assert!(score("tdy", "Today").is_some());
        assert!(score("ydt", "Today").is_none());
        assert!(score("todayx", "Today").is_none());
    }

    /// An empty query matches everything, which is what fills an unopened palette.
    #[test]
    fn an_empty_query_matches() {
        assert_eq!(score("", "anything"), Some(0));
    }

    /// The ranking is the product. A prefix beats a match in the middle, and consecutive
    /// characters beat scattered ones.
    #[test]
    fn a_closer_match_scores_higher() {
        let prefix = score("tod", "Today").expect("matches");
        let middle = score("tod", "Not odd").expect("matches");
        assert!(
            prefix > middle,
            "prefix {prefix} should beat middle {middle}"
        );

        // Within one word, consecutive beats scattered.
        let together = score("abc", "abcdef").expect("matches");
        let apart = score("abc", "axbxcx").expect("matches");
        assert!(
            together > apart,
            "together {together} should beat apart {apart}"
        );

        // Across words it is the other way round, and that is deliberate rather than a bug in the
        // port: "bm" finding "buy milk" by its initials is what makes a palette usable, so a
        // word-start bonus outweighs a run. The Mac scores it this way and so does this.
        let initials = score("abc", "a b c").expect("matches");
        assert!(
            initials > apart,
            "initials {initials} should beat scattered {apart}"
        );
    }

    /// A character starting a word counts for more than one inside one.
    #[test]
    fn a_word_start_counts_for_more() {
        let word_start = score("bm", "buy milk").expect("matches");
        let inside = score("bm", "bumble").expect("matches");
        assert!(word_start > inside);
    }

    /// Case is not part of the question.
    #[test]
    fn case_does_not_matter() {
        assert!(score("BUY", "buy milk").is_some());
        assert!(score("buy", "BUY MILK").is_some());
    }

    /// Commands first: a palette that ranked a task above "new task" would make the shortcut
    /// unreachable by the box that exists to reach it.
    #[test]
    fn commands_come_before_lists_and_tasks() {
        let rows = search(
            "task",
            &[list("l1", "Task list")],
            &[task("t1", "A task about tasks")],
        );
        let kinds: Vec<RowKind> = rows.iter().map(|row| row.kind).collect();
        let first_list = kinds.iter().position(|kind| *kind == RowKind::List);
        let first_task = kinds.iter().position(|kind| *kind == RowKind::Task);
        assert_eq!(kinds[0], RowKind::Command);
        assert!(first_list < first_task);
    }

    /// A task is shown with the list it is in, so two called "Call back" are told apart.
    #[test]
    fn a_task_says_which_list_it_is_in() {
        let mut in_home = task("t1", "Call back");
        in_home.list_ids = Some(vec!["l1".into()]);
        let rows = search("call", &[list("l1", "Home")], &[in_home]);
        let task_row = rows
            .iter()
            .find(|row| row.kind == RowKind::Task)
            .expect("a task row");
        assert_eq!(task_row.subtitle.as_deref(), Some("Home"));
    }

    /// A finished task is not what somebody is reaching for; search is where those live.
    #[test]
    fn finished_tasks_are_not_offered() {
        let mut done = task("t1", "Buy milk");
        done.completed = true;
        let rows = search("milk", &[], &[done]);
        assert!(!rows.iter().any(|row| row.kind == RowKind::Task));
    }

    /// A status list is not somewhere to go.
    #[test]
    fn status_lists_are_not_offered() {
        let status: TaskList = serde_json::from_value(serde_json::json!({
            "id": "s1", "name": "Doing", "listType": "status"
        }))
        .expect("decodes");
        let rows = search("doing", &[status], &[]);
        assert!(!rows.iter().any(|row| row.kind == RowKind::List));
    }

    /// An empty query fills the palette rather than emptying it: a blank box teaches nobody what
    /// it can do.
    #[test]
    fn an_empty_query_still_offers_something() {
        let rows = search("", &[list("l1", "Home")], &[task("t1", "Buy milk")]);
        assert!(rows.iter().any(|row| row.kind == RowKind::Command));
        assert!(rows.iter().any(|row| row.kind == RowKind::List));
        assert!(rows.iter().any(|row| row.kind == RowKind::Task));
    }
}
