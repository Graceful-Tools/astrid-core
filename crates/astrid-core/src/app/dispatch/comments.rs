//! The comment box: suggestions, and the local copies of what was posted.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Every command about a comment, handed here whole by [`super::run`]
/// (task 8a91c5b0).
///
/// Every arm is short on purpose: it names a service call, and the service decides. An arm that
/// grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file nobody
/// would look in for it.
pub(super) async fn run(app: &App, command: Command) -> Response {
    match command {
        Command::Comments { task_id } => match app.context.comments().for_task(&task_id) {
            Ok(comments) => Response::ok(comments),
            Err(error) => Response::failed(error.into()),
        },
        Command::PostComment {
            task_id,
            content,
            parent_comment_id,
            comment_type,
            file_id,
            author_id,
            client_request_id,
        } => {
            let author =
                author_id.or_else(|| app.context.account().current_user_id().ok().flatten());
            // A file already on the server travels by id; the rest of its record comes back with
            // the comment.
            let file = file_id.map(|id| crate::model::SecureFile {
                id,
                name: String::new(),
                size: 0,
                mime_type: String::new(),
            });
            answer(app.context.comments().post_as(
                &task_id,
                &content,
                author.as_deref(),
                comment_type.unwrap_or(crate::model::CommentType::Text),
                file.as_ref(),
                parent_comment_id.as_deref(),
                client_request_id.as_deref(),
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
        Command::RefreshComments { task_id } => {
            // Projected the same way the detail projects them, so a refresh cannot draw a comment
            // differently from the screen it lands in.
            let me = app.context.account().current_user_id().ok().flatten();
            match app.context.comments().refresh(&task_id).await {
                Ok(comments) => {
                    // The same autolink context the detail builds, for the same reason the
                    // projection is shared: a refreshed comment must not draw differently from
                    // the screen it lands in.
                    let lists = app.store.lists().unwrap_or_default();
                    let identifiers = match app.store.task(&task_id).ok().flatten() {
                        Some(task) => identifier_context(app, &task, &lists),
                        None => crate::identifier::LinkContext::default(),
                    };
                    let mut rows = rows::comment::rows(&comments, me.as_deref(), &identifiers);
                    fill_local_paths(app, &task_id, &mut rows);
                    Response::ok(rows)
                }
                Err(error) => Response::failed(error.into()),
            }
        }
        // `super::run` is one exhaustive match over every variant and the groups it routes are
        // disjoint, so the compiler has already proved nothing else reaches this arm.
        _ => misrouted("comments"),
    }
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
/// The autolink context for one task's comments (task 5f3453e2).
///
/// The reader's own boards are the keys that link, and the board this task sits on is the one whose
/// key a bare `#12` means. Both come from this device's own cache, so drawing a comment still costs
/// no request — see [`crate::identifier::context_for`].
pub(super) fn identifier_context(
    app: &App,
    task: &crate::model::Task,
    lists: &[crate::model::TaskList],
) -> crate::identifier::LinkContext {
    crate::identifier::context_for(
        &app.store.projects().unwrap_or_default(),
        crate::rows::detail::project_id_for_task(task, lists).as_deref(),
    )
}

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
