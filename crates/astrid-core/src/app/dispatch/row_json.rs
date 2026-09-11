//! The rows a list draws, and how a row is written to JSON.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Build the rows for a list: filter, sort, splice, project.
///
/// All four in one place because they are four separate contracts and a shell that ran them in its
/// own order would be four chances to show a different list from web.
pub(super) fn rows_for_list(
    app: &App,
    list_id: &str,
    display_mode: Option<String>,
    surface: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Response {
    let now = app.clock.now();
    let offset_from_utc = app.clock.utc_offset();

    // My Tasks is not in the list collection — it is the view the app opens on, and its filters
    // belong to the account rather than to a list row. Everything after this is the same pipeline.
    let my_tasks = list_id == crate::filters::my_tasks::VIRTUAL_ID;
    let preferences = match my_tasks {
        true => match app.context.account().my_tasks_preferences() {
            Ok(preferences) => Some(preferences),
            Err(error) => return Response::failed(error.into()),
        },
        false => None,
    };
    let list = match &preferences {
        // A shape rather than a row: the sort setting is read off it below, the same as any list's.
        Some(preferences) => {
            let mut shape = crate::model::TaskList::new(crate::filters::my_tasks::VIRTUAL_ID, "");
            shape.is_virtual = Some(true);
            shape.sort_by = Some(preferences.sort_by.clone());
            shape.manual_sort_order = Some(preferences.manual_sort_order.clone());
            shape
        }
        None => match app.store.list(list_id) {
            Ok(Some(list)) => list,
            Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
            Err(error) => return Response::failed(error.into()),
        },
    };

    // A VIRTUAL list has no membership: it is a saved set of filters over everything the account
    // has — "Today", "Not in a List", "I've Assigned". Sourcing it from membership, the way a real
    // list is sourced, gives an empty screen with nothing to explain it, which is what makes this
    // worth a branch rather than a clever query.
    let source = if list.is_virtual.unwrap_or(false) {
        app.store.tasks()
    } else {
        app.store.tasks_in_list(list_id)
    };

    let (tasks, lists, users, current_user_id) = match (
        source,
        app.store.lists(),
        app.context.account().current_user_id(),
    ) {
        (Ok(tasks), Ok(lists), Ok(user)) => {
            // Everyone the rows might name. Small: the assignees of the tasks in one list.
            let users = tasks
                .iter()
                .filter_map(|task| task.assignee_id.as_deref())
                .filter_map(|id| app.store.user(id).ok().flatten())
                .collect::<Vec<_>>();
            (tasks, lists, users, user)
        }
        (Err(error), _, _) | (_, Err(error), _) => return Response::failed(error.into()),
        (_, _, Err(error)) => return Response::failed(error.into()),
    };

    // Borrowed the whole way down. A list of ten thousand is read once and then referred to: the
    // owned versions of these would copy every task twice per refresh, and a refresh happens every
    // time anybody touches anything in the list.
    let filtered = match &preferences {
        Some(preferences) => crate::filters::my_tasks::filter(
            &tasks,
            current_user_id.as_deref(),
            preferences,
            now,
            offset_from_utc,
        ),
        None => filters::filter_refs(
            &tasks,
            &list,
            current_user_id.as_deref(),
            now,
            offset_from_utc,
        ),
    };

    // Subtasks are spliced under their parents, so the top-level set is what gets sorted.
    let mut top_level: Vec<&crate::model::Task> = filtered
        .iter()
        .filter(|task| task.parent_task_id.is_none())
        .copied()
        .collect();
    filters::sort_by_setting(
        &mut top_level,
        list.sort_by.as_deref(),
        list.manual_sort_order.as_deref(),
    );

    let visible: std::collections::HashSet<&str> =
        filtered.iter().map(|task| task.id.as_str()).collect();
    let ordered = filters::subtasks::splice_refs(
        &top_level,
        &tasks,
        // The account's half of the rule — "inside parent task only" — beside the list's
        // (task 6ac2639a). See `filters::subtasks` for which one wins.
        filters::subtasks::should_splice(
            list.show_subtasks,
            Some(&app.context.account().smart_tasks().subtask_display),
        ),
        |task| visible.contains(task.id.as_str()),
    );

    // Only the window the shell asked for. A list of ten thousand crosses the boundary as the
    // fifty rows on screen, which is what the M0 spike was worried about.
    let start = offset.unwrap_or(0).min(ordered.len());
    let end = limit
        .map(|limit| (start + limit).min(ordered.len()))
        .unwrap_or(ordered.len());
    let window = &ordered[start..end];

    let depths_index = filters::subtasks::by_id(&tasks);
    let depths = window
        .iter()
        .map(|task| {
            (
                task.id.clone(),
                filters::subtasks::depth_of(task, &depths_index),
            )
        })
        .collect();
    let counts = rows::subtask_counts(&tasks);

    let context = RowContext {
        current_user_id: current_user_id.as_deref(),
        display_mode: resolved_display_mode(app, display_mode.as_deref()),
        surface: Command::surface(surface.as_deref()),
        now,
        offset: offset_from_utc,
        lists: &lists,
        users: &users,
        depths: &depths,
        subtask_counts: &counts,
    };

    Response::ok(serde_json::json!({
        // The total is what a virtualised list needs to size its scrollbar, and it is the count
        // AFTER filtering — the number of rows there are to scroll through, not the number of
        // tasks in the account.
        "total": ordered.len(),
        "offset": start,
        "rows": serialize_rows(&TaskRow::build_all(window, &context)),
    }))
}

/// Rows on the wire.
///
/// Hand-written rather than derived so the field names are chosen for the shell that reads them
/// and cannot drift when a Rust field is renamed for Rust reasons.
pub(super) fn serialize_rows(rows: &[TaskRow]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "title": row.title,
                "identifier": row.identifier,
                "completed": row.completed,
                "priority": row.priority.as_i64(),
                "due": due_json(&row.due),
                "isOverdue": row.is_overdue,
                "leading": leading_json(&row.leading),
                "action": match row.action {
                    rows::LeadingAction::Complete => "complete",
                    rows::LeadingAction::OpenPicker => "openPicker",
                },
                "depth": row.depth,
                "isPending": row.is_pending,
                "isPrivate": row.is_private,
                "isRepeating": row.is_repeating,
                "hasDescription": row.has_description,
                "commentCount": row.comment_count,
                "attachmentCount": row.attachment_count,
                "subtaskCount": row.subtask_count,
                "listChips": row.list_chips.iter().map(|chip| serde_json::json!({
                    "id": chip.id, "name": chip.name, "color": chip.color, "isLabel": chip.is_label
                })).collect::<Vec<_>>(),
                "assignee": row.assignee,
                "statusRole": row.status_role,
            })
        })
        .collect()
}

/// A due label as a key plus its parts. The shell owns the words; see `crate::rows`.
pub(super) fn due_json(due: &rows::DueLabel) -> serde_json::Value {
    match due {
        rows::DueLabel::None => serde_json::json!({ "key": "none" }),
        rows::DueLabel::Yesterday => serde_json::json!({ "key": "yesterday" }),
        rows::DueLabel::Today => serde_json::json!({ "key": "today" }),
        rows::DueLabel::Tomorrow => serde_json::json!({ "key": "tomorrow" }),
        rows::DueLabel::On { day, time } => serde_json::json!({
            "key": "on",
            "date": day.format("%Y-%m-%d").to_string(),
            "time": time.map(|time| time.format("%H:%M").to_string()),
        }),
    }
}

pub(super) fn leading_json(leading: &rows::LeadingControl) -> serde_json::Value {
    match leading {
        rows::LeadingControl::Checkbox => serde_json::json!({ "kind": "checkbox" }),
        rows::LeadingControl::Unassigned => serde_json::json!({ "kind": "unassigned" }),
        rows::LeadingControl::Avatar(id) => {
            serde_json::json!({ "kind": "avatar", "userId": id })
        }
    }
}
