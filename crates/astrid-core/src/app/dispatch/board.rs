//! The board: columns, cards and moving between them.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Every command about a board, its statuses or its projects, handed here whole by [`super::run`]
/// (task 8a91c5b0).
///
/// Every arm is short on purpose: it names a service call, and the service decides. An arm that
/// grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file nobody
/// would look in for it.
pub(super) async fn run(app: &App, command: Command) -> Response {
    match command {
        Command::Board {
            list_id,
            project_id,
            limit,
            ids_only,
        } => board(
            app,
            list_id.as_deref(),
            project_id.as_deref(),
            limit,
            ids_only,
        ),
        Command::DropBoardCard {
            task_id,
            column_id,
            list_id,
            index,
        } => drop_board_card(app, &task_id, &column_id, &list_id, index),
        Command::MoveTaskToColumn {
            task_id,
            column_id,
            list_id,
        } => move_task_to_column(app, &task_id, &column_id, &list_id),
        Command::AddBoardCard {
            list_id,
            column_id,
            title,
        } => add_board_card(app, &list_id, &column_id, title),
        Command::TaskStatusOptions { task_id } => task_status_options(app, &task_id),
        Command::SetTaskStatus { task_id, column_id } => set_task_status(app, &task_id, &column_id),
        Command::AddBoardStatus { list_id, name } => {
            status_outcome(app.context.boards().add_status(&list_id, &name).await)
        }
        Command::RenameBoardStatus {
            list_id,
            role,
            name,
        } => status_outcome(
            app.context
                .boards()
                .rename_status(&list_id, &role, &name)
                .await,
        ),
        Command::ReorderBoardStatus {
            list_id,
            role,
            direction,
        } => {
            let direction = match direction.as_str() {
                "up" => crate::board::ReorderDirection::Up,
                "down" => crate::board::ReorderDirection::Down,
                other => {
                    return Response::failed(Failure::bad_request(format!(
                        "direction must be 'up' or 'down', not '{other}'"
                    )))
                }
            };
            status_outcome(
                app.context
                    .boards()
                    .reorder_status(&list_id, &role, direction)
                    .await,
            )
        }
        Command::RemoveBoardStatus { list_id, role } => {
            status_outcome(app.context.boards().remove_status(&list_id, &role).await)
        }
        Command::Projects => answer(app.store.projects().map_err(Into::into)),
        Command::CreateProject {
            name,
            description,
            color,
            image_url,
        } => answer(
            app.context
                .boards()
                .create_project(
                    &name,
                    description.as_deref(),
                    color.as_deref(),
                    image_url.as_deref(),
                )
                .await,
        ),
        Command::CreateBoardForList { list_id } => {
            answer(app.context.boards().create_board_for_list(&list_id).await)
        }
        Command::DeleteProject { project_id } => {
            answer(app.context.boards().delete_project(&project_id).await)
        }
        // `super::run` is one exhaustive match over every variant and the groups it routes are
        // disjoint, so the compiler has already proved nothing else reaches this arm.
        _ => misrouted("board"),
    }
}

/// How many cards a column carries as rows unless the shell asks for more. The count comes back
/// whole, and an `idsOnly` answer carries every id: a window is a transport choice for a shell
/// that draws the core's rows (Windows), never a limit on what the board holds (D43).
pub(super) const BOARD_COLUMN_LIMIT: usize = 50;

/// The tasks a board can draw: the ones in the project's own lists, and no others.
///
/// Read by membership rather than read whole and filtered. `board_cards::cards` keeps exactly the
/// tasks in `board::domain_list_ids`, so this narrows the *fetch* to what the rule was going to
/// keep anyway and changes no answer — but a board in one project no longer costs every task in
/// the account, which it did on every open and on every card moved (task `71ddacb8`).
fn domain_tasks(
    app: &App,
    lists: &[crate::model::TaskList],
    project_id: &str,
) -> Vec<crate::model::Task> {
    let domain: Vec<String> = crate::board::domain_list_ids(lists, project_id)
        .into_iter()
        .map(str::to_string)
        .collect();
    app.store.tasks_in_lists(&domain).unwrap_or_default()
}

