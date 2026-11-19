//! The multi-list project board (AWTD-1676; web AWTD-1660, Mac AITD-674): each chosen list is a
//! row — a swimlane — and the project's status columns stay columns.
//!
//! Ported from astrid-web `lib/project-board-rows.ts` and the body check of
//! `lib/project-board-view.ts` (`parseBoardRowListIds`), which the Mac had copied into Swift
//! (`ProjectBoardRows.swift`) because this crate did not carry them. Locked by the core-first
//! fixture `contracts/fixtures/board-rows.json`, whose expectations are web's own test cases.
//!
//! **Which lists are rows** is saved per person per project on the server
//! (`GET|PUT /api/v1/projects/{id}/board-rows`). `None` means nothing is saved yet, and the board
//! shows the project's own lists; an empty list is a board the person emptied on purpose.
//!
//! **Which column a card is in** is not decided here: it is [`board::column_for`] against the
//! PROJECT's columns, the same pair the single-list board uses, so a card cannot sit in one column
//! on a list's board and another on its project's board. A row list from another project, or with
//! no board at all, is drawn under this project's columns; a role this project has no column for
//! is Inbox (`docs/CONTRACTS.md` D67).

use jiff::tz::Offset;
use jiff::Timestamp;

use crate::board::BoardColumn;
use crate::board_cards::{self, BoardList};
use crate::model::{Task, TaskList};

#[cfg(test)]
use crate::board;

/// The most rows one board may save — web's `PROJECT_BOARD_MAX_ROWS`; the API refuses more.
pub const MAX_ROWS: usize = 100;

/// The project's own domain lists, in the order the lists arrive (`defaultBoardRowListIds`).
pub fn default_row_list_ids<'a>(project_id: &str, lists: &'a [TaskList]) -> Vec<&'a str> {
    lists
        .iter()
        .filter(|list| list.project_id.as_deref() == Some(project_id) && list.is_domain_list())
        .map(|list| list.id.as_str())
        .collect()
}

/// The lists to draw as rows, in order (`resolveBoardRows`).
///
/// `saved` is what the server holds: `None` for nothing saved, which is the project's own lists.
/// Saved ids this person can no longer see, repeats, and anything that is not a domain list (a
/// status row, a label) are dropped. An emptied board stays empty.
pub fn resolve<'a>(
    saved: Option<&[String]>,
    project_id: &str,
    lists: &'a [TaskList],
) -> Vec<&'a TaskList> {
    let ids: Vec<&str> = match saved {
        Some(saved) => saved.iter().map(String::as_str).collect(),
        None => default_row_list_ids(project_id, lists),
    };
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .filter_map(|id| {
            // Two cached rows with one id are one list; the first is as good as the last.
            let list = lists
                .iter()
                .find(|list| list.id == id && list.is_domain_list())?;
            seen.insert(id).then_some(list)
        })
        .collect()
}

/// Add a row once, at the end (`addBoardRow`).
pub fn add(row_ids: &[String], list_id: &str) -> Vec<String> {
    let mut rows = row_ids.to_vec();
    if !rows.iter().any(|id| id == list_id) {
        rows.push(list_id.to_string());
    }
    rows
}

/// Remove a row — the × on its header (`removeBoardRow`).
pub fn remove(row_ids: &[String], list_id: &str) -> Vec<String> {
    row_ids
        .iter()
        .filter(|id| id.as_str() != list_id)
        .cloned()
        .collect()
}

/// Lists that could be added (`boardRowCandidates`): domain lists that are not virtual and not
/// rows yet, once each, by name.
///
/// By name ignoring case, then as written — web's `localeCompare` and the Mac's
/// `localizedCompare` both fold case, and a core with no collation tables answers the same for
/// every name a list is likely to have.
pub fn candidates<'a>(lists: &'a [TaskList], row_ids: &[String]) -> Vec<&'a TaskList> {
    let mut seen = std::collections::HashSet::new();
    let mut offered: Vec<&TaskList> = lists
        .iter()
        .filter(|list| {
            list.is_domain_list() && list.is_virtual != Some(true) && !row_ids.contains(&list.id)
        })
        .filter(|list| seen.insert(list.id.as_str()))
        .collect();
    offered.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    offered
}

