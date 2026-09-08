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
        Command::Board { list_id, limit } => board(app, &list_id, limit),
        Command::MoveTaskToColumn {
            task_id,
            column_id,
            list_id,
        } => move_task_to_column(app, &task_id, &column_id, &list_id),
        Command::ReminderOptions { task_id } => reminder_options(app, &task_id),
        Command::RemindersDue => reminders_due(app),
        Command::ReminderShown { task_id } => mark_reminder_shown(app, &task_id),
        Command::SnoozeReminder { task_id, minutes } => snooze_reminder(app, &task_id, minutes),
        Command::RepeatOptions { task_id } => repeat_options(app, &task_id),
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
                "action": crate::keyboard::action_for(&key, context).map(crate::keyboard::action_name),
            }))
        }
        Command::Shortcuts => Response::ok(
            crate::keyboard::ALL
                .iter()
                .map(|binding| {
                    serde_json::json!({
                        "keys": binding.keys,
                        "action": crate::keyboard::action_name(binding.action),
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
                None,
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
        Command::Agents => agents(app).await,
        Command::ConnectCopilot => match app.context.agents().copilot_authorize_url().await {
            Ok(url) => Response::ok(serde_json::json!({ "authorizeUrl": url })),
            Err(error) => Response::failed(error.into()),
        },
        Command::DisconnectCopilot => answer_done(app.context.agents().disconnect_copilot().await),
        Command::SetAgentMode { agent, mode } => {
            answer(app.context.agents().set_mode(&agent, mode).await)
        }
        Command::SaveAgentCredential { service_id, key } => answer_done(
            app.context
                .agents()
                .save_credential(&service_id, &key)
                .await,
        ),
        Command::TestAgentCredential { service_id } => {
            match app.context.agents().test_credential(&service_id).await {
                Ok(works) => Response::ok(serde_json::json!({ "works": works })),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::DeleteAgentCredential { service_id } => {
            answer_done(app.context.agents().delete_credential(&service_id).await)
        }
        Command::ExternalSync { list_id } => external_sync(app, &list_id).await,
        Command::ConnectProvider { provider } => {
            match app.context.external().authorize_url(provider).await {
                Ok(url) => Response::ok(serde_json::json!({ "authorizeUrl": url })),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::DisconnectProvider { provider } => {
            answer_done(app.context.external().disconnect(provider).await)
        }
        Command::LinkList {
            provider,
            list_id,
            container_id,
        } => answer(
            app.context
                .external()
                .link(provider, &list_id, &container_id)
                .await,
        ),
        Command::UnlinkList { provider, link_id } => {
            answer_done(app.context.external().unlink(provider, &link_id).await)
        }
        Command::SyncExternal => sync_external(app).await,
        Command::HasSeenTour => Response::ok(serde_json::json!({
            "seen": app
                .store
                .metadata(TOUR_KEY)
                .ok()
                .flatten()
                .is_some_and(|value| value == "yes"),
        })),
        Command::TourSeen => match app.store.set_metadata(TOUR_KEY, "yes") {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },
        Command::Palette { query } => {
            let lists = app.store.lists().unwrap_or_default();
            let tasks = app.store.tasks().unwrap_or_default();
            Response::ok(serde_json::json!({
                "rows": crate::palette::search(&query, &lists, &tasks),
            }))
        }
        Command::ProfileStats => {
            let me = app.context.account().current_user_id().ok().flatten();
            match me {
                Some(id) => answer(app.context.account().stats(&id).await),
                // Signed out there is nobody to have statistics about, and zeroes would read as an
                // account that has done nothing.
                None => Response::failed(Failure::unauthorized()),
            }
        }
        Command::ExportAccount { format, path } => {
            match app
                .context
                .account()
                .export(&format, std::path::Path::new(&path))
                .await
            {
                Ok(bytes) => Response::ok(serde_json::json!({ "path": path, "bytes": bytes })),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::Settings => settings(app),
        Command::RefreshSettings => {
            // The user first: a settings screen with no name on it looks broken in a way the
            // settings themselves do not.
            let _ = app.context.account().refresh_current_user().await;
            match app.context.account().refresh_settings().await {
                Ok(_) => settings(app),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::UpdateReminderSettings { changes } => {
            match update_reminder_settings(app, changes).await {
                Ok(()) => settings(app),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::StartTimer { task_id } => start_timer(app, &task_id),
        Command::StopTimer { task_id } => stop_timer(app, &task_id),
        Command::Attachments { task_id } => attachments(app, &task_id),
        Command::DownloadAttachment { task_id, file_id } => {
            download_attachment(app, &task_id, &file_id).await
        }
        Command::AttachFile {
            task_id,
            path,
            content,
        } => attach_file(app, &task_id, &path, content.as_deref()).await,
        Command::FilterOptions { list_id } => filter_options(app, &list_id),
        Command::Chat { list_id } => chat(app, &list_id),
        Command::RefreshChat { list_id } => refresh_chat(app, &list_id).await,
        Command::SendChatMessage {
            channel_id,
            content,
            reply_to_id,
        } => {
            let author = app.context.account().current_user_id().ok().flatten();
            answer(app.context.chat().send(
                &channel_id,
                &content,
                author.as_deref(),
                reply_to_id.as_deref(),
            ))
        }
        Command::ListMembers { list_id } => list_members(app, &list_id).await,
        Command::InviteToList {
            list_id,
            email,
            role,
        } => answer_done(app.context.lists().invite(&list_id, &email, &role).await),
        Command::SetMemberRole {
            list_id,
            user_id,
            role,
        } => answer_done(
            app.context
                .lists()
                .set_member_role(&list_id, &user_id, &role)
                .await,
        ),
        Command::RemoveMember { list_id, user_id } => {
            answer_done(app.context.lists().remove_member(&list_id, &user_id).await)
        }
        Command::LeaveList { list_id } => answer_done(app.context.lists().leave(&list_id).await),
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
        // The timer, running or not: the section is shown while one runs, and a task with recorded
        // time keeps its caption, so hiding the section never hides the data.
        "timer": timer_state(app, &task),
        // A custom repeat cannot describe itself in a chip: "Custom" says nothing, and the pattern
        // does not fit beside a date and a time. The detail gives it its own row, worded exactly
        // as the picker words it, or the same repeat reads two ways on one screen.
        "repeatSummary": rows::repeat::summary(
            task.repeating,
            task.repeating_data.as_ref(),
            task.repeat_from,
        ),
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

/// How many cards a column carries across the boundary unless the shell asks for more.
///
/// A column is read top-down and the count comes back whole, so a hundred-card Done column crosses
/// as the handful anybody is looking at — the same reason `rowsForList` sends a window.
const BOARD_COLUMN_LIMIT: usize = 50;

/// The account screen: who is signed in, their reminder settings, and the choices for them.
///
/// The offsets come from the same list the per-task reminder picker uses, so "15 minutes before"
/// means one thing in this app rather than two.
fn settings(app: &App) -> Response {
    let account = app.context.account();
    let settings = account.settings().unwrap_or_else(|_| serde_json::json!({}));
    let reminders = settings
        .get("reminderSettings")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let offsets: Vec<serde_json::Value> = rows::reminder_picks::OFFSETS
        .iter()
        .map(
            |(title_key, minutes)| serde_json::json!({ "titleKey": title_key, "minutes": minutes }),
        )
        .collect();

    Response::ok(serde_json::json!({
        "user": account.current_user().ok().flatten(),
        "reminderSettings": reminders,
        "offsets": offsets,
        // What the server should schedule a digest against. The reader's zone, from the clock the
        // core was given, rather than a string the shell types.
        "timezone": app.clock.utc_offset().to_string(),
    }))
}

/// Merge changes into the stored reminder settings and write them back.
async fn update_reminder_settings(
    app: &App,
    changes: serde_json::Value,
) -> crate::services::Result<()> {
    let account = app.context.account();
    let mut reminders = account
        .settings()?
        .get("reminderSettings")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if let (Some(target), Some(source)) = (reminders.as_object_mut(), changes.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    account
        .update_settings(serde_json::json!({ "reminderSettings": reminders }))
        .await?;
    Ok(())
}

/// What a task's timer is doing, running or not.
fn timer_state(app: &App, task: &crate::model::Task) -> crate::services::timer::TimerState {
    let started = app
        .store
        .metadata(&crate::services::timer::started_key(&task.id))
        .ok()
        .flatten()
        .and_then(|stamp| crate::model::date::parse(&stamp));
    crate::services::timer::TimerState {
        is_running: started.is_some(),
        started_at: started,
        logged_minutes: task.timer_duration.unwrap_or(0),
        last_value: task.last_timer_value.clone(),
    }
}

/// Start timing a task.
///
/// Starting one that is already running keeps the original start rather than resetting it: two
/// clicks on a button should not quietly discard the first ten minutes.
fn start_timer(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let key = crate::services::timer::started_key(task_id);
    if app.store.metadata(&key).ok().flatten().is_none() {
        let now = crate::model::date::format(app.clock.now());
        if let Err(error) = app.store.set_metadata(&key, &now) {
            return Response::failed(error.into());
        }
    }
    Response::ok(timer_state(app, &task))
}

/// Stop timing, and add what the session was worth to the task.
fn stop_timer(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let key = crate::services::timer::started_key(task_id);
    let started = app
        .store
        .metadata(&key)
        .ok()
        .flatten()
        .and_then(|stamp| crate::model::date::parse(&stamp));
    let Some(started) = started else {
        // Nothing was running. Not an error: two clicks on Stop is an ordinary thing to do.
        return Response::ok(timer_state(app, &task));
    };

    let minutes = crate::services::timer::minutes_between(started, app.clock.now());
    // Empty rather than deleted: the store keeps metadata by key, and an empty value reads as "not
    // running" everywhere it is looked at — `date::parse` answers None for it.
    if let Err(error) = app.store.set_metadata(&key, "") {
        return Response::failed(error.into());
    }

    if minutes == 0 {
        return Response::ok(timer_state(app, &task));
    }
    let changes = crate::services::TaskChanges {
        timer_duration: Some(Some(task.timer_duration.unwrap_or(0) + minutes)),
        last_timer_value: Some(Some(crate::services::timer::last_value(minutes))),
        ..Default::default()
    };
    match app.context.tasks().update(task_id, &changes) {
        Ok(task) => Response::ok(timer_state(app, &task)),
        Err(error) => Response::failed(error.into()),
    }
}

/// The files on a task, and whether each is already on this machine.
fn attachments(app: &App, task_id: &str) -> Response {
    let service = app.context.attachments(app.attachment_cache());
    match service.for_task(task_id) {
        Ok(files) => Response::ok(serde_json::json!({
            "files": files
                .iter()
                .map(|file| serde_json::json!({
                    "id": file.id,
                    "name": file.name,
                    "size": file.size,
                    "mimeType": file.mime_type,
                    // So the shell can offer "Open" rather than "Download" for one already here.
                    "isCached": service.is_cached(file),
                    "path": service.cached_path(file).to_string_lossy(),
                }))
                .collect::<Vec<_>>(),
        })),
        Err(error) => Response::failed(error.into()),
    }
}

/// Fetch one file and say where it landed.
async fn download_attachment(app: &App, task_id: &str, file_id: &str) -> Response {
    let service = app.context.attachments(app.attachment_cache());
    let file = match service.for_task(task_id) {
        Ok(files) => files.into_iter().find(|file| file.id == file_id),
        Err(error) => return Response::failed(error.into()),
    };
    let Some(file) = file else {
        return Response::failed(Failure::not_found("attachment", file_id));
    };
    match service.download(&file).await {
        Ok(path) => Response::ok(serde_json::json!({ "path": path.to_string_lossy() })),
        Err(error) => Response::failed(error.into()),
    }
}

/// Upload a file and post the comment that carries it.
///
/// One command rather than two, because a file uploaded with no comment naming it is a file nobody
/// can reach: it exists on the server and appears on no task.
async fn attach_file(app: &App, task_id: &str, path: &str, content: Option<&str>) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    // The list decides who may read the file afterwards, so the server is told which one.
    let list_id = task.effective_list_ids().into_iter().next();
    let context = serde_json::json!({ "listId": list_id });

    let service = app.context.attachments(app.attachment_cache());
    let uploaded = match service.upload(std::path::Path::new(path), context).await {
        Ok(file) => file,
        Err(error) => return Response::failed(error.into()),
    };

    let author = app.context.account().current_user_id().ok().flatten();
    answer(app.context.comments().post(
        task_id,
        content.unwrap_or_default(),
        author.as_deref(),
        crate::model::CommentType::Attachment,
        Some(&uploaded),
    ))
}

/// What a list is filtered and sorted by, and what else it could be.
///
/// The rules are `crate::filters`; this is the sheet. Every value here is one those rules match on
/// — they are saved on the list and read by every client, so a value spelled differently would be
/// a filter the others keep and this one silently ignores.
fn filter_options(app: &App, list_id: &str) -> Response {
    let list = match app.context.lists().list(list_id) {
        Ok(Some(list)) => list,
        Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
        Err(error) => return Response::failed(error.into()),
    };
    Response::ok(serde_json::json!({
        "listId": list.id,
        "isFiltered": rows::filter_picks::is_filtered(&list),
        "groups": rows::filter_picks::groups(&list),
    }))
}

/// A list's chat, from the cache.
///
/// `channelId` is null when this deployment has no channel for the list — chat is a feature a
/// deployment can be without, and a shell that read that as an error would show a broken panel to
/// everybody using a server that simply does not have it.
fn chat(app: &App, list_id: &str) -> Response {
    let channel = match app.context.chat().channel_for_list(list_id) {
        Ok(channel) => channel,
        Err(error) => return Response::failed(error.into()),
    };
    let Some(channel) = channel else {
        return Response::ok(serde_json::json!({
            "channelId": serde_json::Value::Null,
            "messages": [],
        }));
    };

    let messages = app.context.chat().messages(&channel.id).unwrap_or_default();
    let me = app.context.account().current_user_id().ok().flatten();
    let people = app.store.users().unwrap_or_default();

    Response::ok(serde_json::json!({
        "channelId": channel.id,
        "name": channel.name,
        "messages": rows::chat::transcript(&messages, me.as_deref(), &people),
    }))
}

/// Catch the chat up with the server.
///
/// The channels first: a list whose channel this client has never seen has nothing to fetch
/// messages for, and that is the ordinary state the first time a conversation is opened.
async fn refresh_chat(app: &App, list_id: &str) -> Response {
    if let Err(error) = app.context.chat().refresh_channels().await {
        return Response::failed(error.into());
    }
    let channel = match app.context.chat().channel_for_list(list_id) {
        Ok(Some(channel)) => channel,
        Ok(None) => {
            return Response::ok(serde_json::json!({
                "channelId": serde_json::Value::Null,
                "messages": [],
            }))
        }
        Err(error) => return Response::failed(error.into()),
    };
    if let Err(error) = app.context.chat().refresh_messages(&channel.id).await {
        return Response::failed(error.into());
    }
    chat(app, list_id)
}

/// Who a list is shared with, and what this account may do about it.
///
/// The permissions come back with the members rather than being worked out in the shell: whether
/// somebody may change a role is [`crate::permissions`], and a screen that decided it itself would
/// be the fourth implementation of a rule whose failure mode is a control that 403s — or one that
/// quietly is not offered to somebody who should have it.
async fn list_members(app: &App, list_id: &str) -> Response {
    let list = match app.context.lists().list(list_id) {
        Ok(Some(list)) => list,
        Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let members = match app.context.lists().members(list_id).await {
        Ok(members) => members,
        Err(error) => return Response::failed(error.into()),
    };

    let me = app.context.account().current_user_id().ok().flatten();
    let lists = app.context.lists();
    let (can_manage_members, can_manage_list, can_delete) = match me.as_deref() {
        Some(me) => (
            lists.can_manage_members(me, &list),
            lists.can_manage(me, &list),
            lists.can_delete(me, &list),
        ),
        None => (false, false, false),
    };

    Response::ok(serde_json::json!({
        "listId": list.id,
        "name": list.name,
        "ownerId": list.owner_id,
        "canManageMembers": can_manage_members,
        "canManageList": can_manage_list,
        "canDeleteList": can_delete,
        // Leaving is for a list somebody else owns: an owner leaving their own list would strand
        // it, which is what deleting is for.
        "canLeave": me.is_some() && list.owner_id.as_deref() != me.as_deref(),
        "currentUserId": me,
        "members": members,
    }))
}

/// The board a list belongs to.
///
/// Answers with rows so a card draws like a row: the same due labels, the same leading control,
/// the same converters in the shell. The surface is `BoardCard`, which is what makes the leading
/// control open the assignee picker rather than complete the task — tapping a face on a card is
/// how you reassign it, and completing from a board is the Done column.
fn board(app: &App, list_id: &str, limit: Option<usize>) -> Response {
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
fn move_task_to_column(app: &App, task_id: &str, column_id: &str, list_id: &str) -> Response {
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
    let project = project_id.and_then(|id| {
        app.store
            .projects()
            .unwrap_or_default()
            .into_iter()
            .find(|project| project.id == id)
    });
    let columns = crate::board::columns(
        project
            .as_ref()
            .and_then(|project| project.custom_states.as_ref()),
    );
    let Some(target) = columns.iter().find(|column| column.id == column_id) else {
        return Response::failed(Failure::bad_request("that column is not on this board"));
    };

    let moved = crate::board::resolve_move(&task, target, &lists);

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

/// When to be reminded about one task.
fn reminder_options(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    Response::ok(serde_json::json!({
        "reminderTime": task.reminder_time.map(|at| at.to_rfc3339()),
        "picks": rows::reminder_picks::options(&task, app.clock.now()),
    }))
}

/// The Agent Hub in one answer: the agents, their modes, the credentials, and Copilot.
///
/// Each part is allowed to be missing. A deployment without Copilot answers 404 for it, and a hub
/// that refused to draw because one of four requests failed would be a screen nobody could use to
/// fix the thing that failed.
async fn agents(app: &App) -> Response {
    let service = app.context.agents();
    let modes = service
        .modes()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let credentials = service
        .credentials()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let copilot = service
        .copilot_status()
        .await
        .unwrap_or_else(|_| serde_json::json!({ "connected": false }));

    Response::ok(serde_json::json!({
        "agents": modes.get("agents").cloned().unwrap_or(serde_json::json!([])),
        "modes": modes.get("modes").cloned().unwrap_or(serde_json::json!({})),
        "credentials": credentials
            .get("services")
            .or_else(|| credentials.get("keys"))
            .cloned()
            .unwrap_or(serde_json::json!([])),
        "copilot": copilot,
    }))
}

/// Everything one list's external-sync panel needs.
///
/// Answers even when nothing is connected: "not connected" is the state the panel exists to show,
/// and a failure there would leave somebody looking at an error instead of a button.
async fn external_sync(app: &App, list_id: &str) -> Response {
    let external = app.context.external();
    let status = external
        .status()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));

    let mut providers = Vec::new();
    for provider in [
        crate::services::Provider::GoogleTasks,
        crate::services::Provider::GitHub,
    ] {
        let connected = status
            .get("integrations")
            .and_then(|value| value.as_array())
            .map(|integrations| {
                integrations.iter().any(|integration| {
                    integration.get("provider").and_then(|value| value.as_str())
                        == Some(provider.wire())
                })
            })
            .unwrap_or(false);

        // Only when connected: asking for somebody's task lists before they have said yes to the
        // provider is a request that can only 401.
        let (containers, links) = if connected {
            (
                external.containers(provider).await.unwrap_or_default().0,
                external.links(provider).await.unwrap_or_default(),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let linked = links
            .iter()
            .find(|link| link.astrid_list_id == list_id)
            .cloned();

        providers.push(serde_json::json!({
            "provider": provider,
            "connected": connected,
            "containers": containers,
            "link": linked,
        }));
    }

    Response::ok(serde_json::json!({ "listId": list_id, "providers": providers }))
}

/// One Google pass over every linked list.
async fn sync_external(app: &App) -> Response {
    let external = app.context.external();
    let links = match external.links(crate::services::Provider::GoogleTasks).await {
        Ok(links) => links,
        Err(error) => return Response::failed(error.into()),
    };

    let mut passes = Vec::new();
    for link in &links {
        match external.sync_google_link(link).await {
            Ok(report) => passes.push(serde_json::json!({ "linkId": link.id, "report": report })),
            // One list failing is not the others failing. A pass that stopped at the first error
            // would leave every list after it stale because one repository went away.
            Err(error) => passes.push(serde_json::json!({
                "linkId": link.id,
                "error": error.to_string(),
            })),
        }
    }
    Response::ok(serde_json::json!({ "passes": passes }))
}

/// Where "the tour has been seen" is remembered.
///
/// The cache, so it belongs to this installation: somebody who has used the app for a year on a
/// laptop still wants to be shown where the hotkey is the first time they open it on a desktop.
const TOUR_KEY: &str = "tour.seen";

/// The key a shown reminder is remembered under.
///
/// The value is the reminder's own time, not a flag: a snoozed reminder has a new time, so the
/// same task can ask again without the mark having to be cleared by whoever moved it.
fn shown_key(task_id: &str) -> String {
    format!("reminder.shown.{task_id}")
}

/// Reminders whose time has come and which have not been shown.
fn reminders_due(app: &App) -> Response {
    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let now = app.clock.now();
    let due = crate::reminders::due_now(&tasks, now, |id| {
        let Some(task) = tasks.iter().find(|task| task.id == id) else {
            return false;
        };
        let Some(at) = task.reminder_time else {
            return false;
        };
        app.store
            .metadata(&shown_key(id))
            .ok()
            .flatten()
            .is_some_and(|stamp| stamp == at.to_rfc3339())
    });
    Response::ok(serde_json::json!({ "reminders": due }))
}

fn mark_reminder_shown(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let Some(at) = task.reminder_time else {
        // Nothing to remember. Not an error: the reminder may have been cleared between the
        // banner going up and somebody dismissing it.
        return Response::done();
    };
    match app
        .store
        .set_metadata(&shown_key(task_id), &at.to_rfc3339())
    {
        Ok(()) => Response::done(),
        Err(error) => Response::failed(error.into()),
    }
}

/// Move a reminder forward.
///
/// A write rather than a timer: an in-memory snooze is lost on a restart, and it leaves the
/// server's copy of the reminder where it was, so the push still arrives at the original time.
fn snooze_reminder(app: &App, task_id: &str, minutes: i64) -> Response {
    let when = crate::reminders::snooze_until(app.clock.now(), minutes);
    let changes = crate::services::TaskChanges {
        reminder_time: Some(Some(when)),
        ..Default::default()
    };
    match app.context.tasks().update(task_id, &changes) {
        Ok(task) => Response::ok(task),
        Err(error) => Response::failed(error.into()),
    }
}

/// The repeat presets and this task's own repeat, described.
///
/// Setting one is an ordinary `updateTask` carrying `repeating`, `repeatFrom` and
/// `repeatingData` — the same three fields the API takes — so there is no separate write.
fn repeat_options(app: &App, task_id: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };

    Response::ok(serde_json::json!({
        "repeating": task.repeating,
        "repeatFrom": task.repeat_from,
        "pattern": task.repeating_data,
        "presets": rows::repeat::presets(task.repeating),
        "summary": rows::repeat::summary(
            task.repeating,
            task.repeating_data.as_ref(),
            task.repeat_from,
        ),
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

/// For a write whose answer is that it happened. `Response::done()` rather than `ok(())`, so the
/// shell is not handed a `null` to decide something about.
fn answer_done(result: crate::services::Result<()>) -> Response {
    match result {
        Ok(()) => Response::done(),
        Err(error) => Response::failed(error.into()),
    }
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

    // Borrowed the whole way down. A list of ten thousand is read once and then referred to: the
    // owned versions of these would copy every task twice per refresh, and a refresh happens every
    // time anybody touches anything in the list.
    let filtered = filters::filter_refs(
        &tasks,
        &list,
        current_user_id.as_deref(),
        now,
        offset_from_utc,
    );

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
            "reminderTime" => changes.reminder_time = clearable_date(value),
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
            "filterPriority" => changes.filter_priority = Some(value.as_str().map(str::to_string)),
            "filterDueDate" => changes.filter_due_date = Some(value.as_str().map(str::to_string)),
            "filterAssignee" => changes.filter_assignee = Some(value.as_str().map(str::to_string)),
            "filterRepeating" => {
                changes.filter_repeating = Some(value.as_str().map(str::to_string))
            }
            "filterAssignedBy" => {
                changes.filter_assigned_by = Some(value.as_str().map(str::to_string))
            }
            "filterInLists" => changes.filter_in_lists = Some(value.as_str().map(str::to_string)),
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

    /// A hub that refused to draw because one of its four requests failed would be a screen
    /// nobody could use to fix the thing that failed.
    #[tokio::test]
    async fn the_agent_hub_draws_even_when_parts_of_it_are_missing() {
        let app = app_with(StubTransport::new());

        let hub = call(&app, json!({ "kind": "agents" })).await;
        assert_eq!(hub["ok"], true);
        assert!(hub["value"]["agents"]
            .as_array()
            .expect("agents")
            .is_empty());
        assert_eq!(hub["value"]["copilot"]["connected"], false);
    }

    /// Nothing connected is a state the panel exists to show, not an error to report.
    #[tokio::test]
    async fn the_external_panel_answers_even_with_nothing_connected() {
        let app = app_with(StubTransport::new());
        let made = call(&app, json!({ "kind": "createList", "name": "Work" })).await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let panel = call(&app, json!({ "kind": "externalSync", "listId": id })).await;
        assert_eq!(panel["ok"], true);
        let providers = panel["value"]["providers"].as_array().expect("providers");
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0]["connected"], false);
        assert!(providers[0]["containers"]
            .as_array()
            .expect("containers")
            .is_empty());
    }

    /// The panel reads "connected" by matching the server's own provider name, so a name we
    /// invented shows a connected account as disconnected and offers a Connect button instead.
    #[tokio::test]
    async fn a_connected_provider_reads_as_connected_under_the_servers_name_for_it() {
        let app = app_with(
            StubTransport::new()
                .push_json(
                    "/api/v1/integrations",
                    200,
                    json!({ "integrations": [{ "provider": "GITHUB_ISSUES" }] }),
                )
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );
        let made = call(&app, json!({ "kind": "createList", "name": "Work" })).await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let panel = call(&app, json!({ "kind": "externalSync", "listId": id })).await;
        let providers = panel["value"]["providers"].as_array().expect("providers");
        let github = providers
            .iter()
            .find(|provider| provider["provider"] == "git_hub")
            .expect("GitHub is one of the two");
        assert_eq!(github["connected"], true);
    }

    /// A pull applies what came back and commits the cursor only after it has — a client killed
    /// mid-pass re-pulls rather than skipping what it never wrote down.""
    #[tokio::test]
    async fn a_google_pass_applies_what_it_pulled_and_then_commits() {
        let transport = StubTransport::new()
            .push_json(
                "/sync/google/links",
                200,
                json!({
                    "links": [{
                        "id": "link-1",
                        "astridListId": "l1",
                        "remoteContainerId": "tasklist-1",
                    }],
                }),
            )
            .push_json(
                "/sync/google/tasks?",
                200,
                json!({
                    "items": [{
                        "remoteId": "tasklist-1:abc",
                        "title": "Buy oat milk",
                        "completed": false,
                        "dueDate": "2026-09-20T00:00:00Z",
                    }],
                    "cursor": "2026-09-07T12:00:00Z",
                }),
            )
            .push_json("/sync/google/task-links", 200, json!({ "taskLinks": [] }))
            .fallback(Ok(crate::api::transport::HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            }));
        let app = app_with(transport);
        app.store
            .upsert_list(&crate::model::TaskList::new("l1", "Work"))
            .expect("stores");

        let answered = call(&app, json!({ "kind": "syncExternal" })).await;
        let passes = answered["value"]["passes"].as_array().expect("passes");
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0]["report"]["applied"], 1);

        // And the pulled task is in the list it was linked to.
        let rows = call(&app, json!({ "kind": "rowsForList", "listId": "l1" })).await;
        assert_eq!(rows["value"]["rows"][0]["title"], "Buy oat milk");
    }

    /// Once. A tour that came back every launch would be the first thing anybody turned off.
    #[tokio::test]
    async fn the_tour_is_shown_once() {
        let app = app_with(StubTransport::new());
        assert_eq!(
            call(&app, json!({ "kind": "hasSeenTour" })).await["value"]["seen"],
            false
        );

        call(&app, json!({ "kind": "tourSeen" })).await;
        assert_eq!(
            call(&app, json!({ "kind": "hasSeenTour" })).await["value"]["seen"],
            true
        );
    }

    /// The palette answers from the cache, ranked, with commands first — so the keyboard action
    /// for "new task" is reachable from the box that exists to reach things.
    #[tokio::test]
    async fn the_palette_finds_commands_lists_and_tasks() {
        let app = app_with(StubTransport::new());
        call(&app, json!({ "kind": "createList", "name": "Home" })).await;
        call(
            &app,
            json!({ "kind": "createTask", "title": "Buy oat milk" }),
        )
        .await;

        let found = call(&app, json!({ "kind": "palette", "query": "milk" })).await;
        let rows = found["value"]["rows"].as_array().expect("rows");
        assert!(rows.iter().any(|row| row["title"] == "Buy oat milk"));

        // Fuzzy, and in order: "hm" finds "Home" by its letters.
        let fuzzy = call(&app, json!({ "kind": "palette", "query": "hm" })).await;
        assert!(fuzzy["value"]["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .any(|row| row["title"] == "Home"));
    }

    /// The settings screen draws from the cache and offers the same reminder offsets the per-task
    /// picker does, so "15 minutes before" means one thing in this app rather than two.
    #[tokio::test]
    async fn the_settings_screen_reads_from_the_cache() {
        let app = app_with(StubTransport::new());
        app.store
            .set_metadata(
                "account.settings",
                r#"{"reminderSettings":{"enablePushReminders":true,"defaultReminderTime":15}}"#,
            )
            .expect("stores");

        let answered = call(&app, json!({ "kind": "settings" })).await;
        assert_eq!(
            answered["value"]["reminderSettings"]["enablePushReminders"],
            true
        );
        let offsets = answered["value"]["offsets"].as_array().expect("offsets");
        assert_eq!(offsets[0]["titleKey"], "reminder.at_due_time");
        assert!(offsets.iter().any(|offset| offset["minutes"] == 15));
    }

    /// One toggle at a time: a screen sending a single field must not clear the rest.
    #[tokio::test]
    async fn changing_one_reminder_setting_keeps_the_others() {
        let transport = StubTransport::new().push_json("/settings", 200, json!({ "ok": true }));
        let app = app_with(transport);
        app.store
            .set_metadata(
                "account.settings",
                r#"{"reminderSettings":{"enablePushReminders":true,"enableEmailReminders":true}}"#,
            )
            .expect("stores");

        let answered = call(
            &app,
            json!({
                "kind": "updateReminderSettings",
                "changes": { "enablePushReminders": false },
            }),
        )
        .await;
        assert_eq!(
            answered["value"]["reminderSettings"]["enablePushReminders"],
            false
        );
        assert_eq!(
            answered["value"]["reminderSettings"]["enableEmailReminders"], true,
            "the setting nobody touched is still there"
        );
    }

    /// A timer survives a restart, because the start time is in the cache rather than in memory —
    /// which is the one thing this does that Apple's does not.
    #[tokio::test]
    async fn a_timer_records_what_the_session_was_worth() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Write it up" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let started = call(&app, json!({ "kind": "startTimer", "taskId": id })).await;
        assert_eq!(started["value"]["isRunning"], true);

        // The clock is fixed in these tests, so the session is zero minutes and records nothing —
        // which is itself the rule: a timer that never ran did not do any work.
        let stopped = call(&app, json!({ "kind": "stopTimer", "taskId": id })).await;
        assert_eq!(stopped["value"]["isRunning"], false);
        assert_eq!(stopped["value"]["loggedMinutes"], 0);

        // And the detail screen carries the state, so the section knows whether to draw.
        let detail = call(&app, json!({ "kind": "taskDetail", "taskId": id })).await;
        assert_eq!(detail["value"]["timer"]["isRunning"], false);
    }

    /// Two clicks on Start should not discard the first ten minutes.
    #[tokio::test]
    async fn starting_a_timer_that_is_already_running_keeps_the_original_start() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Write it up" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let first = call(&app, json!({ "kind": "startTimer", "taskId": id })).await;
        let again = call(&app, json!({ "kind": "startTimer", "taskId": id })).await;
        assert_eq!(first["value"]["startedAt"], again["value"]["startedAt"]);
    }

    /// Stopping one that is not running is an ordinary thing to do, not an error.
    #[tokio::test]
    async fn stopping_a_timer_that_never_started_says_so_quietly() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Write it up" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let stopped = call(&app, json!({ "kind": "stopTimer", "taskId": id })).await;
        assert_eq!(stopped["ok"], true);
        assert_eq!(stopped["value"]["isRunning"], false);
    }

    /// A file reaches a task through a comment — there is no attach-to-task endpoint anywhere —
    /// so the attachments on a task are its own files plus its comments'.
    #[tokio::test]
    async fn attachments_are_gathered_from_the_task_and_its_comments() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Plan the trip" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        // A comment carrying a file, as one arrives from the server.
        app.store
            .upsert_comments(&[serde_json::from_value(json!({
                "id": "c1",
                "taskId": id,
                "content": "the itinerary",
                "secureFiles": [{
                    "id": "f1",
                    "originalName": "itinerary.pdf",
                    "fileSize": 1024,
                    "mimeType": "application/pdf",
                }],
            }))
            .expect("a comment")])
            .expect("stores");

        let found = call(&app, json!({ "kind": "attachments", "taskId": id })).await;
        let files = found["value"]["files"].as_array().expect("files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["name"], "itinerary.pdf");
        // Not downloaded yet, so the shell offers to fetch it rather than to open it.
        assert_eq!(files[0]["isCached"], false);
    }

    #[tokio::test]
    async fn downloading_a_file_that_is_not_on_the_task_is_a_not_found() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Plan the trip" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let answered = call(
            &app,
            json!({ "kind": "downloadAttachment", "taskId": id, "fileId": "nope" }),
        )
        .await;
        assert_eq!(answered["error"]["kind"], "notFound");
    }

    /// The sheet offers the values the rules match on, and setting one is an ordinary list edit —
    /// which is what makes a filter set here mean the same thing on web.
    #[tokio::test]
    async fn a_filter_can_be_read_and_then_set() {
        let app = app_with(StubTransport::new());
        let made = call(&app, json!({ "kind": "createList", "name": "Work" })).await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let offered = call(&app, json!({ "kind": "filterOptions", "listId": id })).await;
        assert_eq!(offered["value"]["isFiltered"], false);
        let groups = offered["value"]["groups"].as_array().expect("groups");
        let due = groups
            .iter()
            .find(|group| group["field"] == "filterDueDate")
            .expect("a due-date group");
        assert_eq!(due["picks"][0]["value"], "all");
        assert_eq!(due["picks"][0]["isSelected"], true);

        call(
            &app,
            json!({
                "kind": "updateList",
                "listId": id,
                "changes": { "filterDueDate": "today" },
            }),
        )
        .await;

        let again = call(&app, json!({ "kind": "filterOptions", "listId": id })).await;
        assert_eq!(again["value"]["isFiltered"], true);
        let due = again["value"]["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .find(|group| group["field"] == "filterDueDate")
            .expect("a due-date group")
            .clone();
        let selected: Vec<&str> = due["picks"]
            .as_array()
            .expect("picks")
            .iter()
            .filter(|pick| pick["isSelected"] == true)
            .map(|pick| pick["value"].as_str().expect("a value"))
            .collect();
        assert_eq!(selected, vec!["today"]);
    }

    /// A message typed offline is in the transcript at once, marked as still going. The panel is
    /// drawn from the cache and catches up afterwards, the same order the task list uses.
    #[tokio::test]
    async fn a_message_is_in_the_transcript_before_it_is_sent() {
        let app = app_with(StubTransport::new());
        app.store
            .upsert_channels(&[serde_json::from_value(json!({
                "id": "c1", "listId": "l1", "name": "Work"
            }))
            .expect("a channel")])
            .expect("stores");
        app.store
            .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
            .expect("stores");

        call(
            &app,
            json!({ "kind": "sendChatMessage", "channelId": "c1", "content": "on my way" }),
        )
        .await;

        let panel = call(&app, json!({ "kind": "chat", "listId": "l1" })).await;
        let messages = panel["value"]["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "on my way");
        assert_eq!(messages[0]["isMine"], true);
        assert_eq!(messages[0]["isPending"], true);
    }

    /// Chat is a feature a deployment can be without. A shell that read "no channel" as an error
    /// would show a broken panel to everybody on a server that simply does not have it.
    #[tokio::test]
    async fn a_list_with_no_channel_has_an_empty_chat_rather_than_an_error() {
        let app = app_with(StubTransport::new());
        let panel = call(&app, json!({ "kind": "chat", "listId": "l1" })).await;
        assert_eq!(panel["ok"], true);
        assert!(panel["value"]["channelId"].is_null());
        assert!(panel["value"]["messages"]
            .as_array()
            .expect("messages")
            .is_empty());
    }

    /// Membership comes with what this account may do about it, decided by the permission rules
    /// rather than by a screen — the failure mode is a control that 403s, or one that quietly is
    /// not offered to somebody who should have it.
    #[tokio::test]
    async fn list_members_come_with_what_this_account_may_do() {
        let transport = StubTransport::new().push_json(
            "/members",
            200,
            json!({
                "members": [
                    { "userId": "me", "role": "owner", "user": { "id": "me", "name": "Jon" } },
                    { "userId": "dana", "role": "member", "user": { "id": "dana", "name": "Dana" } },
                ]
            }),
        );
        let app = app_with(transport);
        app.store
            .upsert_list(
                &serde_json::from_value(json!({
                    "id": "l1", "name": "Work", "ownerId": "me", "privacy": "SHARED"
                }))
                .expect("a list"),
            )
            .expect("stores");
        app.store
            .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
            .expect("stores");

        let answered = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
        assert_eq!(
            answered["value"]["members"]
                .as_array()
                .expect("members")
                .len(),
            2
        );
        assert_eq!(answered["value"]["canManageMembers"], true);
        // The owner cannot leave their own list — that would strand it, which is what deleting is
        // for.
        assert_eq!(answered["value"]["canLeave"], false);
    }

    #[tokio::test]
    async fn members_of_a_list_that_is_not_there_are_a_not_found() {
        let app = app_with(StubTransport::new());
        let answered = call(&app, json!({ "kind": "listMembers", "listId": "nope" })).await;
        assert_eq!(answered["error"]["kind"], "notFound");
    }

    /// An invitation reaches the network rather than the Outbox: queuing one offline would show a
    /// member who does not exist, and the optimistic row would be indistinguishable from a real one
    /// to every permission check that read it afterwards.
    #[tokio::test]
    async fn an_invitation_goes_straight_to_the_server() {
        let transport = StubTransport::new().push_json("/members", 200, json!({ "ok": true }));
        let app = app_with(transport);

        let answered = call(
            &app,
            json!({
                "kind": "inviteToList",
                "listId": "l1",
                "email": "dana@example.test",
                "role": "member",
            }),
        )
        .await;
        assert_eq!(answered["ok"], true);
        assert_eq!(
            call(&app, json!({ "kind": "outboxStats" })).await["value"]["pending"],
            0,
            "an invitation is not a local fact and does not belong in the Outbox"
        );
    }

    /// A board answers with rows, so a card draws like a row, and the columns come back in the
    /// order the board shows them.
    #[tokio::test]
    async fn a_board_answers_with_its_columns_and_their_cards() {
        let app = app_with(StubTransport::new());
        let project = serde_json::json!({ "id": "p1", "name": "Ship it" });
        app.store
            .upsert_projects(&[serde_json::from_value(project).expect("a project")])
            .expect("stores");
        app.store
            .upsert_list(
                &serde_json::from_value(serde_json::json!({
                    "id": "l1", "name": "Work", "projectId": "p1"
                }))
                .expect("a list"),
            )
            .expect("stores");
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Write it down", "listIds": ["l1"] }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
        let columns = board["value"]["columns"].as_array().expect("columns");
        assert_eq!(columns[0]["id"], "__virtual_inbox__");
        assert_eq!(columns.last().expect("done")["id"], "__virtual_done__");
        // A new card with no role is in the Inbox, and it arrives as a row.
        assert_eq!(columns[0]["total"], 1);
        assert_eq!(columns[0]["cards"][0]["title"], "Write it down");
        assert!(columns[0]["cards"][0]["leading"].is_object());

        // Moving it carries the role, and the card lands in that column.
        call(
            &app,
            json!({
                "kind": "moveTaskToColumn",
                "taskId": id,
                "columnId": "doing",
                "listId": "l1",
            }),
        )
        .await;
        let board = call(&app, json!({ "kind": "board", "listId": "l1" })).await;
        let columns = board["value"]["columns"].as_array().expect("columns");
        let doing = columns
            .iter()
            .find(|column| column["id"] == "doing")
            .expect("a doing column");
        assert_eq!(doing["total"], 1);
        assert_eq!(columns[0]["total"], 0);
    }

    /// Dragging a repeating card to Done rolls it forward like every other completion. Rule 2 does
    /// not stop applying because the gesture is a drag.
    #[tokio::test]
    async fn a_repeating_card_dragged_to_done_rolls_over() {
        let app = app_with(StubTransport::new());
        app.store
            .upsert_list(
                &serde_json::from_value(serde_json::json!({
                    "id": "l1", "name": "Work", "projectId": "p1"
                }))
                .expect("a list"),
            )
            .expect("stores");
        let made = call(
            &app,
            json!({
                "kind": "createTask",
                "title": "Water plants",
                "listIds": ["l1"],
                "dueDateTime": "2026-09-20T09:00:00Z",
            }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();
        call(
            &app,
            json!({
                "kind": "updateTask",
                "taskId": id,
                "changes": { "repeating": "daily" },
            }),
        )
        .await;

        call(
            &app,
            json!({
                "kind": "moveTaskToColumn",
                "taskId": id,
                "columnId": "__virtual_done__",
                "listId": "l1",
            }),
        )
        .await;

        let detail = call(&app, json!({ "kind": "taskDetail", "taskId": id })).await;
        assert_eq!(
            detail["value"]["task"]["completed"], false,
            "a repeating card rolls forward instead of finishing"
        );
        // A daily repeat counted from the completion date, which is what a task carries unless it
        // says otherwise: finished on the 7th, so it comes back on the 8th — at the time of day it
        // was already due, rather than the moment it happened to be ticked off.
        assert_eq!(
            detail["value"]["task"]["dueDateTime"],
            "2026-09-08T09:00:00Z"
        );
    }

    /// A list with no project has no board. Not an error: the shell asks before it knows.
    #[tokio::test]
    async fn a_list_with_no_project_has_no_board() {
        let app = app_with(StubTransport::new());
        let made = call(&app, json!({ "kind": "createList", "name": "Home" })).await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let board = call(&app, json!({ "kind": "board", "listId": id })).await;
        assert!(board["value"]["projectId"].is_null());
        assert!(board["value"]["columns"]
            .as_array()
            .expect("columns")
            .is_empty());
    }

    #[tokio::test]
    async fn a_column_that_is_not_on_this_board_is_refused() {
        let app = app_with(StubTransport::new());
        app.store
            .upsert_list(
                &serde_json::from_value(serde_json::json!({
                    "id": "l1", "name": "Work", "projectId": "p1"
                }))
                .expect("a list"),
            )
            .expect("stores");
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Write it down", "listIds": ["l1"] }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let answered = call(
            &app,
            json!({
                "kind": "moveTaskToColumn",
                "taskId": id,
                "columnId": "nowhere",
                "listId": "l1",
            }),
        )
        .await;
        assert_eq!(answered["error"]["kind"], "badRequest");
    }

    /// The picker offers offsets from the due time, because "an hour before" is what somebody
    /// means — and a task with nothing to be before can only have its reminder cleared.
    #[tokio::test]
    async fn reminder_options_are_offsets_from_the_due_time() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({
                "kind": "createTask",
                "title": "Call the vet",
                "dueDateTime": "2026-09-20T09:00:00Z",
            }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();

        let offered = call(&app, json!({ "kind": "reminderOptions", "taskId": id })).await;
        let picks = offered["value"]["picks"].as_array().expect("picks");
        assert_eq!(picks[0]["titleKey"], "reminder.none");
        assert!(picks.len() > 1);

        let bare = call(&app, json!({ "kind": "createTask", "title": "Someday" })).await;
        let bare_id = bare["value"]["id"].as_str().expect("an id").to_string();
        let none = call(
            &app,
            json!({ "kind": "reminderOptions", "taskId": bare_id }),
        )
        .await;
        assert_eq!(none["value"]["picks"].as_array().expect("picks").len(), 1);
    }

    /// A reminder is shown once. A banner that comes back every thirty seconds is one that gets
    /// dismissed without being read.
    #[tokio::test]
    async fn a_reminder_is_offered_once_and_again_after_a_snooze() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Call the vet" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();
        // A minute before the fixed clock, so it is inside the grace window.
        let when = "2026-09-07T11:59:00Z";
        call(
            &app,
            json!({
                "kind": "updateTask",
                "taskId": id,
                "changes": { "reminderTime": when },
            }),
        )
        .await;

        let due = call(&app, json!({ "kind": "remindersDue" })).await;
        assert_eq!(
            due["value"]["reminders"].as_array().expect("a list").len(),
            1
        );

        call(&app, json!({ "kind": "reminderShown", "taskId": id })).await;
        let again = call(&app, json!({ "kind": "remindersDue" })).await;
        assert!(again["value"]["reminders"]
            .as_array()
            .expect("a list")
            .is_empty());

        // Snoozing gives it a new time, so it may ask again — and not before it is due.
        call(
            &app,
            json!({ "kind": "snoozeReminder", "taskId": id, "minutes": 10 }),
        )
        .await;
        let snoozed = call(&app, json!({ "kind": "remindersDue" })).await;
        assert!(snoozed["value"]["reminders"]
            .as_array()
            .expect("a list")
            .is_empty());
    }

    /// A repeat describes itself in parts with resource keys, so the shell says it in its own
    /// words and in its own order.
    #[tokio::test]
    async fn repeat_options_describe_the_current_repeat() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Water plants" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();
        call(
            &app,
            json!({
                "kind": "updateTask",
                "taskId": id,
                "changes": { "repeating": "weekly" },
            }),
        )
        .await;

        let offered = call(&app, json!({ "kind": "repeatOptions", "taskId": id })).await;
        assert_eq!(offered["value"]["presets"][0]["value"], "never");
        assert_eq!(offered["value"]["summary"][0]["key"], "repeat.weekly");
        let selected = offered["value"]["presets"]
            .as_array()
            .expect("presets")
            .iter()
            .filter(|preset| preset["isSelected"] == true)
            .count();
        assert_eq!(selected, 1);
    }

    /// The detail screen carries the same sentence the picker shows, from the same function.
    #[tokio::test]
    async fn the_detail_screen_describes_the_repeat_the_same_way() {
        let app = app_with(StubTransport::new());
        let made = call(
            &app,
            json!({ "kind": "createTask", "title": "Water plants" }),
        )
        .await;
        let id = made["value"]["id"].as_str().expect("an id").to_string();
        call(
            &app,
            json!({
                "kind": "updateTask",
                "taskId": id,
                "changes": { "repeating": "daily" },
            }),
        )
        .await;

        let detail = call(&app, json!({ "kind": "taskDetail", "taskId": id })).await;
        assert_eq!(detail["value"]["repeatSummary"][0]["key"], "repeat.daily");
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
