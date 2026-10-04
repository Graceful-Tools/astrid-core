//! A board as iOS draws it: which cards sit in a column and in what order, what a column is
//! called, and where a dropped card lands (AITD-461).
//!
//! [`crate::board`] holds the rules the web's fixtures lock — the columns a project has, which
//! one a card is in, what a move writes. iOS draws more than that, and the Apple apps carried it
//! in Swift (`ProjectStatus.swift`: `getProjectBoardColumns`, `boardColumnTasksSorted`,
//! `resolveBoardReorder`). On disagreement the core follows iOS (Jon, 2026-10-03), so this is
//! that Swift, ported, and `docs/CONTRACTS.md` D43–D45 record where it differs from what this
//! crate answered before:
//!
//! - a column's cards are in the board list's **manual order**, cards the order does not name
//!   last, in iOS's store order ([`crate::filters::display_order`]);
//! - **Done shows recent work only**: the board list's completion filter and recently-completed
//!   window, read the way iOS's board reads them ([`done_shows`]);
//! - **subtasks are not cards** — they draw inside their parent;
//! - a default column takes its **name and description from a cached status row** when no
//!   rename overrides it ([`columns_with_rows`]);
//! - a **drop** writes the move and the card's new place in the manual order ([`reorder`]).

use chrono::{DateTime, FixedOffset, Utc};

use crate::board::{self, BoardColumn, ColumnKind, ColumnMove};
use crate::model::{RecentlyCompletedWindow, Task, TaskList};

/// The board's columns as iOS names them: a rename stored on the board first, then a status row
/// this client still has cached for that role, then the default.
///
/// The rows are gone server-side, but a renamed default was once a PUT on its row, so a cached
/// row still answers for the clients that have one. Only the name and description: the column id
/// is the role whatever is cached, so the board's shape never depends on the cache.
pub fn columns_with_rows(
    custom_states: Option<&serde_json::Value>,
    lists: &[TaskList],
) -> Vec<BoardColumn> {
    let renamed: Vec<String> = board::parse_custom_states(custom_states)
        .into_iter()
        .filter(|state| board::is_default_role(&state.role))
        .map(|state| state.role)
        .collect();
    let rows = status_rows(lists);
    board::columns(custom_states)
        .into_iter()
        .map(|mut column| {
            if column.kind != ColumnKind::Status || !board::is_default_role(&column.id) {
                return column;
            }
            let Some(row) = rows
                .iter()
                .rev()
                .find(|row| row.status_role.as_deref() == Some(column.id.as_str()))
            else {
                return column;
            };
            if !renamed.contains(&column.id) {
                column.name = row.name.clone();
            }
            if let Some(description) = row.status_description.clone().or(row.description.clone()) {
                column.description = description;
            }
            column
        })
        .collect()
}

