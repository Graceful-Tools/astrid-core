//! The account: settings, sign-out, the agents and the external providers.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

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
pub(super) async fn sign_out(app: &App) -> Response {
    // The flow in progress goes with the session. Leaving it would let a callback from before the
    // sign-out complete afterwards and sign the user back in.
    app.auth.cancel();
    // Files waiting to be uploaded go too. The journal that would have sent them is about to be
    // wiped, so they are bytes belonging to the departing account with nothing left to send them —
    // and the next person on this machine should not be holding them.
    let _ = std::fs::remove_dir_all(
        app.context
            .attachments(app.attachment_cache())
            .pending_dir(),
    );
    match app.context.account().sign_out().await {
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
    Response::ok(serde_json::json!({
        "projectMode": flag("project_mode"),
        "googleTasks": flag("google_tasks"),
        "taskCost": flag("task_cost"),
    }))
}

/// The global quick-add chord, as chosen or as shipped, with its parts for `RegisterHotKey`.
pub(super) fn hotkey(app: &App) -> Response {
    let stored = app
        .store
        .metadata(crate::keyboard::chord::KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::keyboard::chord::DEFAULT.to_string());
    // A stored chord this build cannot read falls back to the default rather than to nothing:
    // an app with no way to summon it is worse than one with the shipped way.
    let chord = crate::keyboard::chord::parse(&stored).unwrap_or_else(|_| {
        crate::keyboard::chord::parse(crate::keyboard::chord::DEFAULT)
            .expect("the default is a chord")
    });
    Response::ok(serde_json::json!({
        "chord": chord.to_string(),
        "ctrl": chord.ctrl,
        "alt": chord.alt,
        "shift": chord.shift,
        "win": chord.win,
        "key": chord.key.to_string(),
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

    Response::ok(serde_json::json!({
        "agents": modes.get("agents").cloned().unwrap_or(serde_json::json!([])),
        "modes": modes.get("modes").cloned().unwrap_or(serde_json::json!({})),
        // Projected, not passed through: the endpoint answers with a MAP of the services a key
        // has already been stored for, and a service with no key — the row somebody opened this
        // screen to fill in — is simply absent from it. See `rows::credential`.
        "credentials": rows::credential::rows(&credentials),
        "copilot": copilot,
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
