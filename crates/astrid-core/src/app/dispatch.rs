//! Where a command becomes work.
//!
//! Every arm is short on purpose: it names a service call, and the service decides. An arm that
//! grew a rule would be a rule the shell's vocabulary had quietly acquired, in the one file where
//! nobody would look for it.

use super::command::{Command, Failure, Response};
use super::App;
use crate::filters;
use crate::model::date;
use crate::rows::{self, RowContext, TaskRow};
use crate::services::{TaskChanges, TaskDraft};

pub(crate) async fn run(app: &App, command: Command) -> Response {
    match command {
        // ── Reads ────────────────────────────────────────────────────────────────────────────
        Command::Lists => match app.context.lists().all() {
            Ok(lists) => Response::ok(lists),
            Err(error) => Response::failed(error.into()),
        },
        Command::List { list_id } => match app.context.lists().list(&list_id) {
            Ok(Some(list)) => Response::ok(list),
            Ok(None) => Response::failed(Failure::not_found("list", list_id)),
            Err(error) => Response::failed(error.into()),
        },
        Command::RowsForList {
            list_id,
            display_mode,
            surface,
            offset,
            limit,
        } => rows_for_list(app, &list_id, display_mode, surface, offset, limit),
        Command::Task { task_id } => match app.context.tasks().task(&task_id) {
            Ok(Some(task)) => Response::ok(task),
            Ok(None) => Response::failed(Failure::not_found("task", task_id)),
            Err(error) => Response::failed(error.into()),
        },
        Command::TaskDetail {
            task_id,
            display_mode,
        } => task_detail(app, &task_id, display_mode),
        Command::DueDateOptions { task_id } => due_date_options(app, &task_id),
        Command::Comments { task_id } => match app.context.comments().for_task(&task_id) {
            Ok(comments) => Response::ok(comments),
            Err(error) => Response::failed(error.into()),
        },
        Command::IsSignedIn => Response::ok(serde_json::json!({
            "signedIn": app.auth.is_signed_in().await,
            "waitingForCallback": app.auth.is_waiting(),
        })),
        Command::SearchTasks {
            query,
            list_id,
            include_completed,
            limit,
        } => search_tasks(app, &query, list_id, include_completed, limit),
        Command::AssigneeOptions { task_id } => assignee_options(app, &task_id),
        Command::CurrentUser => match app.context.account().current_user() {
            Ok(user) => Response::ok(user),
            Err(error) => Response::failed(error.into()),
        },
        Command::ResolveShortcut {
            key,
            has_selection,
            is_text_field_focused,
            is_modal_presented,
        } => {
            let context = crate::keyboard::Context {
                has_selection,
                is_text_field_focused,
                is_modal_presented,
            };
            Response::ok(serde_json::json!({
                "action": crate::keyboard::action_for(&key, context).map(action_name),
            }))
        }
        Command::Shortcuts => Response::ok(
            crate::keyboard::ALL
                .iter()
                .map(|binding| {
                    serde_json::json!({
                        "keys": binding.keys,
                        "action": action_name(binding.action),
                        "requiresSelection": binding.requires_selection,
                        "title": binding.title,
                    })
                })
                .collect::<Vec<_>>(),
        ),
        Command::OutboxStats => match crate::outbox::journal::stats(&app.store) {
            Ok(stats) => Response::ok(serde_json::json!({
                "pending": stats.pending,
                "running": stats.running,
                "completed": stats.completed,
                "failed": stats.failed,
                "hasUnsentWork": stats.has_unsent_work(),
            })),
            Err(error) => Response::failed(error.into()),
        },

        // ── Writes ───────────────────────────────────────────────────────────────────────────
        Command::CreateTask {
            title,
            description,
            list_ids,
            priority,
            due_date_time,
            is_all_day,
            assignee_id,
            parent_task_id,
        } => {
            let mut draft = TaskDraft::new(title);
            draft.description = description.unwrap_or_default();
            draft.list_ids = list_ids;
            if let Some(priority) = priority {
                draft.priority = crate::model::Priority::from_i64(priority);
            }
            draft.due_date_time = due_date_time.as_deref().and_then(date::parse);
            if let Some(all_day) = is_all_day {
                draft.is_all_day = all_day;
            }
            draft.assignee_id = assignee_id;
            draft.parent_task_id = parent_task_id;
            answer(app.context.tasks().create(&draft))
        }
        Command::UpdateTask { task_id, changes } => match changes_from_json(&changes) {
            Ok(changes) => answer(app.context.tasks().update(&task_id, &changes)),
            Err(failure) => Response::failed(failure),
        },
        // The only path to a completion, and the reason `TaskChanges::completed` is never set by
        // the shell: see `crate::services::task`.
        Command::CompleteTask { task_id, completed } => answer(
            app.context
                .tasks()
                .complete(&task_id, completed, None, None),
        ),
        Command::DeleteTask { task_id } => match app.context.tasks().delete(&task_id) {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },
        Command::SetTaskLists { task_id, list_ids } => {
            answer(app.context.tasks().set_lists(&task_id, list_ids))
        }
        Command::SetTaskStatusRole {
            task_id,
            status_role,
        } => answer(app.context.tasks().set_status_role(&task_id, status_role)),
        Command::CreateList { name, color } => answer(app.context.lists().create(&name, color)),
        Command::UpdateList { list_id, changes } => match list_changes_from_json(&changes) {
            Ok(changes) => answer(app.context.lists().update(&list_id, &changes)),
            Err(failure) => Response::failed(failure),
        },
        Command::DeleteList { list_id } => match app.context.lists().delete(&list_id) {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },
        Command::SetListFavorite { list_id, favorite } => {
            answer(app.context.lists().set_favorite(&list_id, favorite))
        }
        Command::PostComment { task_id, content } => {
            let author = app.context.account().current_user_id().ok().flatten();
            answer(app.context.comments().post(
                &task_id,
                &content,
                author.as_deref(),
                crate::model::CommentType::Text,
            ))
        }
        Command::DeleteComment { comment_id } => match app.context.comments().delete(&comment_id) {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },

        // ── The network ──────────────────────────────────────────────────────────────────────
        Command::Sync => {
            let report = app.sync.sync().await;
            Response::ok(serde_json::json!({
                "fetched": report.fetched,
                "listsAdded": report.lists_added,
                "listsUpdated": report.lists_updated,
                "tasksAdded": report.tasks_added,
                "tasksUpdated": report.tasks_updated,
                "pushesFailed": report.pushes_failed,
            }))
        }
        Command::Drain => match app.runner.drain().await {
            Ok(report) => Response::ok(serde_json::json!({
                "completed": report.completed,
                "retried": report.retried,
                "deadLettered": report.dead_lettered,
            })),
            Err(error) => Response::failed(error.into()),
        },
        Command::RefreshComments { task_id } => {
            answer(app.context.comments().refresh(&task_id).await)
        }
        Command::SearchUsers { query } => answer(app.context.account().search_users(&query).await),
        Command::RefreshCapabilities => answer(app.context.account().refresh_capabilities().await),
        Command::BeginSignIn => match app.auth.begin() {
            Ok(url) => Response::ok(serde_json::json!({ "authorizeUrl": url })),
            Err(error) => Response::failed(error.into()),
        },
        Command::CompleteSignIn { callback_url } => answer(app.auth.complete(&callback_url).await),
        Command::CancelSignIn => {
            app.auth.cancel();
            Response::done()
        }
        Command::SignOut => {
            // The flow in progress goes with the session. Leaving it would let a callback from
            // before the sign-out complete afterwards and sign the user back in.
            app.auth.cancel();
            match app.context.account().sign_out().await {
                Ok(()) => Response::done(),
                Err(error) => Response::failed(error.into()),
            }
        }
    }
}

