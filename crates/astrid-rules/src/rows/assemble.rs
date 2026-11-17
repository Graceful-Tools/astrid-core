//! The list pipeline: filter, sort, splice, window, project — in one place, for both doors.
//!
//! Every step of this already lived in `astrid-rules`: [`crate::filters::filter_refs`],
//! [`crate::filters::sort_by_setting`], [`crate::filters::my_tasks::filter`],
//! [`crate::filters::subtasks::splice_refs`], [`super::TaskRow::build_all`]. What did not was the
//! *assembly* — the order they run in, and what each one is handed. That sat in
//! `astrid-core/src/app/dispatch/row_json.rs`, inside a crate that needs SQLite, tokio and
//! reqwest and so cannot be built to wasm. So web kept about 3,850 lines of TypeScript doing the
//! same thing, iOS about 420 lines of Swift, and the VS Code extension shipped an all-day overdue
//! bug because `is_overdue` was not reachable from an extension host (task `c2109ff8`).
//!
//! Assembly is not a detail. Four contracts run in an order, and a shell that ran them in its own
//! order is four chances to show a different list from web — which is why moving the order here,
//! rather than exposing the four steps and trusting each client to compose them, is the point of
//! this module.
//!
//! ## What this is not
//!
//! It is not the store. Nothing here reads a cache, a clock or an account: the tasks, the
//! subtasks, the lists, the users and the instant all arrive in [`Inputs`]. That is what lets the
//! App door and the stateless door give the same answer — the App door's only remaining job is
//! to fetch, and fetching cannot change a rule.
//!
//! ## The line against task `a09a1e2c` (AWTD2-145)
//!
//! That task narrows what the App door decodes: filter and sort over `TaskSummary` columns, so a
//! list of ten thousand decodes only the fifty rows on screen. The two meet here, and the seam is
//! **select / project**:
//!
//! - **select** — [`select`]: filter, sort, splice, window. Over `&[Task]`, here, for the
//!   stateless door and as the reference answer.
//! - **project** — [`super::TaskRow::build_all`]: the window plus its context into rows. Shared,
//!   untouched by that task.
//!
//! `a09a1e2c` replaces *select*'s input on the App side only: the same [`select`] runs there over
//! tasks rebuilt from the indexed columns, so the rules are not written twice, and [`project`] is
//! handed the decoded window. The stateless door cannot use summaries:
//! web and the extension hold wire-shape tasks in memory, and synthesising a storage projection
//! in order to filter them would be pure overhead plus a storage type shipped to wasm. The cost
//! of the seam is that afterwards two selects must agree — which is what `filters.json`,
//! `my-tasks.json` and `manual-order.json` are for, and why they are driven through the door as
//! of this task rather than through the Rust functions.

use std::collections::HashMap;

use jiff::tz::Offset;
use jiff::Timestamp;

use super::{RowContext, Surface, TaskRow};
use crate::filters;
use crate::model::{Task, TaskList, User};

/// Everything the pipeline needs, already fetched.
pub struct Inputs<'a> {
    /// The list's saved settings: filters, sort, manual order, `show_subtasks`.
    ///
    /// **Ignored when `my_tasks` is set** — My Tasks' settings belong to the account, and the
    /// shape is derived here from the preferences rather than by the caller, so neither door can
    /// synthesise a shape that is missing a field. That is what went wrong before: the App door
    /// built one shape for sorting and `my_tasks::filter` built another for filtering, and they
    /// did not carry the same fields.
    pub list: &'a TaskList,
    /// The account's My Tasks preferences, when this is My Tasks. Its scope rule ("assigned to
    /// me") is not a list filter and cannot be expressed as one (D25).
    ///
    /// A sort chosen for this view is set on the preferences before they are passed, because for
    /// My Tasks the sort setting IS a preference.
    pub my_tasks: Option<&'a crate::filters::my_tasks::Preferences>,
    /// The candidates. Membership is the caller's: a real list hands over its members, a virtual
    /// one everything the account holds.
    pub tasks: &'a [Task],
    /// Every cached subtask, wherever it is filed — iOS splices from its whole store, so a
    /// subtask that was never added to its parent's list still shows under it. Also read for the
    /// projection, because a row's subtask count is every subtask it has.
    pub subtasks: &'a [Task],
    /// Every list, for the chips. Only the ones a task is in are read.
    pub lists: &'a [TaskList],
    /// Everyone the rows might name, for the assignee.
    pub users: &'a [User],
    pub current_user_id: Option<&'a str>,
    /// The account's half of the subtask rule — "inside parent task only" (task 6ac2639a).
    pub subtask_display: Option<&'a str>,
    pub surface: Surface,
    pub now: Timestamp,
    pub offset: Offset,
    /// The window the caller asked for: where it starts, and how many rows.
    pub window_offset: usize,
    pub limit: Option<usize>,
    /// Ids alone. Cheap, and it is what a caller that already holds the tasks wants.
    pub ids_only: bool,
}

/// What the pipeline answers.
pub struct Assembled {
    /// How many rows there are to scroll through — AFTER filtering, subtasks spliced in. What a
    /// virtualised list sizes its scrollbar from.
    pub total: usize,
    /// Where the returned window starts, clamped to `total`.
    pub offset: usize,
    /// How many tasks the filters kept, subtasks included — the sidebar's number.
    pub matched: usize,
    /// The window, in order.
    pub ids: Vec<String>,
    /// The window as rows. Empty when [`Inputs::ids_only`].
    pub rows: Vec<TaskRow>,
    /// How the rows were sorted, so a shell knows whether a drag may rearrange them (task
    /// 7883f710): only an order made by hand survives a refresh.
    pub sort_by: String,
    /// Whether the quick-add bar is offered here (iOS `ListPermissions.canAddTasks`, AATD-37).
    pub can_add_tasks: bool,
}