/// The status rows still cached, in iOS's order (`getProjectStatusLists`): by `statusOrder`, then
/// by name; legacy Inbox and Done rows left out, since the board draws virtual columns for those.
/// When two rows claim a role the last one wins, as iOS's dictionary did — hence the caller's
/// `rev().find`.
fn status_rows(lists: &[TaskList]) -> Vec<&TaskList> {
    let mut rows: Vec<&TaskList> = lists
        .iter()
        .filter(|list| list.is_status_list())
        .filter(|list| {
            let role = list.status_role.as_deref();
            role != Some("done") && role != Some("inbox") && list.status_completed != Some(true)
        })
        .collect();
    rows.sort_by(|a, b| {
        a.status_order
            .unwrap_or(i64::MAX)
            .cmp(&b.status_order.unwrap_or(i64::MAX))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

/// Whether a finished card shows in Done, as iOS's board decides it.
///
/// Not the list view's rule ([`crate::filters::recently_completed::should_show_completed`]): the
/// board reads the board list's filter with "no filter" meaning the window, `show` and `all`
/// meaning everything, `hide` meaning nothing, and anything else everything. And it times a card
/// by when it was last touched, never by `completedAt` — iOS hands its window no completion time.
pub fn done_shows(
    filter: Option<&str>,
    updated_at: Option<DateTime<Utc>>,
    window: Option<&RecentlyCompletedWindow>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> bool {
    match filter.unwrap_or("default") {
        "hide" => false,
        "default" => crate::filters::recently_completed::is_recently_completed(
            None, updated_at, window, now, offset,
        ),
        _ => true,
    }
}

/// What the board needs to know about the list it was opened from: the manual order and the Done
/// window. `None` for a board drawn with no list (a project whose list this cache lacks).
#[derive(Debug, Clone, Copy)]
pub struct BoardList<'a> {
    pub manual_order: Option<&'a [String]>,
    pub filter: Option<&'a str>,
    pub window: Option<&'a RecentlyCompletedWindow>,
}

impl<'a> BoardList<'a> {
    pub fn of(list: Option<&'a TaskList>) -> Self {
        BoardList {
            manual_order: list.and_then(|list| list.manual_sort_order.as_deref()),
            filter: list.and_then(|list| list.filter_completion.as_deref()),
            window: list.and_then(|list| list.recently_completed_window.as_ref()),
        }
    }
}

/// The cards a project's board draws: its domain tasks, top-level only, in iOS's store order.
pub fn cards<'a>(tasks: &'a [Task], lists: &[TaskList], project_id: &str) -> Vec<&'a Task> {
    let mut cards: Vec<&Task> = board::domain_tasks(tasks, lists, project_id)
        .into_iter()
        .filter(|task| task.parent_task_id.is_none())
        .collect();
    cards.sort_by(|a, b| crate::filters::display_order(a, b));
    cards
}

/// One column's cards, in the order the board shows them (`boardColumnTasksSorted`).
///
/// `cards` is [`cards`]' answer. Done keeps what [`done_shows`] lets through. The manual order
/// then arranges the column; a card it does not name goes after every card it does, keeping its
/// place among the others.
pub fn column_cards<'a>(
    cards: &[&'a Task],
    column: &BoardColumn,
    columns: &[BoardColumn],
    list: BoardList<'_>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<&'a Task> {
    let mut held: Vec<&Task> = cards
        .iter()
        .copied()
        .filter(|card| board::column_for(card, columns) == column.id)
        .filter(|card| {
            column.kind != ColumnKind::Done
                || done_shows(list.filter, card.updated_at, list.window, now, offset)
        })
        .collect();
    if let Some(order) = list.manual_order.filter(|order| !order.is_empty()) {
        // The first mention wins, as a dictionary built in order would keep it.
        let mut place: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::with_capacity(order.len());
        for (at, id) in order.iter().enumerate() {
            place.entry(id.as_str()).or_insert(at);
        }
        // Stable, so the cards the order does not name keep their store order.
        held.sort_by_key(|task| place.get(task.id.as_str()).copied().unwrap_or(usize::MAX));
    }
    held
}

/// What a drop writes (`resolveBoardReorder`): the move itself, and the board list's new manual
/// order with the card at the slot it was dropped on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reorder {
    pub column_move: ColumnMove,
    pub manual_order: Vec<String>,
}

/// Place `task` at `index` among `target`'s cards — the slot the board showed, 0 the top and the
/// column's length the bottom.
///
/// The whole list's order changes as little as it can: the card is taken out, then put back just
/// above the card it was dropped on, or just below the column's last card when dropped at the
/// end. A card the order has never named is appended. `cards` is [`cards`]' answer.
#[allow(clippy::too_many_arguments)]
pub fn reorder(
    task: &Task,
    target: &BoardColumn,
    index: usize,
    columns: &[BoardColumn],
    cards: &[&Task],
    lists: &[TaskList],
    list: BoardList<'_>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Reorder {
    let column_move = board::resolve_move(task, target, lists);
    let others: Vec<&Task> = cards
        .iter()
        .copied()
        .filter(|card| card.id != task.id)
        .collect();
    let in_target = column_cards(&others, target, columns, list, now, offset);

    let mut order: Vec<String> = list
        .manual_order
        .unwrap_or_default()
        .iter()
        .filter(|id| **id != task.id)
        .cloned()
        .collect();
    let slot = index.min(in_target.len());
    let at = if slot < in_target.len() {
        order.iter().position(|id| *id == in_target[slot].id)
    } else {
        in_target
            .last()
            .and_then(|last| order.iter().position(|id| *id == last.id))
            .map(|at| at + 1)
    };
    match at {
        Some(at) => order.insert(at, task.id.clone()),
        None => order.push(task.id.clone()),
    }
    Reorder {
        column_move,
        manual_order: order,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("an offset")
    }

    fn list(value: serde_json::Value) -> TaskList {
        serde_json::from_value(value).expect("a list")
    }

    fn domain() -> TaskList {
        list(json!({ "id": "p1-list", "name": "List", "projectId": "p1", "listType": "regular" }))
    }

    fn task(value: serde_json::Value) -> Task {
        let mut base = json!({ "listIds": ["p1-list"] });
        for (key, field) in value.as_object().expect("an object") {
            base[key] = field.clone();
        }
        serde_json::from_value(base).expect("a task")
    }

    fn ids(tasks: &[&Task]) -> Vec<String> {
        tasks.iter().map(|task| task.id.clone()).collect()
    }

    fn column(columns: &[BoardColumn], id: &str) -> BoardColumn {
        columns
            .iter()
            .find(|column| column.id == id)
            .cloned()
            .expect("a column")
    }

    /// AITD-461 (iOS `BoardDoneWindowTests`): Done keeps a card finished inside the window and
    /// drops one finished before it.
    #[test]
    fn aitd461_done_honours_the_window() {
        let lists = vec![domain()];
        let tasks = vec![
            task(
                json!({ "id": "recent", "title": "recent", "completed": true,
                         "updatedAt": "2023-11-14T21:13:20Z" }),
            ),
            task(json!({ "id": "old", "title": "old", "completed": true,
                         "updatedAt": "2023-11-12T22:13:20Z" })),
        ];
        let window: RecentlyCompletedWindow =
            serde_json::from_value(json!({ "kind": "duration", "amount": 1, "unit": "day" }))
                .expect("a window");
        let columns = columns_with_rows(None, &lists);
        let done = column(&columns, board::VIRTUAL_DONE_COLUMN_ID);
        let all = cards(&tasks, &lists, "p1");
        let windowed = |filter: Option<&str>| {
            ids(&column_cards(
                &all,
                &done,
                &columns,
                BoardList {
                    manual_order: None,
                    filter,
                    window: Some(&window),
                },
                now(),
                utc(),
            ))
        };
        assert_eq!(windowed(Some("default")), ["recent"]);
        assert_eq!(windowed(None), ["recent"], "no filter is the window");
        let mut shown = windowed(Some("show"));
        shown.sort();
        assert_eq!(shown, ["old", "recent"]);
        assert!(windowed(Some("hide")).is_empty());
    }

    /// AITD-461: the manual order arranges a column; a card it does not name comes last.
    #[test]
    fn aitd461_a_column_is_in_the_manual_order() {
        let lists = vec![domain()];
        let tasks = vec![
            task(json!({ "id": "a", "title": "a" })),
            task(json!({ "id": "b", "title": "b" })),
            task(json!({ "id": "c", "title": "c" })),
        ];
        let columns = columns_with_rows(None, &lists);
        let inbox = column(&columns, board::VIRTUAL_INBOX_COLUMN_ID);
        let order = vec!["c".to_string(), "a".to_string()];
        let drawn = column_cards(
            &cards(&tasks, &lists, "p1"),
            &inbox,
            &columns,
            BoardList {
                manual_order: Some(&order),
                filter: None,
                window: None,
            },
            now(),
            utc(),
        );
        assert_eq!(ids(&drawn), ["c", "a", "b"]);
    }

    /// AITD-461: subtasks draw inside their parent, never as cards.
    #[test]
    fn aitd461_a_subtask_is_not_a_card() {
        let lists = vec![domain()];
        let tasks = vec![
            task(json!({ "id": "parent", "title": "parent" })),
            task(json!({ "id": "child", "title": "child", "parentTaskId": "parent" })),
        ];
        assert_eq!(ids(&cards(&tasks, &lists, "p1")), ["parent"]);
    }

    /// AITD-461 (iOS `testARenamedDefaultKeepsItsListName`, `testARenameOverrideBeatsAStaleRowsName`):
    /// a cached status row names its default column, below a rename stored on the board.
    #[test]
    fn aitd461_a_cached_status_row_names_its_default_column() {
        let lists = vec![
            domain(),
            list(
                json!({ "id": "row-doing", "name": "In flight", "listType": "status",
                         "statusRole": "doing", "statusDescription": "Moving" }),
            ),
            list(
                json!({ "id": "row-ready", "name": "Queued", "listType": "status",
                         "statusRole": "ready" }),
            ),
        ];
        let renamed = json!([{ "role": "ready", "name": "Up next", "order": 0 }]);
        let columns = columns_with_rows(Some(&renamed), &lists);
        assert_eq!(column(&columns, "doing").name, "In flight");
        assert_eq!(column(&columns, "doing").description, "Moving");
        assert_eq!(column(&columns, "ready").name, "Up next", "the rename wins");
        assert_eq!(column(&columns, "waiting").name, "Waiting");
    }

    /// AITD-461 (iOS `test_resolveBoardReorder_*`): a drop lands at the slot it was dropped on.
    #[test]
    fn aitd461_a_drop_lands_where_it_was_dropped() {
        let lists = vec![domain()];
        let tasks = vec![
            task(json!({ "id": "a", "title": "a", "statusRole": "doing" })),
            task(json!({ "id": "b", "title": "b", "statusRole": "doing" })),
            task(json!({ "id": "c", "title": "c", "statusRole": "doing" })),
            task(json!({ "id": "x", "title": "x" })),
        ];
        let columns = columns_with_rows(None, &lists);
        let doing = column(&columns, "doing");
        let all = cards(&tasks, &lists, "p1");
        let order: Vec<String> = ["a", "b", "c", "x"]
            .iter()
            .map(|id| id.to_string())
            .collect();
        let board_list = BoardList {
            manual_order: Some(&order),
            filter: None,
            window: None,
        };
        let drop = |id: &str, index: usize| {
            let dragged = tasks.iter().find(|task| task.id == id).expect("a task");
            reorder(
                dragged,
                &doing,
                index,
                &columns,
                &all,
                &lists,
                board_list,
                now(),
                utc(),
            )
        };

        // Within the column, into the middle.
        assert_eq!(drop("c", 1).manual_order, ["a", "c", "b", "x"]);
        // From another column, to the top.
        let moved = drop("x", 0);
        assert_eq!(moved.manual_order, ["x", "a", "b", "c"]);
        assert_eq!(moved.column_move.status_role.as_deref(), Some("doing"));
        // To the end: just after the column's last card.
        assert_eq!(drop("x", 3).manual_order, ["a", "b", "c", "x"]);
        assert_eq!(drop("a", 9).manual_order, ["b", "c", "a", "x"]);
    }
}