/// The stable name the shell dispatches on.
///
/// Spelled out rather than derived from the enum's `Debug`, because these strings cross the
/// boundary and a rename made for Rust reasons would silently stop a keystroke doing anything.
fn action_name(action: crate::keyboard::ShortcutAction) -> &'static str {
    use crate::keyboard::ShortcutAction as A;
    match action {
        A::NewTask => "newTask",
        A::CompleteTask => "completeTask",
        A::DueDateEarlier => "dueDateEarlier",
        A::DueDateLater => "dueDateLater",
        A::JumpToDate => "jumpToDate",
        A::Postpone => "postpone",
        A::RemoveDueDate => "removeDueDate",
        A::EditLists => "editLists",
        A::EditTitle => "editTitle",
        A::EditDescription => "editDescription",
        A::AddComment => "addComment",
        A::AssignNoOne => "assignNoOne",
        A::PriorityNone => "priorityNone",
        A::PriorityLow => "priorityLow",
        A::PriorityMedium => "priorityMedium",
        A::PriorityHigh => "priorityHigh",
        A::DeleteTask => "deleteTask",
        A::TogglePanel => "togglePanel",
        A::CycleFilters => "cycleFilters",
        A::SelectPrevious => "selectPrevious",
        A::SelectNext => "selectNext",
        A::OutdentTask => "outdentTask",
        A::IndentTask => "indentTask",
        A::ShowShortcuts => "showShortcuts",
    }
}

/// Everything one task's detail screen needs, in one answer.
///
/// The field ORDER comes with it. That is not decoration: the same four rows are shown on web, on
/// both Apple clients and here, and the order they appear in is a product decision written down
/// once — see `crate::rows::detail`. A shell that laid them out itself would be the fifth place
/// to get it wrong.
fn task_detail(app: &App, task_id: &str, display_mode: Option<String>) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };

    let now = app.clock.now();
    let offset = app.clock.utc_offset();
    let mode = Command::display_mode(display_mode.as_deref());

    let lists = app.store.lists().unwrap_or_default();
    let chips: Vec<serde_json::Value> = task
        .effective_list_ids()
        .iter()
        .filter_map(|id| lists.iter().find(|list| &list.id == id))
        .filter(|list| list.is_domain_list())
        .map(|list| {
            serde_json::json!({
                "id": list.id, "name": list.name, "color": list.display_color()
            })
        })
        .collect();

    let assignee = task.assignee.clone().or_else(|| {
        task.assignee_id
            .as_deref()
            .and_then(|id| app.store.user(id).ok().flatten())
    });

    let comments = app.context.comments().for_task(task_id).unwrap_or_default();

    // Subtasks are the children of this task, in the order they were added — the order somebody
    // breaking a task down expects to read them back in.
    let mut subtasks: Vec<crate::model::Task> = app
        .store
        .tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|candidate| candidate.parent_task_id.as_deref() == Some(task_id))
        .collect();
    subtasks.sort_by_key(|subtask| (subtask.created_at, subtask.id.clone()));

    Response::ok(serde_json::json!({
        "task": task,
        "fieldOrder": rows::detail::field_order(mode)
            .iter()
            .map(|field| field.name())
            .collect::<Vec<_>>(),
        "priorityGlyph": rows::detail::priority_glyph(task.priority),
        "due": due_json(&rows::DueLabel::for_due(
            task.due_date_time,
            task.is_all_day,
            now,
            offset,
        )),
        "isOverdue": filters::is_overdue(&task, now, offset),
        "listChips": chips,
        "assignee": assignee,
        "comments": comments,
        "subtasks": subtasks.iter().map(|subtask| serde_json::json!({
            "id": subtask.id,
            "title": subtask.title,
            "completed": subtask.completed,
            "isPending": crate::model::is_temp_id(&subtask.id),
        })).collect::<Vec<_>>(),
    }))
}