/// A board-status write's answer: the state that changed, or the web's own refusal as a bad
/// request (task e5214fba).
///
/// It sat in `lists.rs` until task 8a91c5b0 moved the arms out of the router, which is when it
/// became visible that the only callers are the four board-status arms.
fn status_outcome(outcome: crate::services::Result<crate::services::StatusOutcome>) -> Response {
    match outcome {
        Ok(crate::services::StatusOutcome::Written(state)) => {
            Response::ok(serde_json::json!({ "state": state }))
        }
        Ok(crate::services::StatusOutcome::Refused(refused)) => {
            Response::failed(Failure::bad_request(&refused.message))
        }
        Err(error) => Response::failed(error.into()),
    }
}

/// The board a list belongs to.
///
/// Answers with rows so a card draws like a row: the same due labels, the same leading control,
/// the same converters in the shell. The surface is `BoardCard`, which is what makes the leading
/// control open the assignee picker rather than complete the task — tapping a face on a card is
/// how you reassign it, and completing from a board is the Done column.
///
/// The cards are the ones iOS draws, in iOS's order (AITD-461, D43): top-level tasks only, the
/// opened list's manual order, and Done holding what that list's completion filter and window
/// let through. `ids_only` answers each column's ids instead of rows — every one of them unless
/// `limit` asks for a window, as iOS draws every card; rows come [`BOARD_COLUMN_LIMIT`] at a time.
pub(super) fn board(
    app: &App,
    list_id: Option<&str>,
    project_id: Option<&str>,
    limit: Option<usize>,
    ids_only: bool,
) -> Response {
    let lists = app.store.lists().unwrap_or_default();
    let opened = match list_id {
        Some(id) => match lists.iter().find(|list| list.id == id) {
            Some(list) => Some(list),
            None => return Response::failed(Failure::not_found("list", id)),
        },
        None => None,
    };
    // A list with no project has no board. Not an error — the shell asks before it knows.
    let Some(project_id) = opened
        .and_then(|list| list.project_id.clone())
        .or_else(|| project_id.map(str::to_string))
    else {
        return Response::ok(serde_json::json!({
            "projectId": serde_json::Value::Null,
            "columns": [],
        }));
    };

    let columns = board_columns_with_rows(app, Some(&project_id), &lists);
    let tasks = domain_tasks(app, &lists, &project_id);
    // Borrowed, like the row pipeline: a board of ten thousand cards has no use for a second copy
    // of itself every time somebody moves one.
    let cards = crate::board_cards::cards(&tasks, &lists, &project_id);
    let arrangement = crate::board_cards::BoardList::of(opened);
    let now = app.clock.now();
    let offset = app.clock.utc_offset();
    let held: Vec<Vec<&crate::model::Task>> = columns
        .iter()
        .map(|column| {
            crate::board_cards::column_cards(&cards, column, &columns, arrangement, now, offset)
        })
        .collect();

    if ids_only {
        let drawn: Vec<serde_json::Value> = columns
            .iter()
            .zip(&held)
            .map(|(column, held)| {
                let window = &held[..limit.unwrap_or(usize::MAX).min(held.len())];
                serde_json::json!({
                    "id": column.id,
                    "name": column.name,
                    "description": column.description,
                    "kind": column.kind,
                    "total": held.len(),
                    "ids": window.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(),
                })
            })
            .collect();
        return Response::ok(serde_json::json!({
            "projectId": project_id,
            "listId": opened.map(|list| list.id.as_str()),
            "columns": drawn,
        }));
    }

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
        now,
        offset,
        lists: &lists,
        users: &users,
        // Cards are flat. A card indented under a parent in another column would be indented
        // against nothing.
        depths: &depths,
        subtask_counts: &counts,
    };

    let drawn: Vec<serde_json::Value> = columns
        .iter()
        .zip(&held)
        .map(|(column, held)| {
            let window = &held[..limit.unwrap_or(BOARD_COLUMN_LIMIT).min(held.len())];
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
        "listId": opened.map(|list| list.id.as_str()),
        "columns": drawn,
    }))
}

