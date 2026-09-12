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
        Command::DueDateOnDay { task_id, day } => due_date_on_day(app, &task_id, &day),
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
            quick_add,
            locale,
        } => {
            // What the caller said, before the fields move into the draft: a list's defaults
            // fill in only what was left unsaid (task c4102c67).
            let given = crate::services::list_defaults::Given {
                priority: priority.is_some(),
                due: due_date_time.is_some(),
                assignee: assignee_id.is_some(),
                repeating: false,
                is_private: false,
            };
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

            let lists = app.store.lists().unwrap_or_default();
            let mut given = given;
            // What the quick-add box reads out of a sentence when the account has smart parsing
            // on — `#list` tags, "tomorrow", "weekly mon and wed", "urgent" — by the web's rule,
            // in the reader's language, in the core (tasks 6ac2639a and CONTRACTS.md D11). A
            // person who turned it off on the web gets the same plain title here. The tagged
            // lists come first and the open list after, as the web orders them. A title that was
            // nothing but keywords keeps its words: a task named after its filing beats an
            // untitled one.
            if quick_add
                && app
                    .context
                    .account()
                    .smart_tasks()
                    .smart_task_creation_enabled
            {
                let keywords =
                    crate::parse::smart::Keywords::for_locale(locale.as_deref().unwrap_or("en"));
                let today = crate::filters::local_day(app.clock.now(), app.clock.utc_offset());
                let read = crate::parse::smart::parse(&draft.title, &lists, keywords, today);
                draft.title = read.title;
                if !read.list_ids.is_empty() {
                    let mut filed = read.list_ids;
                    for id in draft.list_ids.drain(..) {
                        if !filed.contains(&id) {
                            filed.push(id);
                        }
                    }
                    draft.list_ids = filed;
                }
                // A date word is a calendar day here, stored the way every all-day date is
                // (CONTRACTS.md D12). It counts as given, so a list's own default does not
                // overrule what the person typed.
                if let Some(day) = read.due_day {
                    if draft.due_date_time.is_none() {
                        draft.due_date_time = Some(date::all_day_instant(day));
                        draft.is_all_day = true;
                    }
                    given.due = true;
                }
                if let Some(priority) = read.priority {
                    draft.priority = crate::model::Priority::from_i64(priority);
                    given.priority = true;
                }
                if let Some(repeating) = read.repeating.as_deref() {
                    draft.repeating = Some(match repeating {
                        "daily" => crate::model::Repeating::Daily,
                        "weekly" => crate::model::Repeating::Weekly,
                        "monthly" => crate::model::Repeating::Monthly,
                        "yearly" => crate::model::Repeating::Yearly,
                        _ => crate::model::Repeating::Custom,
                    });
                    if repeating == "custom" {
                        draft.repeating_data = Some(crate::model::CustomRepeatingPattern {
                            r#type: Some("custom".into()),
                            unit: Some("weeks".into()),
                            interval: Some(1),
                            end_condition: Some("never".into()),
                            weekdays: Some(read.weekdays),
                            ..Default::default()
                        });
                    }
                    given.repeating = true;
                }
            }

            // The first of the task's lists the cache knows decides the defaults — a task filed
            // in two lists takes the first one's, as the web takes its target list's.
            if let Some(list) = draft
                .list_ids
                .iter()
                .find_map(|id| lists.iter().find(|list| &list.id == id))
            {
                crate::services::list_defaults::apply(
                    &mut draft,
                    given,
                    list,
                    app.clock.now(),
                    app.clock.utc_offset(),
                );
            }
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
        Command::SetClosedReason {
            task_id,
            closed_reason,
        } => {
            if let Some(reason) = closed_reason.as_deref() {
                if !crate::model::task::is_closed_reason(reason) {
                    return Response::failed(Failure::bad_request(format!(
                        "closedReason must be one of: {}",
                        crate::model::task::CLOSED_REASONS.join(", ")
                    )));
                }
            }
            answer(
                app.context
                    .tasks()
                    .close(&task_id, closed_reason.as_deref()),
            )
        }
        Command::TaskStatusOptions { task_id } => task_status_options(app, &task_id),
        Command::SetTaskStatus { task_id, column_id } => set_task_status(app, &task_id, &column_id),
        Command::ShareTask { task_id } => share_task(app, &task_id).await,
        Command::CopyTask {
            task_id,
            target_list_id,
            include_comments,
        } => answer(
            app.context
                .tasks()
                .copy(&task_id, target_list_id.as_deref(), include_comments)
                .await,
        ),
        Command::SetTaskLists { task_id, list_ids } => {
            answer(app.context.tasks().set_lists(&task_id, list_ids))
        }
        Command::ListPicks { task_id, query } => list_picks(app, &task_id, &query),
        Command::AddTaskToList { task_id, list_id } => change_task_lists(app, &task_id, |ids| {
            if !ids.contains(&list_id) {
                ids.push(list_id.clone());
            }
        }),
        Command::RemoveTaskFromList { task_id, list_id } => {
            change_task_lists(app, &task_id, |ids| ids.retain(|id| id != &list_id))
        }
        Command::CreateListForTask { task_id, name } => create_list_for_task(app, &task_id, &name),
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
        Command::ListAgentOptions { list_id } => list_agent_options(app, &list_id).await,
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
        Command::PostComment {
            task_id,
            content,
            parent_comment_id,
        } => {
            let author = app.context.account().current_user_id().ok().flatten();
            answer(app.context.comments().post_under(
                &task_id,
                &content,
                author.as_deref(),
                crate::model::CommentType::Text,
                None,
                parent_comment_id.as_deref(),
            ))
        }
        Command::EditComment {
            comment_id,
            content,
        } => match app.context.comments().edit(&comment_id, &content) {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },
        Command::CommentSuggestions {
            task_id,
            text,
            caret,
        } => comment_suggestions(app, &task_id, &text, caret),
        Command::ApplyCommentSuggestion {
            text,
            caret,
            trigger_kind,
            id,
            label,
        } => {
            let (text, caret) =
                crate::parse::mentions::insert(&text, caret, trigger_kind, &label, &id);
            Response::ok(serde_json::json!({ "text": text, "caret": caret }))
        }
        Command::DeleteComment { comment_id } => match app.context.comments().delete(&comment_id) {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },

        // ── The network ──────────────────────────────────────────────────────────────────────
        Command::Sync => {
            let report = app.sync.sync().await;
            // The flags ride on a pass that reached the server: what this person may see can
            // change while the app is open, and a board that appears on the next pass beats one
            // that appears on the next launch. Best effort, like the projects.
            if report.fetched {
                let _ = app.context.account().refresh_features().await;
            }
            Response::ok(serde_json::json!({
                "fetched": report.fetched,
                "skipped": report.skipped,
                "delta": report.delta,
                "listsAdded": report.lists_added,
                "listsUpdated": report.lists_updated,
                "listsDeleted": report.lists_deleted,
                "tasksAdded": report.tasks_added,
                "tasksUpdated": report.tasks_updated,
                "tasksDeleted": report.tasks_deleted,
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
            // Projected the same way the detail projects them, so a refresh cannot draw a comment
            // differently from the screen it lands in.
            let me = app.context.account().current_user_id().ok().flatten();
            match app.context.comments().refresh(&task_id).await {
                Ok(comments) => {
                    let mut rows = rows::comment::rows(&comments, me.as_deref());
                    fill_local_paths(app, &task_id, &mut rows);
                    Response::ok(rows)
                }
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::SearchUsers { query } => answer(app.context.account().search_users(&query).await),
        Command::Agents => agents(app).await,
        Command::WebhookSettings => answer(app.context.agents().webhook_settings().await),
        Command::SaveWebhook {
            url,
            enabled,
            events,
            agents,
            regenerate_secret,
        } => answer(
            app.context
                .agents()
                .save_webhook(&url, enabled, &events, &agents, regenerate_secret)
                .await,
        ),
        Command::DeleteWebhook => answer_done(app.context.agents().delete_webhook().await),
        Command::TestWebhook => answer(app.context.agents().test_webhook().await),
        Command::ApiAccess => answer(app.context.api_access().oauth_clients().await),
        Command::CreateMcpToken => match app.context.api_access().mcp_token().await {
            // Named rather than returned bare: the shell binds to a field, and a bare string would
            // make adding anything beside it a breaking change to every caller.
            Ok(token) => Response::ok(serde_json::json!({ "token": token })),
            Err(error) => Response::failed(error.into()),
        },
        Command::RevokeMcpTokens => answer_done(app.context.api_access().revoke_mcp_tokens().await),
        Command::CreateOAuthClient { name } => {
            answer(app.context.api_access().create_oauth_client(&name).await)
        }
        Command::DeleteOAuthClient { client_id } => answer_done(
            app.context
                .api_access()
                .delete_oauth_client(&client_id)
                .await,
        ),
        Command::CustomAgents => answer(app.context.agents().custom_agents().await),
        Command::RegisterCustomAgent { name, list_ids } => answer(
            app.context
                .agents()
                .register_custom_agent(&name, list_ids)
                .await,
        ),
        Command::DeleteCustomAgent { agent_id } => {
            answer_done(app.context.agents().delete_custom_agent(&agent_id).await)
        }
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
        Command::Features => features(app),
        Command::Hotkey => hotkey(app),
        Command::SetHotkey { chord } => match crate::keyboard::chord::parse(&chord) {
            Ok(parsed) => {
                match app
                    .store
                    .set_metadata(crate::keyboard::chord::KEY, &parsed.to_string())
                {
                    Ok(()) => hotkey(app),
                    Err(error) => Response::failed(error.into()),
                }
            }
            Err(reason) => Response::failed(Failure::bad_request(reason.to_string())),
        },
        Command::BeginEditing { editor } => {
            editing(app, |session| crate::editing::begin(session, &editor))
        }
        Command::EndEditing { editor } => {
            editing(app, |session| crate::editing::end(session, &editor))
        }
        Command::CancelEditing { editor } => {
            editing(app, |session| crate::editing::cancel(session, &editor))
        }
        Command::CommitAllEditing => editing(app, crate::editing::commit_all),
        Command::Theme => {
            let chosen = app
                .store
                .metadata(crate::theme::KEY)
                .ok()
                .flatten()
                .map(|value| crate::theme::Theme::parse(&value))
                .unwrap_or_default();
            Response::ok(serde_json::json!({
                "theme": chosen,
                "isDark": chosen.is_dark(),
                "choices": crate::theme::Theme::all()
                    .iter()
                    .map(|theme| theme.wire())
                    .collect::<Vec<_>>(),
            }))
        }
        Command::SetTheme { theme } => {
            match app.store.set_metadata(crate::theme::KEY, theme.wire()) {
                Ok(()) => Response::ok(serde_json::json!({
                    "theme": theme,
                    "isDark": theme.is_dark(),
                })),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::MyTasksList => Response::ok(app.context.lists().my_tasks()),
        Command::MyTasksFilters => match app.context.account().my_tasks_preferences() {
            Ok(filters) => Response::ok(filters),
            Err(error) => Response::failed(error.into()),
        },
        Command::RefreshMyTasks => {
            match app.context.account().refresh_my_tasks_preferences().await {
                Ok(filters) => Response::ok(filters),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::SetMyTasksFilters { filters } => {
            match app
                .context
                .account()
                .set_my_tasks_preferences(&filters)
                .await
            {
                Ok(filters) => Response::ok(filters),
                // The choice is already on screen and already remembered here. Saying so beats
                // pretending it worked, and beats losing it because the account could not hear.
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::GoogleSyncMode => match app.context.external().auto_link_settings().await {
            Ok(settings) => Response::ok(serde_json::json!({
                "mode": settings.mode,
                "suffix": settings.suffix,
            })),
            Err(error) => Response::failed(error.into()),
        },
        Command::SetGoogleSyncMode { mode, suffix } => answer_done(
            app.context
                .external()
                .set_auto_link_mode(mode, suffix.as_deref())
                .await,
        ),
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
        Command::UpdateProfile { name, photo_path } => {
            let account = app.context.account();
            let image = match photo_path {
                Some(path) => match account.upload_photo(std::path::Path::new(&path)).await {
                    Ok(url) => Some(url),
                    Err(error) => return Response::failed(error.into()),
                },
                None => None,
            };
            match account
                .update_profile(name.as_deref(), image.as_deref())
                .await
            {
                Ok(_) => settings(app),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::ResendVerification => match app.context.account().resend_verification().await {
            Ok(answer) => Response::ok(serde_json::json!({
                "message": answer.get("message").cloned().unwrap_or(serde_json::Value::Null),
            })),
            Err(error) => Response::failed(error.into()),
        },
        Command::Passkeys => match app.context.account().passkeys().await {
            Ok(passkeys) => Response::ok(serde_json::json!({ "passkeys": passkeys })),
            Err(error) => Response::failed(error.into()),
        },
        Command::RenamePasskey { id, name } => {
            if name.trim().is_empty() {
                return Response::failed(Failure::bad_request("a passkey needs a name"));
            }
            match app.context.account().rename_passkey(&id, &name).await {
                Ok(()) => Response::done(),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::RevokePasskey { id } => match app.context.account().revoke_passkey(&id).await {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },
        Command::Contacts => match app.context.account().contacts().await {
            Ok((contacts, total)) => {
                Response::ok(serde_json::json!({ "contacts": contacts, "total": total }))
            }
            Err(error) => Response::failed(error.into()),
        },
        Command::ClearContacts => match app.context.account().clear_contacts().await {
            Ok(deleted) => Response::ok(serde_json::json!({ "deleted": deleted })),
            Err(error) => Response::failed(error.into()),
        },
        Command::DeleteAccount { confirmation } => {
            // The server requires the phrase typed exactly, and so does this: a request that is
            // going to be refused is not worth sending, and a button that sends one looks like it
            // worked.
            if confirmation != crate::services::account::DELETE_CONFIRMATION {
                return Response::failed(Failure::bad_request(format!(
                    "type {} exactly to delete the account",
                    crate::services::account::DELETE_CONFIRMATION
                )));
            }
            if let Err(error) = app.context.account().delete_account(&confirmation).await {
                return Response::failed(error.into());
            }
            // The account is gone; so is everything this machine held for it.
            sign_out(app).await
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
            // Best effort, like the user: a server without the route leaves the defaults standing.
            let _ = app.context.account().refresh_smart_task_settings().await;
            let _ = app.context.account().refresh_features().await;
            match app.context.account().refresh_settings().await {
                Ok(_) => settings(app),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::UpdateReminderSettings { changes } => {
            match update_reminder_settings(app, changes) {
                Ok(()) => settings(app),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::UpdateSmartTaskSettings { changes } => {
            if let Err(reason) = crate::smart_tasks::validate(&changes) {
                return Response::failed(Failure::bad_request(reason));
            }
            match app.context.account().update_smart_task_settings(changes) {
                Ok(_) => settings(app),
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
        } => attach_file(app, &task_id, &path, content.as_deref()),
        Command::ClipboardPaste {
            files,
            image_extension,
            has_text,
        } => clipboard_paste(app, files, image_extension, has_text),
        Command::FilterOptions { list_id } => filter_options(app, &list_id),
        Command::SetFilter {
            list_id,
            field,
            value,
        } => set_filter(app, &list_id, &field, &value).await,
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
        Command::ListMembers { list_id } => list_members(app, &list_id),
        Command::RefreshListMembers { list_id } => {
            match app.context.lists().members(&list_id).await {
                Ok(_) => list_members(app, &list_id),
                Err(error) => Response::failed(error.into()),
            }
        }
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
        Command::Notifications => inbox_response(app.context.notifications().inbox()),
        Command::RefreshNotifications => {
            inbox_response(app.context.notifications().refresh().await)
        }
        Command::MarkNotificationsRead { ids } => {
            inbox_response(app.context.notifications().mark_read(&ids).await)
        }
        Command::MarkAllNotificationsRead => {
            inbox_response(app.context.notifications().mark_all_read().await)
        }
        Command::BeginSignIn => match app.auth.begin() {
            Ok(url) => Response::ok(serde_json::json!({ "authorizeUrl": url })),
            Err(error) => Response::failed(error.into()),
        },
        Command::CompleteSignIn { callback_url } => answer(app.auth.complete(&callback_url).await),
        Command::CancelSignIn => {
            app.auth.cancel();
            Response::done()
        }
        Command::SignOut => sign_out(app).await,
    }
}

/// Where "the tour has been seen" is remembered.
///
/// The cache, so it belongs to this installation: somebody who has used the app for a year on a
/// laptop still wants to be shown where the hotkey is the first time they open it on a desktop.
const TOUR_KEY: &str = "tour.seen";

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

// The helpers, by domain. Glob-imported so an arm above names a helper the way it always
// has, and so the domain files can reach each other through `use super::*`.
mod account;
mod attachments;
mod board;
mod chat;
mod comments;
mod list_filters;
mod lists;
mod reminders;
mod row_json;
mod tasks;
/// Step the one editing session and answer with what the shell must do (task e71ed760).
///
/// The machine is pure ([`crate::editing`]); this is where its session lives between calls. A
/// poisoned lock is recovered rather than propagated: the session is one small value, and a
/// panic elsewhere must not leave every editor in the window unable to open.
fn editing(
    app: &App,
    step: impl FnOnce(&crate::editing::Session) -> crate::editing::Transition,
) -> Response {
    let mut session = app
        .editing
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let transition = step(&session);
    *session = transition.session.clone();
    Response::ok(serde_json::json!({
        "active": transition.session.active,
        "commit": transition.commit,
        "cancel": transition.cancel,
    }))
}

use account::*;
use attachments::*;
use board::*;
use chat::*;
use comments::*;
use list_filters::*;
use lists::*;
use reminders::*;
use row_json::*;
use tasks::*;

#[cfg(test)]
mod tests;
