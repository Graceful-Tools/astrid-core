//! The account: settings, sign-out, the agents and the external providers.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Every command about the account, its agents or its integrations, handed here whole by [`super::run`]
/// (task 8a91c5b0).
///
/// Every arm is short on purpose: it names a service call, and the service decides. An arm that
/// grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file nobody
/// would look in for it.
pub(super) async fn run(app: &App, command: Command) -> Response {
    match command {
        Command::IsSignedIn => Response::ok(serde_json::json!({
            "signedIn": app.auth.is_signed_in().await,
            "waitingForCallback": app.auth.is_waiting(),
        })),
        Command::CurrentUser => match app.context.account().current_user() {
            Ok(user) => Response::ok(user),
            Err(error) => Response::failed(error.into()),
        },
        Command::SearchUsers {
            query,
            task_id,
            list_ids,
            include_agents,
        } => answer(
            app.context
                .account()
                .search_users(&query, task_id.as_deref(), &list_ids, include_agents)
                .await,
        ),
        Command::Agents => agents(app).await,
        Command::AstridModel => answer(app.context.agents().astrid_model().await),
        Command::SetAstridModel { agent_id } => answer(
            app.context
                .agents()
                .set_astrid_model(agent_id.as_deref())
                .await,
        ),
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
        Command::SetCustomAgentPhoto { agent_id, image } => answer(
            app.context
                .agents()
                .set_custom_agent_photo(&agent_id, image.as_deref())
                .await,
        ),
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
        Command::TestAgentCredential { service_id } => answer(
            app.context
                .agents()
                .test_credential_outcome(&service_id)
                .await,
        ),
        Command::AgentCredentials => match app.context.agents().credentials().await {
            Ok(answer) => Response::ok(serde_json::json!({
                "keys": answer.get("keys").cloned().unwrap_or_else(|| serde_json::json!({})),
            })),
            Err(error) => Response::failed(error.into()),
        },
        Command::AgentModes => match app.context.agents().modes().await {
            Ok(answer) => Response::ok(serde_json::json!({
                "agents": answer.get("agents").cloned().unwrap_or_else(|| serde_json::json!([])),
                "modes": answer.get("modes").cloned().unwrap_or_else(|| serde_json::json!({})),
            })),
            Err(error) => Response::failed(error.into()),
        },
        Command::CopilotStatus => match app.context.agents().copilot_status().await {
            Ok(answer) => Response::ok(serde_json::json!({
                "connected": answer
                    .get("connected")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })),
            Err(error) => Response::failed(error.into()),
        },
        Command::AvailableAgents { server_run_only } => {
            answer(app.context.agents().available_agents(server_run_only).await)
        }
        Command::DeleteAgentCredential { service_id } => {
            answer_done(app.context.agents().delete_credential(&service_id).await)
        }
        Command::ExternalSync { list_id } => external_sync(app, &list_id).await,
        Command::ExternalAccounts => external_accounts(app).await,
        Command::CreateAndLinkGoogleList { list_id, name } => answer(
            app.context
                .external()
                .create_and_link_google_list(&list_id, &name)
                .await,
        ),
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
        Command::FeatureFlags => answer(app.context.account().feature_flags().await),
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
        Command::ImportExternalLedger {
            provider,
            pending,
            tombstones,
            server_tombstones,
            excluded,
            links,
        } => answer_done(
            crate::external::ledger::import(
                &app.store,
                provider.slug(),
                &crate::external::ledger::LedgerImport {
                    pending,
                    tombstones,
                    server_tombstones,
                    excluded,
                    links,
                },
            )
            .map_err(Into::into),
        ),
        Command::SyncExternal => sync_external(app).await,
        Command::ProfileStats => {
            let me = app.context.account().current_user_id().ok().flatten();
            match me {
                Some(id) => answer(app.context.account().stats(&id).await),
                // Signed out there is nobody to have statistics about, and zeroes would read as an
                // account that has done nothing.
                None => Response::failed(Failure::unauthorized()),
            }
        }
        Command::UpdateProfile {
            name,
            photo_path,
            image,
        } => {
            let account = app.context.account();
            let image = match (photo_path, image) {
                (Some(_), Some(_)) => {
                    return Response::failed(Failure::bad_request(
                        "send photoPath or image, not both",
                    ))
                }
                (Some(path), None) => {
                    match account.upload_photo(std::path::Path::new(&path)).await {
                        Ok(url) => Some(url),
                        Err(error) => return Response::failed(error.into()),
                    }
                }
                // Already uploaded: the screen put the photo on the server when it was picked.
                (None, image) => image,
            };
            match account
                .update_profile(name.as_deref(), image.as_deref())
                .await
            {
                Ok(_) => settings(app),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::ResendVerification => {
            verification_message(app.context.account().resend_verification().await)
        }
        Command::SendVerification => {
            verification_message(app.context.account().send_verification().await)
        }
        Command::CancelVerification => {
            verification_message(app.context.account().cancel_verification().await)
        }
        Command::ChangeEmail { email } => {
            if email.trim().is_empty() {
                return Response::failed(Failure::bad_request("an email address is required"));
            }
            match app.context.account().change_email(&email).await {
                Ok(answer) => Response::ok(answer),
                Err(error) => Response::failed(error.into()),
            }
        }
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
        Command::PasskeyRegistrationOptions => {
            match app.context.account().passkey_registration_options().await {
                Ok(answer) => Response::ok(answer),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::RegisterPasskey {
            session_id,
            attestation,
            name,
        } => {
            if session_id.trim().is_empty() {
                return Response::failed(Failure::bad_request(
                    "a passkey registration needs the sessionId the challenge came with",
                ));
            }
            if attestation.is_null() {
                return Response::failed(Failure::bad_request(
                    "a passkey registration needs the authenticator's response",
                ));
            }
            match app
                .context
                .account()
                .register_passkey(&session_id, attestation, name.as_deref())
                .await
            {
                Ok(()) => Response::done(),
                Err(error) => Response::failed(error.into()),
            }
        }
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
        Command::UploadContacts { contacts } => {
            match app.context.account().upload_contacts(&contacts).await {
                Ok(stats) => Response::ok(stats),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::SearchContacts {
            query,
            exclude_list_id,
            limit,
        } => {
            let query = query.trim();
            // The route answers 400 under two characters; a request that is going to be refused
            // is not worth sending.
            if query.chars().count() < 2 {
                return Response::failed(Failure::bad_request(
                    "a contact search needs at least two characters",
                ));
            }
            match app
                .context
                .account()
                .search_contacts(query, exclude_list_id.as_deref(), limit.unwrap_or(10))
                .await
            {
                Ok(results) => Response::ok(serde_json::json!({ "results": results })),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::RecommendedCollaborators {
            exclude_list_id,
            limit,
        } => answer(
            app.context
                .account()
                .recommended_collaborators(exclude_list_id.as_deref(), limit.unwrap_or(20))
                .await,
        ),
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
        Command::RefreshCapabilities => answer(app.context.account().refresh_capabilities().await),
        Command::FeedbackListId => answer(app.context.account().feedback_list_id()),
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
        Command::BeginSignIn { provider } => match app.auth.begin_with(provider.as_deref()) {
            Ok(url) => Response::ok(serde_json::json!({ "authorizeUrl": url })),
            Err(error) => Response::failed(error.into()),
        },
        Command::CompleteSignIn { callback_url } => answer(app.auth.complete(&callback_url).await),
        Command::CancelSignIn => {
            app.auth.cancel();
            Response::done()
        }
        Command::SignOut => sign_out(app).await,
        // `super::run` is one exhaustive match over every variant and the groups it routes are
        // disjoint, so the compiler has already proved nothing else reaches this arm.
        _ => misrouted("account"),
    }
}

/// What the verify-email route said, as `{ message }` — the same for resend, send and cancel.
fn verification_message(result: crate::services::Result<serde_json::Value>) -> Response {
    match result {
        Ok(answer) => Response::ok(serde_json::json!({
            "message": answer.get("message").cloned().unwrap_or(serde_json::Value::Null),
        })),
        Err(error) => Response::failed(error.into()),
    }
}

/// The inbox as the bell draws it: each row with the key for its kind, so the shell owns the
/// words, and the task's title and identifier so a row reads without a lookup.
pub(super) fn inbox_response(inbox: crate::services::Result<crate::model::Inbox>) -> Response {
    let inbox = match inbox {
        Ok(inbox) => inbox,
        Err(error) => return Response::failed(error.into()),
    };
    let notifications: Vec<serde_json::Value> = inbox
        .notifications
        .iter()
        .map(|notification| {
            serde_json::json!({
                "id": notification.id,
                "kind": notification.kind,
                // `notification.assigned`, `notification.mentioned`, … — a kind this build has
                // not heard of gets a key nobody has a string for, and the shell falls back to
                // the key, which is ugly and visible rather than blank.
                "labelKey": format!("notification.{}", notification.kind),
                "taskId": notification.task_id,
                "taskTitle": notification.task.as_ref().map(|task| task.title.clone()),
                "taskIdentifier": notification.task.as_ref().and_then(|task| task.identifier.clone()),
                "taskCompleted": notification.task.as_ref().map(|task| task.completed).unwrap_or(false),
                "isRead": notification.is_read(),
                "createdAt": notification.created_at.map(crate::model::date::format),
            })
        })
        .collect();
    Response::ok(serde_json::json!({
        "unreadCount": inbox.unread_count,
        "notifications": notifications,
    }))
}

/// Forget the session and everything this machine held for it. Sign-out, and the tail of deleting
/// the account (task 19fd9289).
/// How long sign-out waits for a sync pass or drain already in flight to finish.
const SIGN_OUT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

pub(super) async fn sign_out(app: &App) -> Response {
    // The flow in progress goes with the session. Leaving it would let a callback from before the
    // sign-out complete afterwards and sign the user back in.
    app.auth.cancel();
    // Close the session first: no sync pass or drain starts, the ones in flight are waited for
    // (bounded — one stuck on a dead network must not hold sign-out), and the stream stops
    // applying frames. Cleared before this, the cache was refilled with the departing account's
    // data by whatever was already on its way back.
    let session = app.store.session();
    let settled = session.close(SIGN_OUT_WAIT).await;
    if !settled {
        tracing::debug!("sign-out went ahead with a pass still in flight");
    }
    // Every file this installation holds goes: the ones waiting to be uploaded — their journal is
    // about to be wiped — and the downloads. The next person on this machine should not hold them.
    let _ = std::fs::remove_dir_all(app.attachment_cache());
    // The credentials, then the cache (`AccountService::sign_out`).
    let signed_out = app.context.account().sign_out().await;
    // After the credentials are gone, not before: dropped earlier, the stream would reconnect at
    // once with the departing account's cookie. Now its reconnect finds no session and waits.
    app.realtime().reconnect_now();
    session.reopen();
    match signed_out {
        Ok(()) => Response::done(),
        Err(error) => Response::failed(error.into()),
    }
}

/// The account screen: who is signed in, their reminder settings, and the choices for them.
///
/// The offsets come from the same list the per-task reminder picker uses, so "15 minutes before"
/// means one thing in this app rather than two.
/// The calendar feed a person subscribes to (task 28c5c6a9).
///
/// The one address this client names outside `/api/v1`, and it is never requested from here: it
/// is handed to a calendar application, which fetches it with the person's session. There is no
/// v1 feed, and this is the address the web hands out. What the feed holds is decided on the
/// server from the stored `calendarSyncType`, so the address carries no filter.
const CALENDAR_FEED: &str = "/api/calendar/tasks.ics";

pub(super) fn settings(app: &App) -> Response {
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
        "calendarFeedUrl": format!("{}{CALENDAR_FEED}", app.context.client.base_url()),
        // The task defaults and the task-detail layout, shaped and defaulted in one place, with
        // the choices each combo offers (task c0f3db19).
        "smartTasks": crate::smart_tasks::SmartTaskSettings::from_stored(
            &account.smart_task_settings().unwrap_or_default(),
        ),
        "dueOffsetChoices": crate::smart_tasks::offset_choices(),
        "dueTimeChoices": crate::smart_tasks::time_choices(),
        "layoutChoices": crate::smart_tasks::layout_choices(),
        "subtaskChoices": crate::smart_tasks::subtask_choices(),
    }))
}

/// This user's flags, for the shell to gate its surfaces on. Null is "not asked yet", which the
/// shell treats as "show" — the web only hides what it has been told to hide.
pub(super) fn features(app: &App) -> Response {
    let account = app.context.account();
    let flag = |name: &str| account.has_feature(name).ok().flatten();
    let hide_list_images = flag("hide_list_images");
    let settings = crate::smart_tasks::SmartTaskSettings::from_stored(
        &account.smart_task_settings().unwrap_or_default(),
    );
    Response::ok(serde_json::json!({
        "hideListImages": hide_list_images,
        // iOS ListImagesVisibility: the person's choice, else the test's arm (AATD-39).
        "showListImages": settings.shows_list_images(hide_list_images.unwrap_or(false)),
        "projectMode": flag("project_mode"),
        "googleTasks": flag("google_tasks"),
        "taskCost": flag("task_cost"),
    }))
}

/// The layout to draw with: what the shell asked for, else what the account chose (task c0f3db19).
/// The preference lives on the server and is read from the cache, so a choice made on the web
/// applies here after the next settings refresh, and one made here applies at once.
pub(super) fn resolved_display_mode(app: &App, asked: Option<&str>) -> rows::DisplayMode {
    match asked {
        Some(value) => Command::display_mode(Some(value)),
        None => app.context.account().display_mode(),
    }
}

/// Merge changes into the stored reminder settings and write them back.
pub(super) fn update_reminder_settings(
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
    account.update_settings(serde_json::json!({ "reminderSettings": reminders }))?;
    Ok(())
}

/// The Agent Hub in one answer: the agents, their modes, the credentials, and Copilot.
///
/// Each part is allowed to be missing. A deployment without Copilot answers 404 for it, and a hub
/// that refused to draw because one of four requests failed would be a screen nobody could use to
/// fix the thing that failed.
pub(super) async fn agents(app: &App) -> Response {
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
    // The GitHub App belongs in this answer for the same reason the credentials do: a hub that
    // drew the coding agents before it knew whether the App was installed would say they were
    // ready and then correct itself (task 959b24b5). A deployment without the coding-agent
    // capability answers 403 here, which is the empty list, which is "not installed" — the honest
    // thing to draw, and the state somebody opens this screen to fix.
    let github_app = service
        .github_installations()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));

    Response::ok(serde_json::json!({
        // Projected, not passed through: the route describes each agent as a mailbox row with a
        // `locked` flag, and the shell read `id` and `name` off it — keys it never carried — so
        // every agent drew nameless and off. See `rows::agent`, which also says which modes a
        // locked agent may be offered.
        "agents": rows::agent::rows(&modes),
        "modes": modes.get("modes").cloned().unwrap_or(serde_json::json!({})),
        // Projected, not passed through: the endpoint answers with a MAP of the services a key
        // has already been stored for, and a service with no key — the row somebody opened this
        // screen to fill in — is simply absent from it. See `rows::credential`.
        "credentials": rows::credential::rows(&credentials),
        "copilot": copilot,
        // Projected, not passed through: the endpoint answers a bare list, and "installed" is a
        // length the web card computes itself. See `rows::github_app`.
        "githubApp": rows::github_app::of(&github_app),
    }))
}

/// Everything one list's external-sync panel needs.
///
/// Answers even when nothing is connected: "not connected" is the state the panel exists to show,
/// and a failure there would leave somebody looking at an error instead of a button.
pub(super) async fn external_sync(app: &App, list_id: &str) -> Response {
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
        let integration = integration_in(&status, provider);
        let connected = integration.is_some();

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
            // Which account, not just whether — the same row says "Connected as …" (task
            // 6db679c3), and it is in the status this already asked for.
            "connectedAs": connected_as(integration),
            "containers": containers,
            "link": linked,
        }));
    }

    Response::ok(serde_json::json!({ "listId": list_id, "providers": providers }))
}

