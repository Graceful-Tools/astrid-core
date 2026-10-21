//! The rows a list draws, and how a row is written to JSON.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Where the rows go and which of them: the projection's settings and the window.
pub(super) struct RowsWindow {
    pub display_mode: Option<String>,
    pub surface: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// Build the rows for a list: filter, sort, splice, project.
///
/// All four in one place because they are four separate contracts and a shell that ran them in its
/// own order would be four chances to show a different list from web.
///
/// Every step answers as iOS's list does (AITD-460; Jon, 2026-10-03: on disagreement follow iOS):
/// the tasks are read in iOS's store order first ([`filters::display_order`]), so ties in every
/// sort fall the same way; subtasks are spliced from every cached subtask, not only the list's
/// members; and a spliced subtask obeys the view's completion filter alone
/// ([`filters::shown_by_completion`]).
pub(super) fn rows_for_list(
    app: &App,
    list_id: &str,
    window: RowsWindow,
    inputs: crate::app::command::RowsInputs,
) -> Response {
    let now = app.clock.now();
    let offset_from_utc = app.clock.utc_offset();
    let RowsWindow {
        display_mode,
        surface,
        offset,
        limit,
    } = window;

    // My Tasks is not in the list collection — it is the view the app opens on, and its filters
    // belong to the account rather than to a list row. Everything after this is the same pipeline.
    let my_tasks = list_id == crate::filters::my_tasks::VIRTUAL_ID;
    let preferences = match (my_tasks, inputs.my_tasks) {
        (false, _) => None,
        (true, Some(held)) => Some(held),
        (true, None) => match app.context.account().my_tasks_preferences() {
            Ok(preferences) => Some(preferences),
            Err(error) => return Response::failed(error.into()),
        },
    };
    let mut list = match (&preferences, inputs.list) {
        // A shape rather than a row: the sort setting is read off it below, the same as any
        // list's, and the completion setting decides which subtasks show.
        (Some(preferences), _) => {
            let mut shape = crate::model::TaskList::new(crate::filters::my_tasks::VIRTUAL_ID, "");
            shape.is_virtual = Some(true);
            shape.filter_completion = Some(preferences.filter_completion.clone());
            shape.sort_by = Some(preferences.sort_by.clone());
            shape.manual_sort_order = Some(preferences.manual_sort_order.clone());
            shape
        }
        (None, Some(held)) => held,
        (None, None) => match app.store.list(list_id) {
            Ok(Some(list)) => list,
            Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
            Err(error) => return Response::failed(error.into()),
        },
    };
    if let Some(sort_by) = inputs.sort_by {
        list.sort_by = Some(sort_by);
    }
    // A list that has never been given a sort stays unset here: `filters::sort_by_setting` and
    // the answer's `sortBy` both read the absence as auto, as web does (CONTRACTS D40).

    // A VIRTUAL list has no membership: it is a saved set of filters over everything the account
    // has — "Today", "Not in a List", "I've Assigned". Sourcing it from membership, the way a real
    // list is sourced, gives an empty screen with nothing to explain it, which is what makes this
    // worth a branch rather than a clever query.
    let source = match inputs.tasks {
        Some(held) => Ok(held),
        None if list.is_virtual.unwrap_or(false) => app.store.tasks(),
        None => app.store.tasks_in_list(&list.id),
    };
    let current_user_id = match inputs.current_user_id {
        Some(held) => Ok(Some(held)),
        None => app.context.account().current_user_id(),
    };
    let (mut tasks, current_user_id) = match (source, current_user_id) {
        (Ok(tasks), Ok(user)) => (tasks, user),
        (Err(error), _) => return Response::failed(error.into()),
        (_, Err(error)) => return Response::failed(error.into()),
    };
    tasks.sort_by(filters::display_order);

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

    // The account's half of the rule — "inside parent task only" — beside the list's (task
    // 6ac2639a). See `filters::subtasks` for which one wins.
    let subtask_display = inputs
        .subtask_display
        .unwrap_or_else(|| app.context.account().smart_tasks().subtask_display);
    let indented = filters::subtasks::should_splice(list.show_subtasks, Some(&subtask_display));
    // Every cached subtask, wherever it is filed: iOS splices from its whole store, so a subtask
    // that was never added to its parent's list still shows under it.
    // Read for the projection too: a row's subtask count is every subtask it has.
    let mut subtasks = match indented || !inputs.ids_only {
        true => match app.store.subtasks() {
            Ok(subtasks) => subtasks,
            Err(error) => return Response::failed(error.into()),
        },
        false => Vec::new(),
    };
    subtasks.sort_by(filters::display_order);
    let ordered = filters::subtasks::splice_refs(&top_level, &subtasks, indented, |task| {
        filters::shown_by_completion(task, &list, now, offset_from_utc)
    });

    // Only the window the shell asked for. A list of ten thousand crosses the boundary as the
    // fifty rows on screen, which is what the M0 spike was worried about.
    let start = offset.unwrap_or(0).min(ordered.len());
    let end = limit
        .map(|limit| (start + limit).min(ordered.len()))
        .unwrap_or(ordered.len());
    let window = &ordered[start..end];
    let sort_by = list.sort_by.as_deref().unwrap_or("auto");

    if inputs.ids_only {
        return Response::ok(serde_json::json!({
            "total": ordered.len(),
            "offset": start,
            "matched": filtered.len(),
            "ids": window.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(),
            "sortBy": sort_by,
        }));
    }

    let mut lists = match app.store.lists() {
        Ok(lists) => lists,
        Err(error) => return Response::failed(error.into()),
    };
    // A list handed in that this account does not hold — a featured public list — still decides
    // which rows are someone else's, read-only, copy-only (AATD-46).
    if !lists.iter().any(|held| held.id == list.id) {
        lists.push(list.clone());
    }
    // Everyone the rows might name. Small: the assignees of the tasks in one list.
    let users = tasks
        .iter()
        .filter_map(|task| task.assignee_id.as_deref())
        .filter_map(|id| app.store.user(id).ok().flatten())
        .collect::<Vec<_>>();
    // Every task a row's parent chain can pass through: the list's and the subtasks, each once
    // (a subtask filed in the list is in both). One copy, as before the subtasks were read.
    let depths_index: std::collections::HashMap<String, crate::model::Task> = tasks
        .iter()
        .chain(subtasks.iter())
        .map(|task| (task.id.clone(), task.clone()))
        .collect();
    let depths = window
        .iter()
        .map(|task| {
            (
                task.id.clone(),
                filters::subtasks::depth_of(task, &depths_index),
            )
        })
        .collect();
    let counts = rows::subtask_counts(&subtasks);

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

    // A virtual list always; a real one by the reader's right to add to it.
    let can_add_tasks = list.is_virtual.unwrap_or(false)
        || current_user_id.as_deref().is_some_and(|me| {
            crate::permissions::can_edit_tasks(me, &crate::permissions::access_of(&list))
        });
    Response::ok(serde_json::json!({
        // The total is what a virtualised list needs to size its scrollbar, and it is the count
        // AFTER filtering — the number of rows there are to scroll through, not the number of
        // tasks in the account.
        "total": ordered.len(),
        "offset": start,
        // How many tasks the filters kept, subtasks included — the sidebar's number.
        "matched": filtered.len(),
        "rows": serialize_rows(&TaskRow::build_all(window, &context)),
        // How the rows were sorted, so the shell knows whether a drag may rearrange them
        // (task 7883f710): only an order made by hand survives a refresh.
        "sortBy": sort_by,
        // Whether the quick-add bar is offered here (iOS ListPermissions.canAddTasks, AATD-37).
        "canAddTasks": can_add_tasks,
    }))
}

/// How many tasks each list's saved filters keep (`Command::ListCounts`, AITD-460).
///
/// One read of the account for all of them: the sidebar asks for every saved filter at once.
pub(super) fn list_counts(
    app: &App,
    lists: &[crate::model::TaskList],
    current_user_id: Option<String>,
) -> Response {
    let now = app.clock.now();
    let offset_from_utc = app.clock.utc_offset();
    let current_user_id = match current_user_id {
        Some(held) => Some(held),
        None => match app.context.account().current_user_id() {
            Ok(user) => user,
            Err(error) => return Response::failed(error.into()),
        },
    };
    let everything = match lists.iter().any(|list| list.is_virtual.unwrap_or(false)) {
        true => match app.store.tasks() {
            Ok(tasks) => tasks,
            Err(error) => return Response::failed(error.into()),
        },
        false => Vec::new(),
    };
    let mut counts = serde_json::Map::new();
    for list in lists {
        let members;
        let source = match list.is_virtual.unwrap_or(false) {
            true => &everything,
            false => {
                members = match app.store.tasks_in_list(&list.id) {
                    Ok(tasks) => tasks,
                    Err(error) => return Response::failed(error.into()),
                };
                &members
            }
        };
        let kept = filters::filter_refs(
            source,
            list,
            current_user_id.as_deref(),
            now,
            offset_from_utc,
        )
        .len();
        counts.insert(list.id.clone(), serde_json::json!(kept));
    }
    Response::ok(serde_json::Value::Object(counts))
}

/// iOS's ListTaskCount: open tasks per list, a saved filter's kept count, a public list's server
/// count, and My Tasks as the reader's open assigned tasks (AATD-39).
pub(super) fn list_badges(
    app: &App,
    lists: &[crate::model::TaskList],
    my_tasks_id: Option<String>,
    current_user_id: Option<String>,
) -> Response {
    let current_user_id = match current_user_id {
        Some(held) => Some(held),
        None => match app.context.account().current_user_id() {
            Ok(user) => user,
            Err(error) => return Response::failed(error.into()),
        },
    };
    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let virtual_lists: Vec<crate::model::TaskList> = lists
        .iter()
        .filter(|list| list.is_virtual.unwrap_or(false))
        .cloned()
        .collect();
    let kept = match list_counts(app, &virtual_lists, current_user_id.clone()).value {
        Some(serde_json::Value::Object(kept)) => kept,
        _ => serde_json::Map::new(),
    };
    let mut open_by_list: std::collections::HashMap<String, usize> = Default::default();
    for task in tasks.iter().filter(|task| !task.completed) {
        for id in task.effective_list_ids() {
            *open_by_list.entry(id).or_insert(0) += 1;
        }
    }
    let mut badges = serde_json::Map::new();
    for list in lists {
        let count =
            if list.privacy == Some(crate::model::Privacy::Public) && list.task_count.is_some() {
                serde_json::json!(list.task_count)
            } else if list.is_virtual.unwrap_or(false) {
                kept.get(&list.id).cloned().unwrap_or(serde_json::json!(0))
            } else {
                serde_json::json!(open_by_list.get(&list.id).copied().unwrap_or(0))
            };
        badges.insert(list.id.clone(), count);
    }
    if let Some(id) = my_tasks_id {
        let mine = current_user_id.as_deref().map_or(0, |me| {
            tasks
                .iter()
                .filter(|task| !task.completed && task.assignee_id.as_deref() == Some(me))
                .count()
        });
        badges.insert(id, serde_json::json!(mine));
    }
    Response::ok(serde_json::Value::Object(badges))
}

/// A featured public list's rows: the server's tasks plus this account's unsent ones for it,
/// through the same projection as any list (AATD-46).
pub(super) async fn featured_rows(
    app: &App,
    list_id: &str,
    held: Option<crate::model::TaskList>,
) -> Response {
    let fetched = match app.context.lists().public_list_tasks(list_id).await {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let mut tasks = fetched;
    // What was added here and has not reached the server yet stays on screen (iOS).
    for unsent in app
        .store
        .tasks_in_list(list_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|task| crate::model::is_temp_id(&task.id))
    {
        if !tasks.iter().any(|task| task.id == unsent.id) {
            tasks.insert(0, unsent);
        }
    }
    let list = held
        .or_else(|| app.store.list(list_id).ok().flatten())
        .unwrap_or_else(|| {
            let mut list = crate::model::TaskList::new(list_id, "");
            list.privacy = Some(crate::model::Privacy::Public);
            list
        });
    rows_for_list(
        app,
        list_id,
        RowsWindow {
            display_mode: None,
            surface: None,
            offset: None,
            limit: None,
        },
        crate::app::command::RowsInputs {
            list: Some(list),
            tasks: Some(tasks),
            ..Default::default()
        },
    )
}

/// A profile's shared tasks, as rows (task da326190).
///
/// The tasks are the server's — up to twenty public ones the person created or was assigned
/// (`app/api/v1/users/[userId]/profile/route.ts`) — and are projected rather than cached: they
/// belong to somebody else's public lists, and writing them into this account's store would put
/// another person's tasks in its sidebar counts and its search.
///
/// The section is read-only, so every row's leading control is a plain mark rather than a
/// complete box: tapping to finish someone else's task is not something this screen offers.
pub(super) async fn profile_rows(app: &App, user_id: Option<&str>) -> Response {
    let me = app.context.account().current_user_id().ok().flatten();
    // Signed out there is nobody whose profile this is, and an empty section would read as an
    // account that shares nothing — the same refusal `ProfileStats` gives.
    let asked_for = match user_id.map(str::to_string).or_else(|| me.clone()) {
        Some(id) => id,
        None => return Response::failed(Failure::unauthorized()),
    };
    let answer = match app.context.account().profile(&asked_for).await {
        Ok(answer) => answer,
        Err(error) => return Response::failed(error.into()),
    };
    let tasks: Vec<crate::model::Task> = answer
        .get("sharedTasks")
        .and_then(|tasks| tasks.as_array())
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(|task| serde_json::from_value(task.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let shown: Vec<&crate::model::Task> = tasks.iter().collect();
    let lists = app.store.lists().unwrap_or_default();
    let users = app.store.users().unwrap_or_default();
    let depths = std::collections::HashMap::new();
    let counts = std::collections::HashMap::new();
    let context = RowContext {
        current_user_id: me.as_deref(),
        display_mode: resolved_display_mode(app, None),
        surface: Command::surface(None),
        now: app.clock.now(),
        offset: app.clock.utc_offset(),
        lists: &lists,
        users: &users,
        depths: &depths,
        subtask_counts: &counts,
    };
    let rows = serialize_rows(&TaskRow::build_all(&shown, &context));
    Response::ok(serde_json::json!({
        "rows": rows,
        "count": rows.len(),
        // The server's own answer, not a comparison made here: on your own profile it is true
        // whichever id was asked for.
        "isOwnProfile": answer
            .get("isOwnProfile")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_else(|| me.as_deref() == Some(asked_for.as_str())),
        "name": answer.pointer("/user/name").cloned().unwrap_or(serde_json::Value::Null),
        "image": answer.pointer("/user/image").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

/// iOS's ReminderTriage.deck and .assistant (AATD-54).
pub(super) fn reminder_triage_deck(app: &App, reminded_task_id: Option<&str>) -> Response {
    let now = app.clock.now();
    let offset = app.clock.utc_offset();
    let Some(me) = app.context.account().current_user_id().ok().flatten() else {
        return Response::ok(serde_json::json!({ "rows": [], "assistant": null }));
    };
    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let due_now = |task: &crate::model::Task| {
        task.due_date_time
            .is_some_and(|due| rows::day_offset(due, task.is_all_day, now, offset) == 0)
            || filters::is_overdue(task, now, offset)
    };
    let mut deck: Vec<&crate::model::Task> = tasks
        .iter()
        .filter(|task| {
            task.assignee_id.as_deref() == Some(me.as_str()) && !task.completed && due_now(task)
        })
        .collect();
    deck.sort_by_key(|task| task.due_date_time);
    if let Some(reminded) = reminded_task_id
        .and_then(|id| tasks.iter().find(|task| task.id == id))
        .filter(|task| !task.completed)
        // Somebody else's task is theirs to triage, not mine.
        .filter(|task| {
            task.assignee_id
                .as_deref()
                .is_none_or(|assignee| assignee == me)
        })
    {
        deck.retain(|task| task.id != reminded.id);
        deck.insert(0, reminded);
    }
    let lists = app.store.lists().unwrap_or_default();
    let users = app.store.users().unwrap_or_default();
    let depths = std::collections::HashMap::new();
    let counts = std::collections::HashMap::new();
    let context = RowContext {
        current_user_id: Some(me.as_str()),
        display_mode: resolved_display_mode(app, None),
        surface: Command::surface(None),
        now,
        offset,
        lists: &lists,
        users: &users,
        depths: &depths,
        subtask_counts: &counts,
    };
    // The account's own assistant: the agent iOS's AvailableAgent.defaultAssistantService names.
    let assistant = users
        .iter()
        .find(|user| user.is_agent() && user.ai_agent_type.as_deref() == Some("astrid"))
        .map(|user| serde_json::json!({ "id": user.id, "name": user.name }));
    Response::ok(serde_json::json!({
        "rows": serialize_rows(&TaskRow::build_all(&deck, &context)),
        "assistant": assistant,
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
                "showsIdentifier": row.shows_identifier,
                "offersCopyIdentifier": row.offers_copy_identifier,
                "completed": row.completed,
                "priority": row.priority.as_i64(),
                "due": due_json(&row.due),
                "dueAt": row.due_at.map(|at| serde_json::json!({
                    "date": at.format("%Y-%m-%d").to_string(),
                    "time": at.format("%H:%M").to_string(),
                })),
                "isOverdue": row.is_overdue,
                "leading": leading_json(&row.leading),
                "action": match row.action {
                    rows::LeadingAction::Complete => "complete",
                    rows::LeadingAction::OpenPicker => "openPicker",
                    rows::LeadingAction::Copy => "copy",
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
                    "id": chip.id, "name": chip.name, "color": chip.color, "isLabel": chip.is_label,
                    "glyph": chip.glyph.as_str()
                })).collect::<Vec<_>>(),
                "assignee": row.assignee,
                "statusRole": row.status_role,
                "isCanceled": row.is_canceled,
                "syncMark": row.sync_mark,
                "remote": row.remote_chip.as_ref().map(|(label, url)| serde_json::json!({ "label": label, "url": url })),
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
        rows::LeadingControl::Copy => serde_json::json!({ "kind": "copy" }),
        rows::LeadingControl::Avatar(id) => {
            serde_json::json!({ "kind": "avatar", "userId": id })
        }
    }
}