/// Why a set of rows cannot be saved — the two things web's `parseBoardRowListIds` refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowsError {
    /// More than [`MAX_ROWS`].
    TooMany,
    /// An empty id.
    EmptyId,
}

impl RowsError {
    /// The sentence the server answers with (`app/api/v1/projects/[id]/board-rows`).
    pub fn message(&self) -> String {
        format!("listIds must be an array of at most {MAX_ROWS} list ids")
    }
}

/// The ids to save: at most [`MAX_ROWS`] non-empty ids, repeats dropped in order
/// (`parseBoardRowListIds`). Checked before a save leaves the device, so a write the server would
/// refuse is refused with its words and no round trip.
pub fn normalize(list_ids: &[String]) -> Result<Vec<String>, RowsError> {
    if list_ids.len() > MAX_ROWS {
        return Err(RowsError::TooMany);
    }
    if list_ids.iter().any(String::is_empty) {
        return Err(RowsError::EmptyId);
    }
    let mut seen = std::collections::HashSet::new();
    Ok(list_ids
        .iter()
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect())
}

/// One row's cards: the top-level tasks on `list_id`, in iOS's store order — the same cards a
/// list's own board draws ([`board_cards::cards`]), but chosen by membership of the row list
/// rather than of the project. A card on two row lists is a card in both rows.
pub fn row_cards<'a>(tasks: &'a [Task], list_id: &str) -> Vec<&'a Task> {
    let mut cards: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.parent_task_id.is_none())
        .filter(|task| task.effective_list_ids().iter().any(|id| id == list_id))
        .collect();
    cards.sort_by(|a, b| crate::filters::display_order(a, b));
    cards
}

