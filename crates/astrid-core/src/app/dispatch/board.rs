//! The board: columns, cards and moving between them.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// How many cards a column carries across the boundary unless the shell asks for more.
///
/// A column is read top-down and the count comes back whole, so a hundred-card Done column crosses
/// as the handful anybody is looking at — the same reason `rowsForList` sends a window.
pub(super) const BOARD_COLUMN_LIMIT: usize = 50;

/// The board a list belongs to.
///
/// Answers with rows so a card draws like a row: the same due labels, the same leading control,
/// the same converters in the shell. The surface is `BoardCard`, which is what makes the leading
/// control open the assignee picker rather than complete the task — tapping a face on a card is
/// how you reassign it, and completing from a board is the Done column.
pub(super) fn board(app: &App, list_id: &str, limit: Option<usize>) -> Response {
    let lists = app.store.lists().unwrap_or_default();
    let Some(opened) = lists.iter().find(|list| list.id == list_id) else {
        return Response::failed(Failure::not_found("list", list_id));
    };
    // A list with no project has no board. Not an error — the shell asks before it knows.
    let Some(project_id) = opened.project_id.clone() else {
        return Response::ok(serde_json::json!({
            "projectId": serde_json::Value::Null,
            "columns": [],
        }));
    };

    let project = app
        .store
        .projects()
        .unwrap_or_default()
        .into_iter()
        .find(|project| project.id == project_id);
    let columns = crate::board::columns(
        project
            .as_ref()
            .and_then(|project| project.custom_states.as_ref()),
    );

    let tasks = app.store.tasks().unwrap_or_default();
    // Borrowed, like the row pipeline: a board of ten thousand cards has no use for a second copy
    // of itself every time somebody moves one.
    let cards: Vec<&crate::model::Task> = crate::board::domain_tasks(&tasks, &lists, &project_id);

    let users: Vec<crate::model::User> = cards
        .iter()
        .filter_map(|task| task.assignee_id.as_deref())
        .filter_map(|id| app.store.user(id).ok().flatten())
        .collect();
    let depths = std::collections::HashMap::new();
    let counts = rows::subtask_counts(&tasks);
    let current_user_id = app.context.account().current_user_id().ok().flatten();
    let context = RowContext {
        current_user_id: current_user_id.as_deref(),
        display_mode: rows::DisplayMode::List,
        surface: rows::Surface::BoardCard,
        now: app.clock.now(),
        offset: app.clock.utc_offset(),
        lists: &lists,
        users: &users,
        // Cards are flat. A card indented under a parent in another column would be indented
        // against nothing.
        depths: &depths,
        subtask_counts: &counts,
    };

    let limit = limit.unwrap_or(BOARD_COLUMN_LIMIT);
    let drawn: Vec<serde_json::Value> = columns
        .iter()
        .map(|column| {
            let held: Vec<&crate::model::Task> = cards
                .iter()
                .copied()
                .filter(|card| crate::board::column_for(card, &columns) == column.id)
                .collect();
            let window = &held[..limit.min(held.len())];
            serde_json::json!({
                "id": column.id,
                "name": column.name,
                "description": column.description,
                "kind": column.kind,
                "total": held.len(),
                "cards": serialize_rows(&TaskRow::build_all(window, &context)),
            })
        })
        .collect();

    Response::ok(serde_json::json!({
        "projectId": project_id,
        "columns": drawn,
    }))
}

/// Move a card to a column.
///
/// Done goes through the completion service rather than writing the flag, because a repeating card
/// dragged to Done must roll forward to its next occurrence like every other completion — rule 2 of
/// `docs/ASTRID.md` §0 does not stop applying because the gesture is a drag. Coming back out of
/// Done un-completes through the same service, for the same reason.
pub(super) fn move_task_to_column(
    app: &App,
    task_id: &str,
    column_id: &str,
    list_id: &str,
) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let lists = app.store.lists().unwrap_or_default();
    let project_id = lists
        .iter()
        .find(|list| list.id == list_id)
        .and_then(|list| list.project_id.clone());
    let columns = board_columns(app, project_id.as_deref());
    move_to_column(app, &task, &lists, &columns, column_id)
}

