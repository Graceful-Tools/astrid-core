//! The comment box: suggestions, and the local copies of what was posted.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Who this task can be assigned to.
///
/// Assigning itself is an ordinary `updateTask` with an `assigneeId` — null clears it — so there
/// is no separate write here. This is only the question of who may be offered.
///
/// Agents come from the account rather than from the task's lists, so they are whichever cached
/// users say they are agents. Until the core fetches the agent roster (M3) that is only the ones
/// seen embedded in a response; an agent nobody has met yet is simply not offered, which is
/// better than offering a bare id.
/// The comment box's popup (task 3271a0c5). The rules are `parse::mentions`; this gathers what
/// they read from — the task, every list, every task, everyone the cache knows — and asks.
pub(super) fn comment_suggestions(app: &App, task_id: &str, text: &str, caret: usize) -> Response {
    let Some(trigger) = crate::parse::mentions::find_trigger(text, caret) else {
        return Response::ok(serde_json::json!({ "trigger": null, "items": [] }));
    };
    let task = match app.context.tasks().task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return Response::failed(Failure::not_found("task", task_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let lists = app.store.lists().unwrap_or_default();
    let tasks = app.store.tasks().unwrap_or_default();
    let users = app.store.users().unwrap_or_default();
    let me = app.context.account().current_user_id().ok().flatten();
    // The task's own first list stands in for "the open list": the comment box lives in the
    // detail, which is opened from that list far more often than from anywhere else.
    let selected = task.effective_list_ids().into_iter().find(|id| {
        lists
            .iter()
            .any(|list| &list.id == id && list.is_domain_list())
    });
    let items = crate::parse::mentions::suggestions(
        trigger.kind,
        &trigger.query,
        &crate::parse::mentions::Sources {
            task: &task,
            lists: &lists,
            tasks: &tasks,
            users: &users,
            me: me.as_deref(),
            selected_list_id: selected.as_deref(),
        },
    );
    Response::ok(serde_json::json!({ "trigger": trigger, "items": items }))
}

/// Say where each drawable file's bytes already are, so a comment can show the picture rather than
/// an icon standing in for it.
///
/// No network and no download: this only reports what is already on disk. A file this device
/// attached is in the pending directory before it has been anywhere, which is the case worth
/// having — posting a screenshot and then watching it load, from the machine it was taken on, is
/// the bug the Mac fixed in AITD-308.
///
/// Failing to resolve is not an error. The bytes are simply not here yet, and the chip is what a
/// screen draws until they are.
pub(super) fn fill_local_paths(app: &App, task_id: &str, rows: &mut [rows::comment::CommentRow]) {
    let attachments = app.context.attachments(app.attachment_cache());
    let Ok(files) = attachments.for_task(task_id) else {
        return;
    };
    rows::comment::with_local_paths(rows, |id| {
        files
            .iter()
            .find(|file| file.id == id)
            .and_then(|file| attachments.local_path(file))
            .map(|path| path.to_string_lossy().into_owned())
    });
}