/// The quick date and time choices for one task.
///
/// Each carries the instant it means, so the shell shows a label and sends back a value it did not
/// have to compute. `isSelected` uses the same day arithmetic the row labels use, which is why
/// `rows::day_offset` is public: a quick-pick row deciding for itself is how the tick lands on the
/// wrong row for anybody west of UTC.
fn due_date_options(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };

    let now = app.clock.now();
    let offset = app.clock.utc_offset();
    // A task with no date yet is being given one from today, so the picks are anchored on now.
    let anchor = task.due_date_time.unwrap_or(now);

    let dates: Vec<serde_json::Value> = rows::due_picks::DATE_OPTIONS
        .iter()
        .map(|option| match option.days_from_today {
            None => serde_json::json!({
                "titleKey": option.title_key,
                "dueDateTime": serde_json::Value::Null,
                "isSelected": task.due_date_time.is_none(),
            }),
            Some(days) => {
                let picked = if task.is_all_day {
                    rows::due_picks::all_day_pick(days, now, offset)
                } else {
                    // Keep the time of day: choosing a date must not silently discard a time the
                    // person already set.
                    rows::due_picks::timed_pick(
                        days - rows::day_offset(anchor, task.is_all_day, now, offset),
                        anchor,
                        offset,
                    )
                };
                serde_json::json!({
                    "titleKey": option.title_key,
                    "dueDateTime": date::format(picked),
                    "isSelected": task.due_date_time.is_some_and(|due| {
                        rows::day_offset(due, task.is_all_day, now, offset) == days
                    }),
                })
            }
        })
        .collect();

    let times: Vec<serde_json::Value> = rows::due_picks::TIME_OPTIONS
        .iter()
        .map(|option| {
            let picked = rows::due_picks::with_hour(option.hour, anchor, offset);
            serde_json::json!({
                "titleKey": option.title_key,
                "hour": option.hour,
                "dueDateTime": date::format(picked),
                // An all-day task has no time, so nothing is selected until one is chosen.
                "isSelected": !task.is_all_day
                    && task.due_date_time.is_some_and(|due| {
                        due.with_timezone(&offset).format("%H").to_string()
                            == format!("{:02}", option.hour)
                    }),
            })
        })
        .collect();

    Response::ok(serde_json::json!({
        "isAllDay": task.is_all_day,
        "dueDateTime": task.due_date_time.map(date::format),
        "dates": dates,
        "times": times,
    }))
}

/// Search the cache, and answer with rows rather than tasks.
///
/// Rows, because a result list is a list: it draws the same way, needs the same due labels and the
/// same leading control, and returning raw tasks would leave the shell to project them — which is
/// the one thing it is not allowed to do.
fn search_tasks(
    app: &App,
    query: &str,
    list_id: Option<String>,
    include_completed: Option<bool>,
    limit: Option<usize>,
) -> Response {
    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let scope = crate::services::search::SearchScope {
        list_id,
        include_completed: include_completed.unwrap_or(true),
    };
    let found = crate::services::search::matches(&tasks, query, &scope);
    let total = found.len();
    let window = &found[..limit.unwrap_or(total).min(total)];

    let lists = app.store.lists().unwrap_or_default();
    let users: Vec<crate::model::User> = window
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
        surface: rows::Surface::ListRow,
        now: app.clock.now(),
        offset: app.clock.utc_offset(),
        lists: &lists,
        users: &users,
        // Results are flat. A search result indented under a parent that did not match reads as a
        // hierarchy that is not there.
        depths: &depths,
        subtask_counts: &counts,
    };

    Response::ok(serde_json::json!({
        "total": total,
        "offset": 0,
        "rows": serialize_rows(&TaskRow::build_all(window, &context)),
    }))
}

/// Who this task can be assigned to.
///
/// Assigning itself is an ordinary `updateTask` with an `assigneeId` — null clears it — so there
/// is no separate write here. This is only the question of who may be offered.
///
/// Agents come from the account rather than from the task's lists, so they are whichever cached
/// users say they are agents. Until the core fetches the agent roster (M3) that is only the ones
/// seen embedded in a response; an agent nobody has met yet is simply not offered, which is
/// better than offering a bare id.
fn assignee_options(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };

    let lists = app.store.lists().unwrap_or_default();
    let known = app.store.users().unwrap_or_default();
    let agents: Vec<crate::model::User> = known
        .iter()
        .filter(|user| user.is_agent())
        .cloned()
        .collect();

    let current_user = app.context.account().current_user().ok().flatten();
    // The task's own assignee record is the last resort, and never wins over the id: a stale
    // embedded record is how the previous person stays on screen (task 42013da7).
    let assignee = crate::rows::assignee::resolve(
        task.assignee_id.as_deref(),
        &[known.as_slice()],
        task.assignee.as_ref(),
    );

    let list_ids = task.effective_list_ids();
    let options = crate::rows::assignee::options(&crate::rows::assignee::AssigneeSources {
        lists: &lists,
        task_list_ids: &list_ids,
        agents: &agents,
        current_assignee: assignee.as_ref(),
        current_user: current_user.as_ref(),
        ..Default::default()
    });

    Response::ok(serde_json::json!({
        "assigneeId": task.assignee_id,
        "options": options,
    }))
}

fn answer<T: serde::Serialize>(result: crate::services::Result<T>) -> Response {
    match result {
        Ok(value) => Response::ok(value),
        Err(error) => Response::failed(error.into()),
    }
}

