//! The rows a list draws, and how a row is written to JSON.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Every command that answers with rows, counts or badges, handed here whole by [`super::run`]
/// (task 8a91c5b0).
///
/// Every arm is short on purpose: it names a service call, and the service decides. An arm that
/// grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file nobody
/// would look in for it.
pub(super) async fn run(app: &App, command: Command) -> Response {
    match command {
        Command::RowsForList {
            list_id,
            surface,
            offset,
            limit,
            inputs,
        } => rows_for_list(
            app,
            &list_id,
            RowsWindow {
                surface,
                offset,
                limit,
            },
            *inputs,
        ),
        Command::ListCounts {
            lists,
            current_user_id,
        } => list_counts(app, &lists, current_user_id),
        Command::ReminderTriageDeck { reminded_task_id } => {
            reminder_triage_deck(app, reminded_task_id.as_deref())
        }
        Command::FeaturedRows { list_id, list } => {
            featured_rows(app, &list_id, list.map(|list| *list)).await
        }
        Command::ListBadges {
            lists,
            my_tasks_id,
            current_user_id,
        } => list_badges(app, &lists, my_tasks_id, current_user_id),
        Command::Sidebar { current_user_id } => sidebar(app, current_user_id),
        Command::ProfileTasks { user_id } => row_json::profile_rows(app, user_id.as_deref()).await,
        // `super::run` is one exhaustive match over every variant and the groups it routes are
        // disjoint, so the compiler has already proved nothing else reaches this arm.
        _ => misrouted("row_json"),
    }
}

/// Where the rows go and which of them: the projection's settings and the window.
pub(super) struct RowsWindow {
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
        surface,
        offset,
        limit,
    } = window;

    // My Tasks is not in the list collection — it is the view the app opens on, and its filters
    // belong to the account rather than to a list row. Everything after this is the same pipeline.
    let my_tasks = list_id == crate::filters::my_tasks::VIRTUAL_ID;
    let mut preferences = match (my_tasks, inputs.my_tasks) {
        (false, _) => None,
        (true, Some(held)) => Some(held),
        (true, None) => match app.context.account().my_tasks_preferences() {
            Ok(preferences) => Some(preferences),
            Err(error) => return Response::failed(error.into()),
        },
    };
    let mut list = match (&preferences, inputs.list) {
        // The shape is derived by the pipeline from the preferences, so nothing is built here:
        // `rows::assemble::select` reads the sort setting off it, the same as any list's, and the
        // completion setting decides which subtasks show.
        (Some(_), _) => crate::filters::my_tasks::shape(
            preferences.as_ref().expect("the preferences just matched"),
        ),
        (None, Some(held)) => held,
        (None, None) => match app.store.list(list_id) {
            Ok(Some(list)) => list,
            Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
            Err(error) => return Response::failed(error.into()),
        },
    };
    // A sort chosen for this view. For My Tasks the setting IS a preference, so it goes there —
    // the pipeline derives the shape from the preferences and would otherwise overwrite it.
    if let Some(sort_by) = inputs.sort_by {
        match &mut preferences {
            Some(preferences) => preferences.sort_by = sort_by,
            None => list.sort_by = Some(sort_by),
        }
    }
    // A list that has never been given a sort stays unset here: `filters::sort_by_setting` and
    // the answer's `sortBy` both read the absence as auto, as web does (CONTRACTS D40).

    // From the cache, the candidates are read as columns and only the window is decoded (task
    // `a09a1e2c`). Not when the caller handed the tasks over, and not for `public_lists`, which
    // reads the nested lists a column does not keep (`filters::matches_in_lists`, D61).
    if inputs.tasks.is_none() && list.filter_in_lists.as_deref() != Some("public_lists") {
        return bounded_rows_for_list(
            app,
            &list,
            preferences.as_ref(),
            RowsWindow {
                surface,
                offset,
                limit,
            },
            BoundedInputs {
                current_user_id: inputs.current_user_id,
                subtask_display: inputs.subtask_display,
                ids_only: inputs.ids_only,
            },
        );
    }

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
    let (tasks, current_user_id) = match (source, current_user_id) {
        (Ok(tasks), Ok(user)) => (tasks, user),
        (Err(error), _) => return Response::failed(error.into()),
        (_, Err(error)) => return Response::failed(error.into()),
    };

    // The account's half of the rule — "inside parent task only" — beside the list's (task
    // 6ac2639a). See `filters::subtasks` for which one wins.
    let subtask_display = inputs
        .subtask_display
        .unwrap_or_else(|| app.context.account().smart_tasks().subtask_display);
    // Every cached subtask, wherever it is filed: iOS splices from its whole store, so a subtask
    // that was never added to its parent's list still shows under it. Read for the projection
    // too — a row's subtask count is every subtask it has — so only an ids-only answer with
    // nothing indented can skip the read.
    let indented = filters::subtasks::should_splice(list.show_subtasks, Some(&subtask_display));
    let subtasks = match indented || !inputs.ids_only {
        true => match app.store.subtasks() {
            Ok(subtasks) => subtasks,
            Err(error) => return Response::failed(error.into()),
        },
        false => Vec::new(),
    };

    let (lists, users) = match inputs.ids_only {
        true => (Vec::new(), Vec::new()),
        false => {
            let mut lists = match app.store.lists() {
                Ok(lists) => lists,
                Err(error) => return Response::failed(error.into()),
            };
            // A list handed in that this account does not hold — a featured public list — still
            // decides which rows are someone else's, read-only, copy-only (AATD-46).
            if !lists.iter().any(|held| held.id == list.id) {
                lists.push(list.clone());
            }
            // Everyone the rows might name. Small: the assignees of the tasks in one list.
            let users = tasks
                .iter()
                .filter_map(|task| task.assignee_id.as_deref())
                .filter_map(|id| app.store.user(id).ok().flatten())
                .collect::<Vec<_>>();
            (lists, users)
        }
    };

    // Everything above this line is a fetch. The pipeline itself — filter, sort, splice, window,
    // project — is `astrid_rules::rows::assemble`, which the stateless door runs too, so a client
    // that cannot link Rust gets the same list rather than its own approximation of one
    // (task c2109ff8).
    let assembled = rows::assemble::assemble(&rows::assemble::Inputs {
        list: &list,
        my_tasks: preferences.as_ref(),
        tasks: &tasks,
        subtasks: &subtasks,
        lists: &lists,
        users: &users,
        current_user_id: current_user_id.as_deref(),
        subtask_display: Some(&subtask_display),
        surface: Command::surface(surface.as_deref()),
        now,
        offset: offset_from_utc,
        window_offset: offset.unwrap_or(0),
        limit,
        ids_only: inputs.ids_only,
    });
    Response::ok(rows::json::answer(&assembled, inputs.ids_only))
}