/// Filter, sort and splice `inputs` into the order a list draws, then return the asked-for window.
///
/// Every step answers as iOS's list does (AITD-460; Jon, 2026-10-03: on disagreement follow iOS):
/// the tasks are read in iOS's store order first ([`filters::display_order`]), so ties in every
/// sort fall the same way; subtasks are spliced from every cached subtask, not only the list's
/// members; and a spliced subtask obeys the view's completion filter alone
/// ([`filters::shown_by_completion`]).
///
/// Returns the ordered window, how many rows there were in all, and how many tasks the filters
/// kept — the three numbers a virtualised list needs.
pub fn select<'a>(inputs: &Inputs<'a>) -> Selection<'a> {
    let shape;
    let list = match inputs.my_tasks {
        Some(preferences) => {
            shape = filters::my_tasks::shape(preferences);
            &shape
        }
        None => inputs.list,
    };

    let mut tasks: Vec<&Task> = inputs.tasks.iter().collect();
    tasks.sort_by(|a, b| filters::display_order(a, b));

    // The saved filters, then — for My Tasks only — the account's own half of the rule, which is
    // not expressible as a list filter (D25). Applied over the references rather than through
    // `filter_refs`, which wants a `&[Task]` it can be handed whole.
    let filtered: Vec<&Task> = tasks
        .iter()
        .copied()
        .filter(|task| {
            filters::keeps(
                task,
                list,
                inputs.current_user_id,
                inputs.now,
                inputs.offset,
            ) && match inputs.my_tasks {
                Some(preferences) => {
                    filters::my_tasks::in_account_scope(task, inputs.current_user_id, preferences)
                }
                None => true,
            }
        })
        .collect();

    // Subtasks are spliced under their parents, so the top-level set is what gets sorted.
    let mut top_level: Vec<&Task> = filtered
        .iter()
        .filter(|task| task.parent_task_id.is_none())
        .copied()
        .collect();
    filters::sort_by_setting(
        &mut top_level,
        list.sort_by.as_deref(),
        list.manual_sort_order.as_deref(),
    );

    let mut subtasks: Vec<&Task> = inputs.subtasks.iter().collect();
    subtasks.sort_by(|a, b| filters::display_order(a, b));
    let indented = filters::subtasks::should_splice(list.show_subtasks, inputs.subtask_display);
    let ordered = filters::subtasks::splice_refs(&top_level, &subtasks, indented, |task| {
        filters::shown_by_completion(task, list, inputs.now, inputs.offset)
    });

    // Only the window the shell asked for. A list of ten thousand crosses the boundary as the
    // fifty rows on screen, which is what the M0 spike was worried about.
    let start = inputs.window_offset.min(ordered.len());
    let end = inputs
        .limit
        .map(|limit| (start + limit).min(ordered.len()))
        .unwrap_or(ordered.len());
    Selection {
        total: ordered.len(),
        matched: filtered.len(),
        offset: start,
        window: ordered[start..end].to_vec(),
        sort_by: list.sort_by.as_deref().unwrap_or("auto").to_string(),
        // A virtual list always; a real one by the reader's right to add to it.
        can_add_tasks: list.is_virtual.unwrap_or(false)
            || inputs.current_user_id.is_some_and(|me| {
                crate::permissions::can_edit_tasks(me, &crate::permissions::access_of(list))
            }),
    }
}

/// [`select`]'s answer: the window, and the two counts that describe what it was cut from.
pub struct Selection<'a> {
    pub total: usize,
    pub matched: usize,
    pub offset: usize,
    pub window: Vec<&'a Task>,
    /// How the rows were sorted — the resolved setting, with `auto` standing for unset.
    pub sort_by: String,
    pub can_add_tasks: bool,
}

/// [`select`], then project the window into rows.
pub fn assemble(inputs: &Inputs<'_>) -> Assembled {
    let selected = select(inputs);
    let rows = match inputs.ids_only {
        true => Vec::new(),
        false => project(inputs, &selected.window),
    };
    finish(selected, rows)
}

/// Project `window` into rows — the *project* half of the seam, callable on its own.
///
/// `window` may be richer than what `inputs` holds. The App door selects over tasks rebuilt from
/// the indexed columns (`inputs.tasks` and `inputs.subtasks` then carry only what filtering,
/// sorting and the parent chain read) and decodes just the window's tasks, which are what it hands
/// here (task `a09a1e2c`). What a row says about the rest of the account — its depth, its subtask
/// count — is read from `inputs`, which needs ids and parents and nothing more.
pub fn project(inputs: &Inputs<'_>, window: &[&Task]) -> Vec<TaskRow> {
    // Every task a row's parent chain can pass through: the candidates and the subtasks,
    // each once (a subtask filed in the list is in both).
    let depths_index: HashMap<String, Task> = inputs
        .tasks
        .iter()
        .chain(inputs.subtasks.iter())
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
    let counts = super::subtask_counts(inputs.subtasks);
    let context = RowContext {
        current_user_id: inputs.current_user_id,
        surface: inputs.surface,
        now: inputs.now,
        offset: inputs.offset,
        lists: inputs.lists,
        users: inputs.users,
        depths: &depths,
        subtask_counts: &counts,
    };
    TaskRow::build_all(window, &context)
}

/// The answer, from a selection and the rows projected from its window.
pub fn finish(selected: Selection<'_>, rows: Vec<TaskRow>) -> Assembled {
    Assembled {
        total: selected.total,
        offset: selected.offset,
        matched: selected.matched,
        ids: selected.window.iter().map(|task| task.id.clone()).collect(),
        rows,
        sort_by: selected.sort_by,
        can_add_tasks: selected.can_add_tasks,
    }
}