/// The account's side of external sync: who each provider is connected as, and every list it has
/// linked (task 6db679c3).
///
/// One command for the whole settings screen, for the reason [`Command::ExternalSync`] gives for
/// one list: a panel filled by three round trips is a panel with three chances to be seen half
/// drawn. Both ends of every link are named here, so nothing downstream has to resolve an opaque
/// Google tasklist id.
pub(super) async fn external_accounts(app: &App) -> Response {
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
        let integration = integration_in(&status, provider);
        // Same reason as the per-list panel: links for a provider nobody has connected can only
        // 401, and an empty section is the honest thing to draw.
        let links = match integration {
            Some(_) => external.links(provider).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let links: Vec<serde_json::Value> = links
            .into_iter()
            .map(|link| {
                let name = app
                    .store
                    .list(&link.astrid_list_id)
                    .ok()
                    .flatten()
                    .map(|list| list.name);
                serde_json::json!({
                    "id": link.id,
                    "listId": link.astrid_list_id,
                    // Absent when the link names a list this device has not cached yet — the
                    // screen shows the link rather than hiding it, and fills the name in on the
                    // next refresh.
                    "listName": name,
                    "containerId": link.remote_container_id,
                    "containerName": link.remote_container_name,
                })
            })
            .collect();

        providers.push(serde_json::json!({
            "provider": provider,
            "connected": integration.is_some(),
            "connectedAs": connected_as(integration),
            "links": links,
        }));
    }

    Response::ok(serde_json::json!({ "providers": providers }))
}