/// A project's columns, or the ones every board shares when there is no project.
pub(super) fn board_columns(app: &App, project_id: Option<&str>) -> Vec<crate::board::BoardColumn> {
    let project = project_id.and_then(|id| {
        app.store
            .projects()
            .unwrap_or_default()
            .into_iter()
            .find(|project| project.id == id)
    });
    crate::board::columns(
        project
            .as_ref()
            .and_then(|project| project.custom_states.as_ref()),
    )
}

/// The columns a task's own menu can put it in (task 016ce981).
///
/// The detail has no board open, so the project comes from the task's own lists; a task in no
/// project gets the columns every board shares, which is what web's `boardColumnsFor(null)` gives
/// its menu. Resolved here rather than in the shell so the menu and the board read one list.
pub(super) fn task_columns(app: &App, task: &crate::model::Task) -> Vec<crate::board::BoardColumn> {
    let lists = app.store.lists().unwrap_or_default();
    let project_id = task.effective_list_ids().into_iter().find_map(|id| {
        lists
            .iter()
            .find(|list| list.id == id)
            .and_then(|list| list.project_id.clone())
    });
    board_columns(app, project_id.as_deref())
}

/// Which columns the menu offers, and which one is lit. `board::column_for` decides the latter, so
/// the lit row and the card's column on the board are one answer (task 016ce981).
pub(super) fn task_status_options(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let columns = task_columns(app, &task);
    let current = crate::board::column_for(&task, &columns);
    Response::ok(serde_json::json!({
        "current": current,
        "columns": columns.iter().map(|column| serde_json::json!({
            "id": column.id,
            "name": column.name,
            "kind": column.kind,
            "isCurrent": column.id == current,
        })).collect::<Vec<_>>(),
    }))
}

/// The menu's "Set status": the very move a dragged card makes, against the same columns the menu
/// was shown (task 016ce981).
pub(super) fn set_task_status(app: &App, task_id: &str, column_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let lists = app.store.lists().unwrap_or_default();
    let columns = task_columns(app, &task);
    move_to_column(app, &task, &lists, &columns, column_id)
}

/// Put `task` in the column called `column_id`, out of `columns`. The shared tail of a drag and a
/// menu choice; see [`move_task_to_column`] for why Done goes through the completion service.
pub(super) fn move_to_column(
    app: &App,
    task: &crate::model::Task,
    lists: &[crate::model::TaskList],
    columns: &[crate::board::BoardColumn],
    column_id: &str,
) -> Response {
    let task_id = &task.id;
    let Some(target) = columns.iter().find(|column| column.id == column_id) else {
        return Response::failed(Failure::bad_request("that column is not on this board"));
    };

    let moved = crate::board::resolve_move(task, target, lists);

    // The memberships first: a completion that also has to shed a stale status membership should
    // shed it whichever way the write is ordered, and doing it here keeps one path for it.
    if moved.list_ids != task.effective_list_ids() {
        if let Err(error) = app
            .context
            .tasks()
            .set_lists(task_id, moved.list_ids.clone())
        {
            return Response::failed(error.into());
        }
    }

    let changes = crate::services::TaskChanges {
        status_role: Some(moved.status_role.clone()),
        ..Default::default()
    };
    if let Err(error) = app.context.tasks().update(task_id, &changes) {
        return Response::failed(error.into());
    }

    if moved.completed != task.completed {
        return answer(
            app.context
                .tasks()
                .complete(task_id, moved.completed, None, None),
        );
    }
    match app.context.tasks().task(task_id) {
        Ok(Some(task)) => Response::ok(task),
        Ok(None) => Response::failed(Failure::not_found("task", task_id)),
        Err(error) => Response::failed(error.into()),
    }
}