/// What [`bounded_rows_for_list`] still needs of the command's inputs.
struct BoundedInputs {
    current_user_id: Option<String>,
    subtask_display: Option<String>,
    ids_only: bool,
}

/// [`rows_for_list`] from the cache without decoding the account (task `a09a1e2c`).
///
/// The same pipeline, in the same order, by the same rules: `astrid_rules::rows::assemble::select`
/// runs over tasks rebuilt from the indexed columns ([`crate::store::TaskSummary::skeleton`]),
/// which carry everything filtering, sorting and the parent chain read, and `project` is handed
/// only the window — the one part that is decoded. A list of ten thousand decodes the rows on
/// screen, not the ten thousand.
fn bounded_rows_for_list(
    app: &App,
    list: &crate::model::TaskList,
    preferences: Option<&crate::filters::my_tasks::Preferences>,
    window: RowsWindow,
    inputs: BoundedInputs,
) -> Response {
    let now = app.clock.now();
    let offset_from_utc = app.clock.utc_offset();
    let RowsWindow {
        surface,
        offset,
        limit,
    } = window;

    let current_user_id = match inputs.current_user_id {
        Some(held) => Some(held),
        None => match app.context.account().current_user_id() {
            Ok(user) => user,
            Err(error) => return Response::failed(error.into()),
        },
    };
    let subtask_display = inputs
        .subtask_display
        .unwrap_or_else(|| app.context.account().smart_tasks().subtask_display);
    let indented = filters::subtasks::should_splice(list.show_subtasks, Some(&subtask_display));

    let summaries = match list.is_virtual.unwrap_or(false) {
        true => app.store.summaries(),
        false => app.store.summaries_in_list(&list.id),
    };
    let filed = app.store.filed_task_ids();
    let subtask_summaries = match indented || !inputs.ids_only {
        true => app.store.subtask_summaries(),
        false => Ok(Vec::new()),
    };
    let (summaries, filed, subtask_summaries) = match (summaries, filed, subtask_summaries) {
        (Ok(summaries), Ok(filed), Ok(subtasks)) => (summaries, filed, subtasks),
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
            return Response::failed(error.into())
        }
    };
    let skeletons: Vec<crate::model::Task> = summaries
        .iter()
        .map(|summary| summary.skeleton(filed.contains(&summary.id)))
        .collect();
    let subtask_skeletons: Vec<crate::model::Task> = subtask_summaries
        .iter()
        .map(|summary| summary.skeleton(filed.contains(&summary.id)))
        .collect();

    let mut lists = Vec::new();
    if !inputs.ids_only {
        lists = match app.store.lists() {
            Ok(lists) => lists,
            Err(error) => return Response::failed(error.into()),
        };
        if !lists.iter().any(|held| held.id == list.id) {
            lists.push(list.clone());
        }
    }

    // Everything above is a fetch of columns. Selecting is the shared pipeline.
    let mut pipeline = rows::assemble::Inputs {
        list,
        my_tasks: preferences,
        tasks: &skeletons,
        subtasks: &subtask_skeletons,
        lists: &lists,
        users: &[],
        current_user_id: current_user_id.as_deref(),
        subtask_display: Some(&subtask_display),
        surface: Command::surface(surface.as_deref()),
        now,
        offset: offset_from_utc,
        window_offset: offset.unwrap_or(0),
        limit,
        ids_only: inputs.ids_only,
    };
    let selected = rows::assemble::select(&pipeline);
    if inputs.ids_only {
        let assembled = rows::assemble::finish(selected, Vec::new());
        return Response::ok(rows::json::answer(&assembled, true));
    }

    // The window, decoded — in the order selected, which the store's answer is not in. A task
    // deleted between the two reads is simply not there.
    let ids: Vec<String> = selected.window.iter().map(|task| task.id.clone()).collect();
    let mut decoded: std::collections::HashMap<String, crate::model::Task> =
        match app.store.tasks_with_ids(&ids) {
            Ok(tasks) => tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
            Err(error) => return Response::failed(error.into()),
        };
    let window: Vec<crate::model::Task> = ids.iter().filter_map(|id| decoded.remove(id)).collect();
    // Only the people the window names.
    let users = window
        .iter()
        .filter_map(|task| task.assignee_id.as_deref())
        .filter_map(|id| app.store.user(id).ok().flatten())
        .collect::<Vec<_>>();
    pipeline.users = &users;
    let refs: Vec<&crate::model::Task> = window.iter().collect();
    let rows = rows::assemble::project(&pipeline, &refs);
    let assembled = rows::assemble::finish(selected, rows);
    Response::ok(rows::json::answer(&assembled, false))
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
    match badge_map(app, lists, my_tasks_id, current_user_id) {
        Ok(badges) => Response::ok(serde_json::Value::Object(badges)),
        Err(failed) => failed,
    }
}

