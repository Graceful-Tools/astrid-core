//! One task: its detail, its pickers, its timer, its lists and its assignee.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Everything a new task can be told, as `createTask` says it.
///
/// A struct rather than eleven parameters because there are two callers now: the `createTask` arm
/// and the board's own add-a-card (task 95c7a68f), which has to pick up a list's defaults and the
/// rest of [`create_task`]'s work rather than reach for `TaskService::create` on its own.
pub(super) struct NewTask {
    pub title: String,
    pub description: Option<String>,
    pub list_ids: Vec<String>,
    pub priority: Option<i64>,
    pub due_date_time: Option<String>,
    pub is_all_day: Option<bool>,
    pub assignee_id: Option<String>,
    pub parent_task_id: Option<String>,
    pub status_role: Option<String>,
    pub quick_add: bool,
    pub locale: Option<String>,
}

/// Make a task: what the caller said, plus what its list says a new task looks like.
pub(super) fn create_task(app: &App, new: NewTask) -> crate::services::Result<crate::model::Task> {
    let NewTask {
        title,
        description,
        list_ids,
        priority,
        due_date_time,
        is_all_day,
        assignee_id,
        parent_task_id,
        status_role,
        quick_add,
        locale,
    } = new;

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
    // A role, never a list id: one bad id rejects the whole write, and the board resolves a card's
    // column from the role first (task 95c7a68f).
    draft.status_role = status_role;

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
        let keywords = crate::parse::smart::Keywords::for_locale(locale.as_deref().unwrap_or("en"));
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
    app.context.tasks().create(&draft)
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

    let me = app.context.account().current_user_id().ok().flatten();
    let is_copy_only = rows::copy_only(&task, &lists, me.as_deref());

    // The BOARD STATE row (task 5221e43f): the task's board's columns as chips, Done left out,
    // only for a board task in list mode and never for a read-only viewer. The rule is shared
    // with the web and both Apple clients and asked here, not restated. `null` means no row.
    let board_state = rows::detail::shows_board_state(
        mode,
        rows::detail::is_task_in_project(&task, &lists),
        is_copy_only,
    )
    .then(|| {
        let columns = super::board::task_columns(app, &task);
        let current = crate::board::column_for(&task, &columns);
        serde_json::json!({
            "current": current,
            "chips": rows::detail::board_state_chips(&columns).iter().map(|column| serde_json::json!({
                "id": column.id,
                "name": column.name,
                "kind": column.kind,
                "isCurrent": column.id == current,
            })).collect::<Vec<_>>(),
        })
    });

    // The WAITING ON row (task 69a840a4). Drawn from whatever is cached, so it appears with the
    // rest of the screen and with no network at all; `TaskBlockers` is the round trip that
    // corrects it. `null` means no row, exactly as `boardState` does — the shell holds no
    // predicate of its own. Unlike the state row this one survives a read-only viewer: chips are
    // information, and knowing a task is held is worth having even when you cannot lift the block.
    let dependencies = app.context.dependencies().cached(task_id);
    let blockers = rows::detail::shows_task_blockers(
        rows::detail::is_task_in_project(&task, &lists),
        is_copy_only,
        dependencies
            .as_ref()
            .is_some_and(|it| !it.blocked_by.is_empty()),
    )
    .then(|| {
        let dependencies = dependencies.unwrap_or_default();
        serde_json::json!({
            // Every chip, in the server's order, with no cap and no "+n more": a blocker the
            // reader cannot see is counted and drawn, because it still blocks.
            "chips": dependencies.blocked_by,
            "canEdit": !is_copy_only,
        })
    });

    Response::ok(serde_json::json!({
        "blockers": blockers,
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
        // A task in a public list the reader cannot edit (task f6bc59e8): the header offers a
        // copy where the checkbox would be, and the text is not for editing.
        "isCopyOnly": is_copy_only,
        "boardState": board_state,
        // Whether to DRAW the identifier here, and whether to offer "Copy task id" — the shared
        // show-rule (`rows::identifier`, fixture `task-identifiers.json`), asked once so the shell
        // holds no predicate. The two disagree on purpose for a task that has left every board: it
        // stops showing its key and goes on being copyable, because the key still resolves.
        "showsIdentifier": rows::identifier::shows_identifier(
            rows::Surface::Detail,
            task.identifier.as_deref().is_some_and(|id| !id.is_empty()),
            rows::detail::is_task_in_project(&task, &lists),
        ),
        "offersCopyIdentifier": rows::identifier::offers_copy_identifier(
            task.identifier.as_deref().is_some_and(|id| !id.is_empty()),
        ),
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

/// Refresh one task's dependencies from the server, and answer with them (task 69a840a4).
pub(super) async fn task_blockers(app: &App, task_id: &str) -> Response {
    match app.context.dependencies().refresh(task_id).await {
        Ok(dependencies) => Response::ok(dependencies),
        // Offline is not a failure for a row that already has something to draw: the cache is the
        // answer, and the refresh is the correction. Only a task with nothing cached reports the
        // error, because then there is genuinely nothing to show.
        Err(error) => match app.context.dependencies().cached(task_id) {
            Some(cached) => Response::ok(cached),
            None => Response::failed(error.into()),
        },
    }
}

pub(super) fn add_task_blocker(app: &App, task_id: &str, blocking_task_id: &str) -> Response {
    match app.context.dependencies().add(task_id, blocking_task_id) {
        Ok(dependencies) => Response::ok(dependencies),
        Err(error) => Response::failed(error.into()),
    }
}

pub(super) fn remove_task_blocker(app: &App, task_id: &str, blocking_task_id: &str) -> Response {
    match app.context.dependencies().remove(task_id, blocking_task_id) {
        Ok(dependencies) => Response::ok(dependencies),
        Err(error) => Response::failed(error.into()),
    }
}

/// The candidates the "Wait on a task…" picker may offer.
///
/// Two characters is the threshold — `services::search::MINIMUM_QUERY_LENGTH`, not a number
/// written again here — and below it the answer is an empty list rather than every task in the
/// account. The three exclusions and the same-board ranking are
/// `services::dependency::pickable`, stated there so the shell chooses nothing.
///
/// **This searches the cache, and the spec asks for the server's `GET /api/v1/search`.** The
/// difference is deliberate and recorded in `docs/CONTRACTS.md`: this core has no server-search
/// path at all — `Command::SearchTasks` reads the cache too, as every search surface in this app
/// does — and growing one is its own task rather than a detail of this row. What the spec is
/// guarding against is a picker that filters the page it happens to have loaded (web's
/// `5df85b9f`); this searches the whole synced account with the shared grammar, so it does not
/// have that bug. What it does not have is the server's permission filter *in the query*, which
/// matters for a task synced before a share was revoked.
pub(super) fn task_blocker_candidates(
    app: &App,
    task_id: &str,
    query: &str,
    limit: Option<usize>,
) -> Response {
    let trimmed = query.trim();

    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Response::failed(error.into()),
    };
    let lists = app.store.lists().unwrap_or_default();
    let users = app.store.users().unwrap_or_default();
    let current_user_id = app.context.account().current_user_id().ok().flatten();
    let results = crate::services::search::search(
        &tasks,
        trimmed,
        &crate::services::search::SearchScope {
            list_id: None,
            // A completed task can still be a blocker — a chip draws one struck through — so the
            // picker offers them. Excluding them would make "why can I not find it" the first
            // thing anybody asks.
            include_completed: true,
        },
        &crate::services::search::SearchContext {
            lists: &lists,
            users: &users,
            current_user_id: current_user_id.as_deref(),
            now: app.clock.now(),
            offset: app.clock.utc_offset(),
        },
    );

    let dependencies = app
        .context
        .dependencies()
        .cached(task_id)
        .unwrap_or_default();

    // The task's own board, so its neighbours rank first. A task off every board ranks everything
    // equally, which is the honest answer rather than an arbitrary one.
    let board_list_ids: Vec<String> = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => {
            let project = rows::detail::project_id_for_task(&task, &lists);
            lists
                .iter()
                .filter(|list| project.is_some() && list.project_id == project)
                .map(|list| list.id.clone())
                .collect()
        }
        _ => Vec::new(),
    };

    let offerable =
        crate::services::dependency::pickable(&results, task_id, &dependencies, &board_list_ids);
    let total = offerable.len();
    let candidates: Vec<serde_json::Value> = offerable[..limit.unwrap_or(total).min(total)]
        .iter()
        .map(|task| {
            serde_json::json!({
                "id": task.id,
                "title": task.title,
                "identifier": task.identifier,
                "completed": task.completed,
            })
        })
        .collect();

    Response::ok(serde_json::json!({ "candidates": candidates }))
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
    let lists = app.store.lists().unwrap_or_default();
    // Everyone the query might name by handle, and everyone the rows might draw.
    let users = app.store.users().unwrap_or_default();
    let current_user_id = app.context.account().current_user_id().ok().flatten();
    let found = crate::services::search::search(
        &tasks,
        query,
        &scope,
        &crate::services::search::SearchContext {
            lists: &lists,
            users: &users,
            current_user_id: current_user_id.as_deref(),
            now: app.clock.now(),
            offset: app.clock.utc_offset(),
        },
    );
    let total = found.len();
    let window = &found[..limit.unwrap_or(total).min(total)];

    let depths = std::collections::HashMap::new();
    let counts = rows::subtask_counts(&tasks);

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