/// One row under the project's columns (`buildSwimlanes`, for one list): for each of `columns`,
/// the row's cards in it, in order.
///
/// `columns` are the PROJECT's. A card's column is its role against them, so a role the project
/// has no column for — a column of the list's own project, or a role left by a removed column — is
/// Inbox, and a list with no board of its own still draws its cards (D67). The row list's manual
/// order arranges each column and its completion filter and window decide Done, as on its own
/// board.
pub fn row_columns<'a>(
    row: Option<&TaskList>,
    row_list_id: &str,
    tasks: &'a [Task],
    columns: &[BoardColumn],
    now: Timestamp,
    offset: Offset,
) -> Vec<Vec<&'a Task>> {
    let cards = row_cards(tasks, row_list_id);
    let arrangement = BoardList::of(row);
    columns
        .iter()
        .map(|column| board_cards::column_cards(&cards, column, columns, arrangement, now, offset))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(value: serde_json::Value) -> TaskList {
        serde_json::from_value(value).expect("a list")
    }

    fn task(value: serde_json::Value) -> Task {
        serde_json::from_value(value).expect("a task")
    }

    fn lists() -> Vec<TaskList> {
        vec![
            list(serde_json::json!({ "id": "web", "name": "Web", "projectId": "p1" })),
            list(serde_json::json!({ "id": "ios", "name": "iOS", "projectId": "p1" })),
            list(
                serde_json::json!({ "id": "status-row", "name": "Ready", "projectId": "p1", "listType": "status" }),
            ),
            list(
                serde_json::json!({ "id": "label", "name": "Bug", "projectId": "p1", "listType": "label" }),
            ),
            list(serde_json::json!({ "id": "groceries", "name": "Groceries" })),
            list(serde_json::json!({ "id": "other-project", "name": "Other", "projectId": "p2" })),
            list(serde_json::json!({ "id": "smart", "name": "Smart", "isVirtual": true })),
        ]
    }

    fn ids(rows: &[&TaskList]) -> Vec<String> {
        rows.iter().map(|list| list.id.clone()).collect()
    }

    fn strings(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn nothing_saved_is_the_projects_own_domain_lists_awtd_1676() {
        let lists = lists();
        assert_eq!(default_row_list_ids("p1", &lists), ["web", "ios"]);
        assert_eq!(ids(&resolve(None, "p1", &lists)), ["web", "ios"]);
    }

    #[test]
    fn saved_rows_draw_in_saved_order_without_unseen_repeats_or_non_lists_awtd_1676() {
        let lists = lists();
        let saved = strings(&["groceries", "gone", "web", "web", "label", "status-row"]);
        assert_eq!(
            ids(&resolve(Some(&saved), "p1", &lists)),
            ["groceries", "web"]
        );
        // An emptied board stays empty: it is not the default.
        assert!(resolve(Some(&[]), "p1", &lists).is_empty());
    }

    #[test]
    fn a_row_is_added_once_at_the_end_and_removed_with_its_cross_awtd_1676() {
        assert_eq!(add(&strings(&["web"]), "ios"), ["web", "ios"]);
        assert_eq!(add(&strings(&["web", "ios"]), "web"), ["web", "ios"]);
        assert_eq!(remove(&strings(&["web", "ios"]), "web"), ["ios"]);
        assert_eq!(remove(&strings(&["ios"]), "missing"), ["ios"]);
    }

    #[test]
    fn candidates_are_unused_real_lists_by_name_awtd_1676() {
        let lists = lists();
        let offered: Vec<&str> = candidates(&lists, &strings(&["web"]))
            .iter()
            .map(|list| list.name.as_str())
            .collect();
        // Case folded: "iOS" sorts among the I's, not after every capital.
        assert_eq!(offered, ["Groceries", "iOS", "Other"]);
    }

    #[test]
    fn a_save_is_at_most_a_hundred_non_empty_ids_without_repeats_awtd_1676() {
        assert_eq!(
            normalize(&strings(&["a", "b", "a"])),
            Ok(strings(&["a", "b"]))
        );
        assert_eq!(normalize(&[]), Ok(vec![]));
        assert_eq!(normalize(&strings(&["a", ""])), Err(RowsError::EmptyId));
        let many: Vec<String> = (0..=MAX_ROWS).map(|n| n.to_string()).collect();
        assert_eq!(normalize(&many), Err(RowsError::TooMany));
        assert_eq!(
            normalize(&many[..MAX_ROWS]).map(|ids| ids.len()),
            Ok(MAX_ROWS)
        );
    }

    /// The Mac drew a foreign list's own board and mapped its columns across; a list with no
    /// board drew nothing. The row is the list's cards under THIS project's columns.
    #[test]
    fn a_row_is_its_lists_cards_under_the_projects_columns_awtd_1676() {
        let columns = board::columns(Some(&serde_json::json!([
            { "role": "review", "name": "Review", "order": 3 }
        ])));
        let now: Timestamp = "2026-11-14T12:00:00Z".parse().unwrap();
        let offset = Offset::UTC;
        let tasks = vec![
            task(serde_json::json!({ "id": "a", "listIds": ["groceries"], "statusRole": "doing" })),
            task(
                serde_json::json!({ "id": "b", "listIds": ["groceries"], "statusRole": "custom-qa" }),
            ),
            task(
                serde_json::json!({ "id": "c", "listIds": ["groceries", "web"], "statusRole": "review" }),
            ),
            task(serde_json::json!({ "id": "d", "listIds": ["groceries"] })),
            task(serde_json::json!({ "id": "e", "listIds": ["web"], "statusRole": "ready" })),
            task(serde_json::json!({ "id": "sub", "listIds": ["groceries"], "parentTaskId": "a" })),
        ];
        let lists = lists();
        let row = lists.iter().find(|list| list.id == "groceries");
        let drawn = row_columns(row, "groceries", &tasks, &columns, now, offset);
        let by_column: Vec<(String, Vec<&str>)> = columns
            .iter()
            .zip(&drawn)
            .map(|(column, held)| {
                (
                    column.id.clone(),
                    held.iter().map(|task| task.id.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            by_column,
            vec![
                // A role this project has no column for waits in the Inbox, beside the card with none.
                ("waiting".to_string(), vec![]),
                (board::VIRTUAL_INBOX_COLUMN_ID.to_string(), vec!["b", "d"]),
                ("ready".to_string(), vec![]),
                ("doing".to_string(), vec!["a"]),
                ("review".to_string(), vec!["c"]),
                (board::VIRTUAL_DONE_COLUMN_ID.to_string(), vec![]),
            ]
        );
    }
}
