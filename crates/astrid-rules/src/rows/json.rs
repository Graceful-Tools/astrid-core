//! A row on the wire.
//!
//! Hand-written rather than derived so the field names are chosen for the shell that reads them
//! and cannot drift when a Rust field is renamed for Rust reasons.
//!
//! This lived in `astrid-core/src/app/dispatch/row_json.rs` until task `c2109ff8`. The shape of a
//! row is a cross-client contract — astrid-windows, astrid-ios and the VS Code extension all
//! decode these keys — and it was in the one crate that cannot be built to wasm, so the stateless
//! door could not have answered with a row even once the pipeline reached it. Both doors now
//! serialise through here, which is also what makes "`rowsForList`'s output is unchanged" a
//! property of the code rather than of a careful copy.

use super::{assemble::Assembled, DueLabel, LeadingAction, LeadingControl, TaskRow};

/// The whole answer a list asks for: the window, the counts, and what the shell may do with them.
///
/// `idsOnly` answers the first four keys and no rows — the caller already holds the tasks, so
/// ids are enough to order them.
pub fn answer(assembled: &Assembled, ids_only: bool) -> serde_json::Value {
    if ids_only {
        return serde_json::json!({
            "total": assembled.total,
            "offset": assembled.offset,
            "matched": assembled.matched,
            "ids": assembled.ids,
            "sortBy": assembled.sort_by,
        });
    }
    serde_json::json!({
        // The total is what a virtualised list needs to size its scrollbar, and it is the count
        // AFTER filtering — the number of rows there are to scroll through, not the number of
        // tasks in the account.
        "total": assembled.total,
        "offset": assembled.offset,
        // How many tasks the filters kept, subtasks included — the sidebar's number.
        "matched": assembled.matched,
        "rows": rows(&assembled.rows),
        // How the rows were sorted, so the shell knows whether a drag may rearrange them
        // (task 7883f710): only an order made by hand survives a refresh.
        "sortBy": assembled.sort_by,
        // Whether the quick-add bar is offered here (iOS ListPermissions.canAddTasks, AATD-37).
        "canAddTasks": assembled.can_add_tasks,
    })
}

/// Rows on the wire.
pub fn rows(rows: &[TaskRow]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "title": row.title,
                "identifier": row.identifier,
                "showsIdentifier": row.shows_identifier,
                "offersCopyIdentifier": row.offers_copy_identifier,
                "completed": row.completed,
                "priority": row.priority.as_i64(),
                "due": due(&row.due),
                "dueAt": row.due_at.map(|at| serde_json::json!({
                    "date": at.format("%Y-%m-%d").to_string(),
                    "time": at.format("%H:%M").to_string(),
                })),
                "isOverdue": row.is_overdue,
                "leading": leading(&row.leading),
                "action": match row.action {
                    LeadingAction::Complete => "complete",
                    LeadingAction::OpenPicker => "openPicker",
                    LeadingAction::Copy => "copy",
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
                    "id": chip.id, "name": chip.name, "color": chip.color, "isLabel": chip.is_label,
                    "glyph": chip.glyph.as_str()
                })).collect::<Vec<_>>(),
                "assignee": row.assignee,
                "statusRole": row.status_role,
                "isCanceled": row.is_canceled,
                "syncMark": row.sync_mark,
                "remote": row.remote_chip.as_ref().map(|(label, url)| serde_json::json!({ "label": label, "url": url })),
            })
        })
        .collect()
}

/// A due label as a key plus its parts. The shell owns the words; see [`super`].
pub fn due(due: &DueLabel) -> serde_json::Value {
    match due {
        DueLabel::None => serde_json::json!({ "key": "none" }),
        DueLabel::Yesterday => serde_json::json!({ "key": "yesterday" }),
        DueLabel::Today => serde_json::json!({ "key": "today" }),
        DueLabel::Tomorrow => serde_json::json!({ "key": "tomorrow" }),
        DueLabel::On { day, time } => serde_json::json!({
            "key": "on",
            "date": day.format("%Y-%m-%d").to_string(),
            "time": time.map(|time| time.format("%H:%M").to_string()),
        }),
    }
}

pub fn leading(leading: &LeadingControl) -> serde_json::Value {
    match leading {
        LeadingControl::Checkbox => serde_json::json!({ "kind": "checkbox" }),
        LeadingControl::Unassigned => serde_json::json!({ "kind": "unassigned" }),
        LeadingControl::Copy => serde_json::json!({ "kind": "copy" }),
        LeadingControl::Avatar(id) => {
            serde_json::json!({ "kind": "avatar", "userId": id })
        }
    }
}