/// Build the rows for a list: filter, sort, splice, project.
///
/// All four in one place because they are four separate contracts and a shell that ran them in its
/// own order would be four chances to show a different list from web.
fn rows_for_list(
    app: &App,
    list_id: &str,
    display_mode: Option<String>,
    surface: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Response {
    let now = app.clock.now();
    let offset_from_utc = app.clock.utc_offset();

    let list = match app.store.list(list_id) {
        Ok(Some(list)) => list,
        Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
        Err(error) => return Response::failed(error.into()),
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

    let filtered = filters::filter_for_list(
        &tasks,
        &list,
        current_user_id.as_deref(),
        now,
        offset_from_utc,
    );

    // Subtasks are spliced under their parents, so the top-level set is what gets sorted.
    let mut top_level: Vec<crate::model::Task> = filtered
        .iter()
        .filter(|task| task.parent_task_id.is_none())
        .cloned()
        .collect();
    filters::sort_by_setting(
        &mut top_level,
        list.sort_by.as_deref(),
        list.manual_sort_order.as_deref(),
    );

    let visible: std::collections::HashSet<&str> =
        filtered.iter().map(|task| task.id.as_str()).collect();
    let ordered = filters::subtasks::splice(
        &top_level,
        &tasks,
        filters::subtasks::should_splice(list.show_subtasks, None),
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
        display_mode: Command::display_mode(display_mode.as_deref()),
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
fn serialize_rows(rows: &[TaskRow]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "title": row.title,
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
                    "id": chip.id, "name": chip.name, "color": chip.color
                })).collect::<Vec<_>>(),
                "assignee": row.assignee,
                "statusRole": row.status_role,
            })
        })
        .collect()
}

