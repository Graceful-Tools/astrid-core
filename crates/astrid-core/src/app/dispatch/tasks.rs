//! One task: its detail, its pickers, its timer, its lists and its assignee.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

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
pub(super) fn task_detail(app: &App, task_id: &str, display_mode: Option<String>) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };

    let now = app.clock.now();
    let offset = app.clock.utc_offset();
    let mode = resolved_display_mode(app, display_mode.as_deref());

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

    // Projected rather than sent raw: a comment's own files are what a screen has to draw, and
    // whether there is a bubble at all is a rule — see `rows::comment`.
    let mut comments = rows::comment::rows(
        &app.context.comments().for_task(task_id).unwrap_or_default(),
        app.context
            .account()
            .current_user_id()
            .ok()
            .flatten()
            .as_deref(),
    );
    fill_local_paths(app, task_id, &mut comments);

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
        // The description as blocks to draw, beside the text to edit: the web renders one and
        // edits the other, and a shell handed only the text drew `**bold**` with its asterisks
        // (task 11cfaf6d). Which markdown means what is `crate::markdown`, mirrored from web.
        "descriptionBlocks": crate::markdown::render(&task.description),
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
        // Closed as anything but done, and the address the menu's "Copy link" copies — the same
        // one the web's own task links carry, built here so one place knows its shape. A task
        // that has not reached the server yet has no address (task 016ce981).
        "isCanceled": task.is_canceled(),
        "link": (!crate::model::is_temp_id(&task.id))
            .then(|| format!("{}/tasks/{}", app.context.client.base_url(), task.id)),
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
/// What a calendar day means for one task.
///
/// Answers in the same shape a quick pick does, so the shell takes it down the path it already
/// has rather than growing a second one.
pub(super) fn due_date_on_day(app: &App, task_id: &str, day: &str) -> Response {
    let Ok(day) = day.parse::<chrono::NaiveDate>() else {
        // 400: the caller sent something this command cannot mean, which is not a failure of the
        // account, the network or the cache.
        return Response::failed(Failure::refused(400, "day must be YYYY-MM-DD"));
    };
    let task = match app.store.task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => {
            return Response::failed(
                crate::services::ServiceError::NotFound {
                    kind: "task",
                    id: task_id.to_string(),
                }
                .into(),
            )
        }
        Err(error) => return Response::failed(error.into()),
    };

    let picked = rows::due_picks::on_day(
        day,
        task.due_date_time,
        task.is_all_day,
        app.clock.utc_offset(),
    );
    Response::ok(serde_json::json!({
        "dueDateTime": date::format(picked),
        "isAllDay": task.is_all_day,
    }))
}

pub(super) fn due_date_options(app: &App, task_id: &str) -> Response {
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
pub(super) fn search_tasks(
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

/// What a task's timer is doing, running or not.
pub(super) fn timer_state(
    app: &App,
    task: &crate::model::Task,
) -> crate::services::timer::TimerState {
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
pub(super) fn start_timer(app: &App, task_id: &str) -> Response {
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
pub(super) fn stop_timer(app: &App, task_id: &str) -> Response {
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

/// A link other people can open, minted on the server like the web's (task 016ce981). A task that
/// has not reached the server yet has no id the server knows, so there is nothing to mint.
pub(super) async fn share_task(app: &App, task_id: &str) -> Response {
    if crate::model::is_temp_id(task_id) {
        return Response::failed(Failure::bad_request(
            "this task has not reached the server yet, so it cannot be shared",
        ));
    }
    match app.context.share().link_for_task(task_id).await {
        Ok(url) => Response::ok(serde_json::json!({ "url": url })),
        Err(error) => Response::failed(error.into()),
    }
}

/// The repeat presets and this task's own repeat, described.
///
/// Setting one is an ordinary `updateTask` carrying `repeating`, `repeatFrom` and
/// `repeatingData` — the same three fields the API takes — so there is no separate write.
pub(super) fn repeat_options(app: &App, task_id: &str) -> Response {
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

/// What the detail's list editor shows for a task (task d3f3b111). The rules are
/// `rows::list_picks`; this only finds the task and hands over every list.
pub(super) fn list_picks(app: &App, task_id: &str, query: &str) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let lists = app.store.lists().unwrap_or_default();
    Response::ok(rows::list_picks::picks(
        &task.effective_list_ids(),
        &lists,
        query,
    ))
}

/// Change which lists a task is in, by editing the set it has. One write through the task
/// service, which journals it for the Outbox like any other edit.
pub(super) fn change_task_lists(
    app: &App,
    task_id: &str,
    edit: impl FnOnce(&mut Vec<String>),
) -> Response {
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let mut list_ids = task.effective_list_ids();
    edit(&mut list_ids);
    answer(app.context.tasks().set_lists(task_id, list_ids))
}

/// The editor's **Create "…"**: a list in one of the web's colours, with the privacy the task's
/// other lists have, and the task filed in it — two Outbox entries, the second depending on the
/// first's id.
pub(super) fn create_list_for_task(app: &App, task_id: &str, name: &str) -> Response {
    let name = name.trim();
    if name.is_empty() {
        return Response::failed(Failure::bad_request("a list needs a name"));
    }
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let lists = app.store.lists().unwrap_or_default();
    let mut list_ids = task.effective_list_ids();
    let siblings = list_ids
        .iter()
        .filter_map(|id| lists.iter().find(|list| &list.id == id));
    let privacy = rows::list_picks::privacy_for_new_list(siblings);

    let list = match app.context.lists().create_with(
        name,
        Some(rows::list_picks::random_list_color().to_string()),
        Some(privacy),
    ) {
        Ok(list) => list,
        Err(error) => return Response::failed(error.into()),
    };
    list_ids.push(list.id.clone());
    match app.context.tasks().set_lists(task_id, list_ids) {
        Ok(task) => Response::ok(serde_json::json!({ "list": list, "task": task })),
        Err(error) => Response::failed(error.into()),
    }
}

pub(super) fn assignee_options(app: &App, task_id: &str) -> Response {
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

/// Read an edit from the shape the shell sends.
///
/// A field that is **present and null** clears; a field that is **absent** is left alone. That is
/// the distinction [`TaskChanges`] exists for, and reading it from JSON is the only place it can be
/// lost — `Option<Option<T>>` through serde needs the double wrap spelled out, which is why this is
/// hand-written rather than derived.
pub(super) fn changes_from_json(value: &serde_json::Value) -> Result<TaskChanges, Failure> {
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