/// The account's integration for one provider, as `/api/v1/integrations` answered it.
///
/// Matched on the server's own name for the provider — see [`crate::services::Provider::wire`]:
/// a name we invented reads a connected account as disconnected.
fn integration_in(
    status: &serde_json::Value,
    provider: crate::services::Provider,
) -> Option<&serde_json::Value> {
    status
        .get("integrations")
        .and_then(|value| value.as_array())?
        .iter()
        .find(|integration| {
            integration.get("provider").and_then(|value| value.as_str()) == Some(provider.wire())
        })
}

/// The Google email or GitHub login the account is connected as, when the server knows one.
fn connected_as(integration: Option<&serde_json::Value>) -> Option<&str> {
    integration?
        .get("externalAccountId")
        .and_then(|value| value.as_str())
}

/// One Google pass over every linked list.
pub(super) async fn sync_external(app: &App) -> Response {
    let external = app.context.external();
    // Before the passes, so a list added on either side since the last one is linked and then
    // synced in the same round rather than a round later.
    let auto_linked = external.auto_link_google().await.unwrap_or_default();
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
    // My Tasks — unlisted tasks assigned to you — against Google's default list, which is where
    // Google's own apps put a task nobody filed anywhere. Only in the all-lists modes.
    let my_tasks = match &auto_linked.my_tasks_container {
        Some(container) => external.sync_my_tasks(container).await.ok(),
        None => None,
    };
    Response::ok(serde_json::json!({
        "passes": passes,
        "autoLinked": auto_linked,
        "myTasks": my_tasks,
    }))
}
