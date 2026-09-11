//! A list's filters and sort, and My Tasks' account-wide ones.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// What a list is filtered and sorted by, and what else it could be.
///
/// The rules are `crate::filters`; this is the sheet. Every value here is one those rules match on
/// — they are saved on the list and read by every client, so a value spelled differently would be
/// a filter the others keep and this one silently ignores.
pub(super) fn filter_options(app: &App, list_id: &str) -> Response {
    let list = if list_id == crate::filters::my_tasks::VIRTUAL_ID {
        // The same groups, filled in from the account's preferences: one filter sheet, whichever
        // of the two it is looking at.
        match app.context.account().my_tasks_preferences() {
            Ok(preferences) => my_tasks_shape(&preferences),
            Err(error) => return Response::failed(error.into()),
        }
    } else {
        match app.context.lists().list(list_id) {
            Ok(Some(list)) => list,
            Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
            Err(error) => return Response::failed(error.into()),
        }
    };
    Response::ok(serde_json::json!({
        "listId": list_id,
        "isFiltered": rows::filter_picks::is_filtered(&list),
        "groups": rows::filter_picks::groups(&list),
    }))
}

/// My Tasks' preferences in the shape a list's filters have.
///
/// The sheet, the "is anything narrowing this" test and the sort all read a `TaskList`, and having
/// two of each — one for lists, one for My Tasks — is two places for them to disagree about what
/// "this week" means. The priority *set* collapses to the one the sheet can show: the sheet offers
/// one at a time, and a set chosen elsewhere is left alone unless somebody changes it here.
pub(super) fn my_tasks_shape(
    preferences: &crate::filters::my_tasks::Preferences,
) -> crate::model::TaskList {
    let mut shape = crate::model::TaskList::new(crate::filters::my_tasks::VIRTUAL_ID, "My Tasks");
    shape.is_virtual = Some(true);
    shape.filter_completion = Some(preferences.filter_completion.clone());
    shape.filter_due_date = Some(preferences.filter_due_date.clone());
    shape.filter_priority = Some(
        preferences
            .filter_priority
            .first()
            .map(i64::to_string)
            .unwrap_or_else(|| "all".into()),
    );
    shape.sort_by = Some(preferences.sort_by.clone());
    shape.manual_sort_order = Some(preferences.manual_sort_order.clone());
    shape
}

/// Set one filter, wherever that filter lives.
pub(super) async fn set_filter(app: &App, list_id: &str, field: &str, value: &str) -> Response {
    if list_id != crate::filters::my_tasks::VIRTUAL_ID {
        // A list's filters are fields on the list, so this is an ordinary update — through the
        // same decoder the `updateList` command uses, so one field cannot mean two things.
        return match list_changes_from_json(&serde_json::json!({ field: value })) {
            Ok(changes) => answer(app.context.lists().update(list_id, &changes)),
            Err(failure) => Response::failed(failure),
        };
    }

    let account = app.context.account();
    let mut preferences = match account.my_tasks_preferences() {
        Ok(preferences) => preferences,
        Err(error) => return Response::failed(error.into()),
    };
    match field {
        "filterCompletion" => preferences.filter_completion = value.to_string(),
        "filterDueDate" => preferences.filter_due_date = value.to_string(),
        // Back into the set the account stores. "all" is no priority chosen, which is what an
        // empty set means — see `astrid_core::filters::my_tasks`.
        "filterPriority" => {
            preferences.filter_priority = value
                .parse::<i64>()
                .map(|one| vec![one])
                .unwrap_or_default()
        }
        "sortBy" => preferences.sort_by = value.to_string(),
        // Every other group is a list's own — "in lists", "assigned by" — and My Tasks has no
        // equivalent. Quietly ignoring it would look like a control that does nothing.
        other => {
            return Response::failed(Failure::not_found("myTasksFilter", other));
        }
    }
    match account.set_my_tasks_preferences(&preferences).await {
        Ok(saved) => Response::ok(saved),
        Err(error) => Response::failed(error.into()),
    }
}