/// The badges themselves, for the two commands that answer with them: `listBadges` sends them as
/// they are, `sidebar` puts each one on its list. Factored out rather than read back out of a
/// `Response`, so the sidebar cannot drift from the numbers `listBadges` gives.
fn badge_map(
    app: &App,
    lists: &[crate::model::TaskList],
    my_tasks_id: Option<String>,
    current_user_id: Option<String>,
) -> Result<serde_json::Map<String, serde_json::Value>, Response> {
    let current_user_id = match current_user_id {
        Some(held) => Some(held),
        None => match app.context.account().current_user_id() {
            Ok(user) => user,
            Err(error) => return Err(Response::failed(error.into())),
        },
    };
    let tasks = match app.store.tasks() {
        Ok(tasks) => tasks,
        Err(error) => return Err(Response::failed(error.into())),
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
    Ok(badges)
}

/// The sidebar, whole: the places, in order, each carrying its badge (task e22621e2).
///
/// Three calls and a filter in every shell before this — `lists`, `myTasksList`, `listBadges` —
/// and the filter was wrong on Android. The lists are the cache's, as `lists` answers them, so
/// this says the same as the three calls it replaces; what it adds is that the filtering and the
/// ordering are [`astrid_rules::rows::sidebar::sections`]' and nobody else's. With the
/// `project_mode` flag it adds the Projects section over the cached projects (AWTD-1681).
fn sidebar(app: &App, current_user_id: Option<String>) -> Response {
    let lists = match app.context.lists().all() {
        Ok(lists) => lists,
        Err(error) => return Response::failed(error.into()),
    };
    let my_tasks = app.context.lists().my_tasks();
    let badges = match badge_map(app, &lists, Some(my_tasks.id.clone()), current_user_id) {
        Ok(badges) => badges,
        Err(failed) => return failed,
    };
    let badges: std::collections::HashMap<String, i64> = badges
        .into_iter()
        .filter_map(|(id, count)| count.as_i64().map(|count| (id, count)))
        .collect();
    // The Projects section is drawn only with Project Mode (AWTD-1681). A flag not fetched yet
    // reads as off: a section that appears is better than one that flashes and vanishes.
    let project_mode = app
        .context
        .account()
        .has_feature("project_mode")
        .ok()
        .flatten()
        .unwrap_or(false);
    let projects = if project_mode {
        match app.store.projects() {
            Ok(projects) => Some(projects),
            Err(error) => return Response::failed(error.into()),
        }
    } else {
        None
    };
    Response::ok(crate::rows::sidebar::sections(
        &lists,
        Some(my_tasks),
        &badges,
        projects.as_deref(),
    ))
}

/// A featured public list's rows: the server's tasks plus this account's unsent ones for it,
/// through the same projection as any list (AATD-46).
///
/// The answer also carries those tasks whole, as `tasks` (AITD-634): they are not cached, so a
/// shell that draws and opens task models — the Apple apps — would otherwise have to fetch the
/// same list a second time for the rows it was just handed.
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
    let whole = serde_json::to_value(&tasks).unwrap_or_default();
    let mut answer = rows_for_list(
        app,
        list_id,
        RowsWindow {
            surface: None,
            offset: None,
            limit: None,
        },
        crate::app::command::RowsInputs {
            list: Some(list),
            tasks: Some(tasks),
            ..Default::default()
        },
    );
    if let Some(serde_json::Value::Object(value)) = answer.value.as_mut() {
        value.insert("tasks".to_string(), whole);
    }
    answer
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
        // Enough to draw someone else's profile from this one answer (AATD-69): who they are and
        // their three numbers, from the same moment as the rows. Never an email — the server gives
        // one only on your own profile, and `currentUser` already has yours.
        "id": answer.pointer("/user/id").cloned().unwrap_or(serde_json::Value::String(asked_for.clone())),
        "createdAt": answer.pointer("/user/createdAt").cloned().unwrap_or(serde_json::Value::Null),
        "isAIAgent": answer.pointer("/user/isAIAgent").cloned().unwrap_or(serde_json::Value::Null),
        "stats": answer.get("stats").cloned().unwrap_or(serde_json::Value::Null),
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
    // The deck itself is a rule, and lives with the others (task `75a27f7f`): the stateless door's
    // `reminderTriage` answers from passed tasks, and this one from the cache, over one copy.
    let deck = rows::triage::deck(&tasks, &me, reminded_task_id, now, offset);
    let lists = app.store.lists().unwrap_or_default();
    let users = app.store.users().unwrap_or_default();
    let depths = std::collections::HashMap::new();
    let counts = std::collections::HashMap::new();
    let context = RowContext {
        current_user_id: Some(me.as_str()),
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
/// The shape itself is [`rows::json`], in the rules crate, because it is a cross-client contract
/// and a crate that needs SQLite and tokio is not where a contract web has to match can live
/// (task `c2109ff8`). These three stay as the names the rest of this dispatch calls.
pub(super) fn serialize_rows(rows: &[TaskRow]) -> Vec<serde_json::Value> {
    rows::json::rows(rows)
}

/// A due label as a key plus its parts. The shell owns the words; see `crate::rows`.
pub(super) fn due_json(due: &rows::DueLabel) -> serde_json::Value {
    rows::json::due(due)
}

pub(super) fn leading_json(leading: &rows::LeadingControl) -> serde_json::Value {
    rows::json::leading(leading)
}