/// A due label as a key plus its parts. The shell owns the words; see `crate::rows`.
fn due_json(due: &rows::DueLabel) -> serde_json::Value {
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

fn leading_json(leading: &rows::LeadingControl) -> serde_json::Value {
    match leading {
        rows::LeadingControl::Checkbox => serde_json::json!({ "kind": "checkbox" }),
        rows::LeadingControl::Unassigned => serde_json::json!({ "kind": "unassigned" }),
        rows::LeadingControl::Avatar(id) => {
            serde_json::json!({ "kind": "avatar", "userId": id })
        }
    }
}

/// Read an edit from the shape the shell sends.
///
/// A field that is **present and null** clears; a field that is **absent** is left alone. That is
/// the distinction [`TaskChanges`] exists for, and reading it from JSON is the only place it can be
/// lost — `Option<Option<T>>` through serde needs the double wrap spelled out, which is why this is
/// hand-written rather than derived.
fn changes_from_json(value: &serde_json::Value) -> Result<TaskChanges, Failure> {
    let object = value
        .as_object()
        .ok_or_else(|| Failure::bad_request("changes must be an object"))?;
    let mut changes = TaskChanges::default();

    for (key, value) in object {
        let clearable_date = |value: &serde_json::Value| -> Option<Option<_>> {
            match value {
                serde_json::Value::Null => Some(None),
                serde_json::Value::String(text) => Some(date::parse(text)),
                _ => None,
            }
        };
        match key.as_str() {
            "title" => changes.title = value.as_str().map(str::to_string),
            "description" => changes.description = value.as_str().map(str::to_string),
            "priority" => changes.priority = value.as_i64().map(crate::model::Priority::from_i64),
            "dueDateTime" => changes.due_date_time = clearable_date(value),
            "isAllDay" => changes.is_all_day = value.as_bool(),
            "assigneeId" => {
                changes.assignee_id = Some(value.as_str().map(str::to_string));
            }
            "repeating" => {
                changes.repeating = Some(serde_json::from_value(value.clone()).unwrap_or(None));
            }
            "repeatingData" => {
                changes.repeating_data =
                    Some(serde_json::from_value(value.clone()).unwrap_or(None));
            }
            "repeatFrom" => {
                changes.repeat_from = serde_json::from_value(value.clone()).ok();
            }
            "listIds" => {
                changes.list_ids = serde_json::from_value(value.clone()).ok();
            }
            "parentTaskId" => changes.parent_task_id = Some(value.as_str().map(str::to_string)),
            "statusRole" => changes.status_role = Some(value.as_str().map(str::to_string)),
            "isPrivate" => changes.is_private = value.as_bool(),
            "timerDuration" => changes.timer_duration = Some(value.as_i64()),
            "lastTimerValue" => changes.last_timer_value = Some(value.as_str().map(str::to_string)),
            // `completed` is deliberately absent. Completing a task through an update skips the
            // repeat rollover, which is rule 2 of `docs/ASTRID.md` §0 — so the shell cannot ask
            // for it here even by accident.
            "completed" | "completedAt" => {
                return Err(Failure::bad_request(
                    "complete a task with completeTask, which rolls repeating tasks over",
                ))
            }
            _ => {}
        }
    }
    Ok(changes)
}

fn list_changes_from_json(
    value: &serde_json::Value,
) -> Result<crate::services::ListChanges, Failure> {
    let object = value
        .as_object()
        .ok_or_else(|| Failure::bad_request("changes must be an object"))?;
    let mut changes = crate::services::ListChanges::default();

    for (key, value) in object {
        match key.as_str() {
            "name" => changes.name = value.as_str().map(str::to_string),
            "color" => changes.color = Some(value.as_str().map(str::to_string)),
            "description" => changes.description = Some(value.as_str().map(str::to_string)),
            "privacy" => changes.privacy = serde_json::from_value(value.clone()).ok(),
            "isFavorite" => changes.is_favorite = value.as_bool(),
            "favoriteOrder" => changes.favorite_order = Some(value.as_i64()),
            "sortBy" => changes.sort_by = Some(value.as_str().map(str::to_string)),
            "manualSortOrder" => {
                changes.manual_sort_order = serde_json::from_value(value.clone()).ok()
            }
            "showSubtasks" => changes.show_subtasks = value.as_bool(),
            "defaultAssigneeId" => {
                changes.default_assignee_id = Some(value.as_str().map(str::to_string))
            }
            "defaultPriority" => changes.default_priority = Some(value.as_i64()),
            "defaultDueTime" => changes.default_due_time = Some(value.as_str().map(str::to_string)),
            "filterCompletion" => {
                changes.filter_completion = Some(value.as_str().map(str::to_string))
            }
            "recentlyCompletedWindow" => {
                changes.recently_completed_window =
                    Some(serde_json::from_value(value.clone()).unwrap_or(None))
            }
            _ => {}
        }
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::super::tests::app_with;
    use crate::api::StubTransport;
    use crate::app::{App, Config};
    use serde_json::json;

    async fn call(app: &super::App, command: serde_json::Value) -> serde_json::Value {
        serde_json::from_str(&app.run_json(&command.to_string()).await).expect("valid JSON")
    }

    #[tokio::test]
    async fn a_task_created_through_the_door_appears_in_its_list_rows() {
        let app = app_with(StubTransport::new());
        call(&app, json!({ "kind": "createList", "name": "Home" })).await;
        let lists = call(&app, json!({ "kind": "lists" })).await;
        let list_id = lists["value"][0]["id"].as_str().expect("an id").to_string();

        call(
            &app,
            json!({ "kind": "createTask", "title": "Buy milk", "listIds": [list_id] }),
        )
        .await;

        let rows = call(&app, json!({ "kind": "rowsForList", "listId": list_id })).await;
        assert_eq!(rows["value"]["total"], 1);
        assert_eq!(rows["value"]["rows"][0]["title"], "Buy milk");
        assert_eq!(
            rows["value"]["rows"][0]["isPending"], true,
            "it has not reached the server yet, and the row says so"
        );
    }

    /// Rule 2, enforced at the boundary rather than remembered: the shell cannot complete a task
    /// through an update, because that path skips the repeat rollover.
    #[tokio::test]
    async fn the_shell_cannot_complete_a_task_by_updating_it() {
        let app = app_with(StubTransport::new());
        let created = call(&app, json!({ "kind": "createTask", "title": "Water" })).await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();

        let refused = call(
            &app,
            json!({ "kind": "updateTask", "taskId": task_id, "changes": { "completed": true } }),
        )
        .await;
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["error"]["kind"], "badRequest");
        assert!(
            refused["error"]["message"]
                .as_str()
                .expect("a message")
                .contains("completeTask"),
            "the message has to say what to use instead"
        );
    }

    /// And the command that IS allowed rolls a repeating task forward rather than finishing it.
    #[tokio::test]
    async fn completing_a_repeating_task_through_the_door_rolls_it_forward() {
        let app = app_with(StubTransport::new());
        let created = call(
            &app,
            json!({ "kind": "createTask", "title": "Water", "dueDateTime": "2026-09-07T00:00:00Z" }),
        )
        .await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();
        call(
            &app,
            json!({
                "kind": "updateTask", "taskId": task_id,
                "changes": { "repeating": "daily", "repeatFrom": "DUE_DATE" }
            }),
        )
        .await;

        let completed = call(
            &app,
            json!({ "kind": "completeTask", "taskId": task_id, "completed": true }),
        )
        .await;
        assert_eq!(completed["value"]["completed"], false);
        assert_eq!(completed["value"]["dueDateTime"], "2026-09-08T00:00:00Z");
    }

    /// Absent leaves a field alone; present-and-null clears it. Losing that distinction in the
    /// JSON layer would make "remove the due date" impossible to ask for.
    #[tokio::test]
    async fn an_explicit_null_clears_and_an_absent_field_does_not() {
        let app = app_with(StubTransport::new());
        let created = call(
            &app,
            json!({ "kind": "createTask", "title": "Buy milk", "dueDateTime": "2026-09-08T00:00:00Z" }),
        )
        .await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();

        let renamed = call(
            &app,
            json!({ "kind": "updateTask", "taskId": task_id, "changes": { "title": "Buy oat milk" } }),
        )
        .await;
        assert_eq!(renamed["value"]["dueDateTime"], "2026-09-08T00:00:00Z");

        let cleared = call(
            &app,
            json!({ "kind": "updateTask", "taskId": task_id, "changes": { "dueDateTime": null } }),
        )
        .await;
        assert!(cleared["value"]["dueDateTime"].is_null());
    }

    /// The window a virtualised list asks for, with the total it needs to size its scrollbar.
    #[tokio::test]
    async fn rows_come_back_a_window_at_a_time() {
        let app = app_with(StubTransport::new());
        call(&app, json!({ "kind": "createList", "name": "Home" })).await;
        let list_id = call(&app, json!({ "kind": "lists" })).await["value"][0]["id"]
            .as_str()
            .expect("an id")
            .to_string();
        for index in 0..10 {
            call(
                &app,
                json!({ "kind": "createTask", "title": format!("Task {index}"), "listIds": [list_id] }),
            )
            .await;
        }

        let page = call(
            &app,
            json!({ "kind": "rowsForList", "listId": list_id, "offset": 2, "limit": 3 }),
        )
        .await;
        assert_eq!(
            page["value"]["total"], 10,
            "the total is what there is to scroll"
        );
        assert_eq!(page["value"]["offset"], 2);
        assert_eq!(
            page["value"]["rows"].as_array().expect("rows").len(),
            3,
            "and only the window crosses the boundary"
        );
    }

    #[tokio::test]
    async fn asking_for_a_list_that_is_not_there_says_which_one() {
        let app = app_with(StubTransport::new());
        let answer = call(&app, json!({ "kind": "list", "listId": "nope" })).await;
        assert_eq!(answer["error"]["kind"], "notFound");
        assert_eq!(answer["error"]["id"], "nope");
    }

    /// Offline is not an error the user needs to see: the write is journalled and will go.
    #[tokio::test]
    async fn a_write_made_offline_still_succeeds() {
        let app = app_with(StubTransport::new().fallback(Err(
            crate::api::TransportError::Unreachable("no network".into()),
        )));
        let created = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
        assert_eq!(created["ok"], true);

        let stats = call(&app, json!({ "kind": "outboxStats" })).await;
        assert_eq!(stats["value"]["hasUnsentWork"], true);
    }

    #[tokio::test]
    async fn a_sync_that_cannot_reach_the_server_reports_rather_than_fails() {
        let app = app_with(StubTransport::new().fallback(Err(
            crate::api::TransportError::Unreachable("no network".into()),
        )));
        let report = call(&app, json!({ "kind": "sync" })).await;
        assert_eq!(report["ok"], true);
        assert_eq!(report["value"]["fetched"], false);
    }

    /// The sign-in commands, end to end through the door the shell uses.
    #[tokio::test]
    async fn signing_in_goes_out_through_the_browser_and_comes_back_through_a_callback() {
        let app = app_with(StubTransport::new().push_json(
            "/api/v1/auth/desktop/exchange",
            200,
            json!({
                "sessionToken": "eyJhbGciOi.token",
                "expiresAt": "2026-10-07T12:00:00Z",
                "sessionCookieName": "next-auth.session-token",
                "user": { "id": "u1", "email": "ada@example.com", "name": "Ada" }
            }),
        ));
        assert_eq!(
            call(&app, json!({ "kind": "isSignedIn" })).await["value"]["signedIn"],
            false
        );

        let began = call(&app, json!({ "kind": "beginSignIn" })).await;
        let url = began["value"]["authorizeUrl"].as_str().expect("a URL");
        let state = url::Url::parse(url)
            .expect("a URL")
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("a state");

        let completed = call(
            &app,
            json!({
                "kind": "completeSignIn",
                "callbackUrl": format!("astrid://auth/callback?code=abc&state={state}")
            }),
        )
        .await;
        assert_eq!(completed["value"]["id"], "u1");
        assert_eq!(
            call(&app, json!({ "kind": "isSignedIn" })).await["value"]["signedIn"],
            true
        );
        assert_eq!(
            call(&app, json!({ "kind": "currentUser" })).await["value"]["email"],
            "ada@example.com"
        );
    }

    /// Any web page can open the callback scheme. Without a flow in progress there is nothing to
    /// check it against, so it is refused.
    #[tokio::test]
    async fn a_callback_nobody_asked_for_is_refused() {
        let app = app_with(StubTransport::new());
        let answer = call(
            &app,
            json!({ "kind": "completeSignIn", "callbackUrl": "astrid://auth/callback?code=abc&state=x" }),
        )
        .await;
        assert_eq!(answer["ok"], false);
    }

    /// A callback that arrives after signing out must not sign the user back in.
    #[tokio::test]
    async fn signing_out_abandons_a_sign_in_in_progress() {
        let app = app_with(StubTransport::new());
        let began = call(&app, json!({ "kind": "beginSignIn" })).await;
        let url = began["value"]["authorizeUrl"]
            .as_str()
            .expect("a URL")
            .to_string();
        let state = url::Url::parse(&url)
            .expect("a URL")
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .expect("a state");

        call(&app, json!({ "kind": "signOut" })).await;
        let late = call(
            &app,
            json!({
                "kind": "completeSignIn",
                "callbackUrl": format!("astrid://auth/callback?code=abc&state={state}")
            }),
        )
        .await;
        assert_eq!(late["ok"], false);
    }

    /// The shell asks what a key means rather than knowing. The guard — never while typing,
    /// never inside a modal, selection-scoped actions need a selection — is the half of the
    /// contract that is easiest to get wrong and impossible to see in a shortcut table.
    #[tokio::test]
    async fn a_key_resolves_to_an_action_only_when_it_is_allowed_to_fire() {
        let app = app_with(StubTransport::new());

        let ready = call(
            &app,
            json!({ "kind": "resolveShortcut", "key": "x", "hasSelection": true }),
        )
        .await;
        assert_eq!(ready["value"]["action"], "completeTask");

        // Selection-scoped, with nothing selected.
        let unselected = call(&app, json!({ "kind": "resolveShortcut", "key": "x" })).await;
        assert!(unselected["value"]["action"].is_null());

        // Typing.
        let typing = call(
            &app,
            json!({
                "kind": "resolveShortcut", "key": "x",
                "hasSelection": true, "isTextFieldFocused": true
            }),
        )
        .await;
        assert!(typing["value"]["action"].is_null());

        // A modal is open.
        let modal = call(
            &app,
            json!({
                "kind": "resolveShortcut", "key": "x",
                "hasSelection": true, "isModalPresented": true
            }),
        )
        .await;
        assert!(modal["value"]["action"].is_null());
    }

    /// A Windows `VirtualKey` reads as `ArrowDown`; web and Mac write the glyph. Both resolve, so
    /// the shell does not have to translate before asking.
    #[tokio::test]
    async fn an_arrow_key_resolves_under_either_name() {
        let app = app_with(StubTransport::new());
        for key in ["ArrowDown", "\u{2193}", "j"] {
            let answer = call(&app, json!({ "kind": "resolveShortcut", "key": key })).await;
            assert_eq!(answer["value"]["action"], "selectNext", "for {key}");
        }
    }

    /// Resolving a key touches nothing. It has to be safe to ask while a key is being handled.
    #[tokio::test]
    async fn resolving_a_shortcut_reaches_no_network() {
        let transport = std::sync::Arc::new(StubTransport::new());
        let app = App::with_parts(
            &Config {
                cache_path: ":memory:".into(),
                base_url: "https://astrid.cc".into(),
            },
            std::sync::Arc::new(crate::platform::MemorySecureStore::new()),
            transport.clone(),
            std::sync::Arc::new(crate::platform::FixedClock::parsed("2026-09-07T12:00:00Z")),
        )
        .expect("starts");

        call(&app, json!({ "kind": "resolveShortcut", "key": "n" })).await;
        assert!(transport.requests().is_empty());
    }

    #[tokio::test]
    async fn the_whole_scheme_is_available_for_a_help_sheet() {
        let app = app_with(StubTransport::new());
        let answer = call(&app, json!({ "kind": "shortcuts" })).await;
        let shortcuts = answer["value"].as_array().expect("an array");
        assert!(shortcuts.len() > 10);
        assert!(shortcuts
            .iter()
            .any(|entry| entry["action"] == "newTask" && entry["requiresSelection"] == false));
    }

    /// The whole detail screen in one answer, with the field order that makes four clients agree.
    #[tokio::test]
    async fn the_detail_screen_arrives_assembled() {
        let app = app_with(StubTransport::new());
        call(&app, json!({ "kind": "createList", "name": "Home" })).await;
        let list_id = call(&app, json!({ "kind": "lists" })).await["value"][0]["id"]
            .as_str()
            .expect("an id")
            .to_string();
        let created = call(
            &app,
            json!({ "kind": "createTask", "title": "Plan the trip", "listIds": [list_id] }),
        )
        .await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();

        call(
            &app,
            json!({ "kind": "createTask", "title": "Book flights", "parentTaskId": task_id }),
        )
        .await;
        call(
            &app,
            json!({ "kind": "postComment", "taskId": task_id, "content": "asked Sam" }),
        )
        .await;
        call(
            &app,
            json!({ "kind": "updateTask", "taskId": task_id, "changes": { "priority": 3 } }),
        )
        .await;

        let detail = call(&app, json!({ "kind": "taskDetail", "taskId": task_id })).await;
        let value = &detail["value"];
        assert_eq!(value["task"]["title"], "Plan the trip");
        assert_eq!(
            value["fieldOrder"],
            json!(["assignee", "when", "priority", "lists"]),
            "the order is the contract; changing it is a cross-repo change"
        );
        assert_eq!(value["priorityGlyph"], "!!!");
        assert_eq!(value["listChips"][0]["name"], "Home");
        assert_eq!(value["subtasks"][0]["title"], "Book flights");
        assert_eq!(value["comments"][0]["content"], "asked Sam");
    }

    /// Project mode has no assignee or priority row — both live behind the leading control.
    #[tokio::test]
    async fn project_mode_asks_for_a_shorter_detail_screen() {
        let app = app_with(StubTransport::new());
        let created = call(&app, json!({ "kind": "createTask", "title": "x" })).await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();

        let detail = call(
            &app,
            json!({ "kind": "taskDetail", "taskId": task_id, "displayMode": "project" }),
        )
        .await;
        assert_eq!(detail["value"]["fieldOrder"], json!(["when", "lists"]));
    }

    #[tokio::test]
    async fn asking_for_a_task_that_is_not_there_says_which_one() {
        let app = app_with(StubTransport::new());
        let answer = call(&app, json!({ "kind": "taskDetail", "taskId": "nope" })).await;
        assert_eq!(answer["error"]["kind"], "notFound");
        assert_eq!(answer["error"]["id"], "nope");
    }

    /// The quick picks arrive with the instant each one means, so the shell shows a label and
    /// sends back a value it did not have to compute.
    #[tokio::test]
    async fn the_quick_date_picks_carry_the_instants_they_mean() {
        let app = app_with(StubTransport::new());
        let created = call(
            &app,
            json!({ "kind": "createTask", "title": "Water", "dueDateTime": "2026-09-07T00:00:00Z" }),
        )
        .await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();

        let options = call(&app, json!({ "kind": "dueDateOptions", "taskId": task_id })).await;
        let dates = options["value"]["dates"].as_array().expect("an array");

        // Clearing comes first, and it is a choice like any other.
        assert_eq!(dates[0]["titleKey"], "picker.no_due_date");
        assert!(dates[0]["dueDateTime"].is_null());

        assert_eq!(dates[1]["titleKey"], "picker.today");
        assert_eq!(dates[1]["dueDateTime"], "2026-09-07T00:00:00Z");
        assert_eq!(dates[1]["isSelected"], true, "it is due today");
        assert_eq!(dates[2]["dueDateTime"], "2026-09-08T00:00:00Z");
        assert_eq!(dates[4]["dueDateTime"], "2026-09-14T00:00:00Z");
    }

    /// The tick goes on the row the task is actually set to, computed the same way the row label
    /// is — a picker deciding for itself is how it lands on the wrong row west of UTC.
    #[tokio::test]
    async fn nothing_is_ticked_when_a_task_has_no_date() {
        let app = app_with(StubTransport::new());
        let created = call(&app, json!({ "kind": "createTask", "title": "Water" })).await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();

        let options = call(&app, json!({ "kind": "dueDateOptions", "taskId": task_id })).await;
        let dates = options["value"]["dates"].as_array().expect("an array");
        assert_eq!(
            dates[0]["isSelected"], true,
            "no due date is the selected choice"
        );
        assert!(dates[1..].iter().all(|date| date["isSelected"] == false));
    }

    /// An all-day task has no time of day, so no time is ticked until one is chosen.
    #[tokio::test]
    async fn an_all_day_task_has_no_time_selected() {
        let app = app_with(StubTransport::new());
        let created = call(
            &app,
            json!({ "kind": "createTask", "title": "Water", "dueDateTime": "2026-09-07T00:00:00Z" }),
        )
        .await;
        let task_id = created["value"]["id"].as_str().expect("an id").to_string();

        let options = call(&app, json!({ "kind": "dueDateOptions", "taskId": task_id })).await;
        assert_eq!(options["value"]["isAllDay"], true);
        let times = options["value"]["times"].as_array().expect("an array");
        assert_eq!(times[0]["titleKey"], "picker.morning");
        assert_eq!(times[0]["dueDateTime"], "2026-09-07T09:00:00Z");
        assert!(times.iter().all(|time| time["isSelected"] == false));
    }

    /// A virtual list — "Today", "Not in a List", "I've Assigned" — has no membership. It is a
    /// saved set of filters over everything, and sourcing it from membership the way a real list
    /// is sourced gives an empty screen with nothing to explain it.
    #[tokio::test]
    async fn a_virtual_list_filters_everything_rather_than_its_own_membership() {
        let app = app_with(StubTransport::new());

        // A real list with one task in it, and a task in no list at all.
        call(&app, json!({ "kind": "createList", "name": "Home" })).await;
        let list_id = call(&app, json!({ "kind": "lists" })).await["value"][0]["id"]
            .as_str()
            .expect("an id")
            .to_string();
        call(
            &app,
            json!({ "kind": "createTask", "title": "Buy milk", "listIds": [list_id] }),
        )
        .await;
        call(&app, json!({ "kind": "createTask", "title": "Loose end" })).await;

        // The "Not in a List" virtual list, exactly as the server stores it.
        let virtual_list: crate::model::TaskList = serde_json::from_value(json!({
            "id": "v-not-in-list",
            "name": "Not in a List",
            "isVirtual": true,
            "virtualListType": "not-in-list",
            "filterInLists": "not_in_list",
            "filterCompletion": "default",
            "sortBy": "auto"
        }))
        .expect("decodes");
        app.store.upsert_list(&virtual_list).expect("stores");

        let rows = call(
            &app,
            json!({ "kind": "rowsForList", "listId": "v-not-in-list" }),
        )
        .await;
        assert_eq!(rows["value"]["total"], 1);
        assert_eq!(rows["value"]["rows"][0]["title"], "Loose end");
    }

    /// And a virtual list with no filters shows everything, rather than everything in a list that
    /// does not exist.
    #[tokio::test]
    async fn an_unfiltered_virtual_list_shows_the_whole_account() {
        let app = app_with(StubTransport::new());
        call(&app, json!({ "kind": "createTask", "title": "one" })).await;
        call(&app, json!({ "kind": "createTask", "title": "two" })).await;

        let virtual_list: crate::model::TaskList = serde_json::from_value(json!({
            "id": "v-all", "name": "Everything", "isVirtual": true
        }))
        .expect("decodes");
        app.store.upsert_list(&virtual_list).expect("stores");

        let rows = call(&app, json!({ "kind": "rowsForList", "listId": "v-all" })).await;
        assert_eq!(rows["value"]["total"], 2);
    }

    /// Search answers with rows, so a result list draws exactly like any other list.
    #[tokio::test]
    async fn search_answers_with_rows_over_the_cache() {
        let app = app_with(StubTransport::new());
        call(
            &app,
            json!({ "kind": "createTask", "title": "Book flights" }),
        )
        .await;
        call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;

        let found = call(&app, json!({ "kind": "searchTasks", "query": "book" })).await;
        assert_eq!(found["value"]["total"], 1);
        assert_eq!(found["value"]["rows"][0]["title"], "Book flights");
        // A row, with everything a row has.
        assert!(found["value"]["rows"][0]["leading"].is_object());
    }

    /// Results are flat: one indented under a parent that did not match reads as a hierarchy that
    /// is not there.
    #[tokio::test]
    async fn search_results_are_not_indented() {
        let app = app_with(StubTransport::new());
        let parent = call(
            &app,
            json!({ "kind": "createTask", "title": "Plan the trip" }),
        )
        .await;
        let parent_id = parent["value"]["id"].as_str().expect("an id").to_string();
        call(
            &app,
            json!({ "kind": "createTask", "title": "Book flights", "parentTaskId": parent_id }),
        )
        .await;

        let found = call(&app, json!({ "kind": "searchTasks", "query": "flights" })).await;
        assert_eq!(found["value"]["rows"][0]["depth"], 0);
    }

    #[tokio::test]
    async fn a_search_too_short_to_mean_anything_finds_nothing() {
        let app = app_with(StubTransport::new());
        call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;

        let found = call(&app, json!({ "kind": "searchTasks", "query": "b" })).await;
        assert_eq!(found["value"]["total"], 0);
    }

    /// The picker offers unassigned first and knows which row is the current one, so the shell
    /// draws a tick beside it without deciding anything.
    #[tokio::test]
    async fn assignee_options_offer_unassigned_and_say_who_holds_the_task() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Book flights" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let offered = call(&app, json!({ "kind": "assigneeOptions", "taskId": id })).await;
        assert_eq!(
            offered["value"]["options"][0]["userId"],
            serde_json::Value::Null
        );
        assert_eq!(offered["value"]["assigneeId"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn assignee_options_for_a_task_that_is_not_there_are_a_not_found() {
        let app = app_with(StubTransport::new());
        let answered = call(&app, json!({ "kind": "assigneeOptions", "taskId": "nope" })).await;
        assert_eq!(answered["error"]["kind"], "notFound");
    }

    #[tokio::test]
    async fn signing_out_empties_everything() {
        let app = app_with(StubTransport::new());
        call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
        call(&app, json!({ "kind": "signOut" })).await;
        let stats = call(&app, json!({ "kind": "outboxStats" })).await;
        assert_eq!(stats["value"]["hasUnsentWork"], false);
    }
}