/// Move a card to a column.
///
/// Done goes through the completion service rather than writing the flag, because a repeating card
/// dragged to Done must roll forward to its next occurrence like every other completion — rule 2 of
/// README.md does not stop applying because the gesture is a drag. Coming back out of
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

/// Add a card at the bottom of a column (task 95c7a68f).
///
/// The card is BORN in the column — its lists, its role and, in Done, its completion all decided
/// here before anything is written. Not created bare and then moved: that is the Mac's AITD-328,
/// where the role was dropped in between and every card typed into a column appeared in the Inbox.
///
/// Done completes through the completion service for the same reason a card dragged there does
/// (see [`move_task_to_column`]): `update(completed: true)` skips the repeat rollover, and a
/// repeating card is repeating whichever gesture finished it — rule 2 of README.md.
pub(super) fn add_board_card(app: &App, list_id: &str, column_id: &str, title: String) -> Response {
    let lists = app.store.lists().unwrap_or_default();
    let Some(opened) = lists.iter().find(|list| list.id == list_id) else {
        return Response::failed(Failure::not_found("list", list_id));
    };
    let columns = board_columns(app, opened.project_id.as_deref());
    let Some(target) = columns.iter().find(|column| column.id == column_id) else {
        return Response::failed(Failure::bad_request("that column is not on this board"));
    };

    // The board's own list is the card's domain list: the column is a state, not somewhere a task
    // can be filed, and `resolve_create` is what keeps a role out of `listIds`.
    let born = crate::board::resolve_create(target, Some(list_id));
    let created = match create_task(
        app,
        NewTask {
            title: title.trim().to_string(),
            description: None,
            list_ids: born.list_ids.unwrap_or_default(),
            priority: None,
            due_date_time: None,
            is_all_day: None,
            assignee_id: None,
            parent_task_id: None,
            status_role: born.status_role,
            // Not the quick-add box: a card typed into a column is a title, and the web's board
            // form does not read `#list` tags out of one either.
            quick_add: false,
            locale: None,
            repeating: None,
            repeating_data: None,
            repeat_from: None,
            is_private: None,
            apply_list_defaults: true,
        },
    ) {
        Ok(task) => task,
        Err(error) => return Response::failed(error.into()),
    };

    if target.kind == crate::board::ColumnKind::Done {
        return answer(app.context.tasks().complete(&created.id, true, None, None));
    }
    Response::ok(created)
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

/// A project's columns as iOS names them — a cached status row may name a default (AITD-461,
/// D44). What the board and the status menu show; a move only needs the ids, which are the same.
pub(super) fn board_columns_with_rows(
    app: &App,
    project_id: Option<&str>,
    lists: &[crate::model::TaskList],
) -> Vec<crate::board::BoardColumn> {
    let project = project_id.and_then(|id| {
        app.store
            .projects()
            .unwrap_or_default()
            .into_iter()
            .find(|project| project.id == id)
    });
    crate::board_cards::columns_with_rows(
        project
            .as_ref()
            .and_then(|project| project.custom_states.as_ref()),
        lists,
    )
}

/// The columns a task's own menu can put it in (task 016ce981).
///
/// The detail has no board open, so the project comes from the task's own lists; a task in no
/// project gets the columns every board shares, which is what web's `boardColumnsFor(null)` gives
/// its menu. Resolved here rather than in the shell so the menu and the board read one list.
pub(super) fn task_columns(app: &App, task: &crate::model::Task) -> Vec<crate::board::BoardColumn> {
    let lists = app.store.lists().unwrap_or_default();
    // The task's own board, never the selected list's — see `rows::detail::project_id_for_task`.
    let project_id = rows::detail::project_id_for_task(task, &lists);
    board_columns_with_rows(app, project_id.as_deref(), &lists)
}

/// Which columns the menu offers, and which one is lit. `board::column_for` decides the latter, so
/// the lit row and the card's column on the board are one answer (task 016ce981).
///
/// Never Done (AITD-461, D46, iOS task 7574067b): the menu sits beside an explicit Complete, and
/// offering Done as a state too gave the same action twice — the chip the one that never said it
/// would finish the task. A finished task lights nothing; `current` still says Done.
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
        "columns": columns.iter().filter(|column| column.kind != crate::board::ColumnKind::Done).map(|column| serde_json::json!({
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

    // Already there: nothing to write (AITD-461, D45). A card nudged and released inside its own
    // column, or the lit chip tapped again, used to send an edit of nothing.
    if crate::board::column_for(task, columns) == target.id {
        return Response::ok(task.clone());
    }

    let moved = crate::board::resolve_move(task, target, lists);

    // Leaving Done un-completes FIRST, then moves (AITD-461, D45) — iOS's order: the task is open
    // before it is given its new column, so nothing reads a finished task in a status column.
    if task.completed && !moved.completed {
        if let Err(error) = app.context.tasks().complete(task_id, false, None, None) {
            return Response::failed(error.into());
        }
    }

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

    if moved.completed && !task.completed {
        return answer(app.context.tasks().complete(task_id, true, None, None));
    }
    match app.context.tasks().task(task_id) {
        Ok(Some(task)) => Response::ok(task),
        Ok(None) => Response::failed(Failure::not_found("task", task_id)),
        Err(error) => Response::failed(error.into()),
    }
}

/// Drop a card at a slot in a column (AITD-461): what iOS's board writes on a drop.
///
/// A completion change first, through the completion service (a repeating card dropped on Done
/// rolls forward); then the card's lists and role; then the opened list's manual order with the
/// card at the slot it was dropped on, and the list's sort set to manual so the order shows.
/// A drop onto the card's own column still rearranges it — that is what a drop there is for.
pub(super) fn drop_board_card(
    app: &App,
    task_id: &str,
    column_id: &str,
    list_id: &str,
    index: usize,
) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let lists = app.store.lists().unwrap_or_default();
    let Some(opened) = lists.iter().find(|list| list.id == list_id) else {
        return Response::failed(Failure::not_found("list", list_id));
    };
    let Some(project_id) = opened.project_id.clone() else {
        return Response::failed(Failure::bad_request("that list has no board"));
    };
    let columns = board_columns(app, Some(&project_id));
    let Some(target) = columns.iter().find(|column| column.id == column_id) else {
        return Response::failed(Failure::bad_request("that column is not on this board"));
    };

    let tasks = domain_tasks(app, &lists, &project_id);
    let cards = crate::board_cards::cards(&tasks, &lists, &project_id);
    let placed = crate::board_cards::reorder(
        &task,
        target,
        index,
        &columns,
        &cards,
        &lists,
        crate::board_cards::BoardList::of(Some(opened)),
        app.clock.now(),
        app.clock.utc_offset(),
    );
    let moved = &placed.column_move;

    if moved.completed != task.completed {
        if let Err(error) = app
            .context
            .tasks()
            .complete(task_id, moved.completed, None, None)
        {
            return Response::failed(error.into());
        }
    }
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
    let task = match app.context.tasks().update(task_id, &changes) {
        Ok(task) => task,
        Err(error) => return Response::failed(error.into()),
    };

    let list_changes = crate::services::ListChanges {
        manual_sort_order: Some(placed.manual_order.clone()),
        sort_by: (opened.sort_by.as_deref() != Some("manual")).then(|| Some("manual".to_string())),
        ..Default::default()
    };
    let list = match app.context.lists().update(list_id, &list_changes) {
        Ok(list) => list,
        Err(error) => return Response::failed(error.into()),
    };
    Response::ok(serde_json::json!({ "task": task, "list": list }))
}
