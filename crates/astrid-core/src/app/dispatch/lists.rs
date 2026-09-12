//! A list's settings, members and agent binding.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these.

use super::*;

/// Who a list is shared with, and what this account may do about it.
///
/// The permissions come back with the members rather than being worked out in the shell: whether
/// somebody may change a role is [`crate::permissions`], and a screen that decided it itself would
/// be the fourth implementation of a rule whose failure mode is a control that 403s — or one that
/// quietly is not offered to somebody who should have it.
///
/// From the cache, and nothing else: one network call in here used to take the list's name,
/// colour, privacy, columns and permissions — all cache-derived — down with it whenever the
/// server could not be reached. The roster is refreshed by `RefreshListMembers`, which answers
/// with this same shape once the server has spoken.
pub(super) fn list_members(app: &App, list_id: &str) -> Response {
    let list = match app.context.lists().list(list_id) {
        Ok(Some(list)) => list,
        Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let members = match app.context.lists().cached_members(list_id) {
        Ok(members) => members,
        Err(error) => return Response::failed(error.into()),
    };

    // The board's editable columns, for the Statuses section (task e5214fba): the defaults,
    // renamed or not, and the board's own — never Inbox or Done, which are derived.
    let statuses: Vec<serde_json::Value> = match &list.project_id {
        Some(project_id) => {
            let custom_states = app
                .store
                .projects()
                .unwrap_or_default()
                .into_iter()
                .find(|project| &project.id == project_id)
                .and_then(|project| project.custom_states);
            crate::board::columns(custom_states.as_ref())
                .into_iter()
                .filter(|column| column.kind == crate::board::ColumnKind::Status)
                .map(|column| {
                    serde_json::json!({
                        "id": column.id,
                        "name": column.name,
                        "isDefault": crate::board::is_default_role(&column.id),
                    })
                })
                .collect()
        }
        None => Vec::new(),
    };

    let me = app.context.account().current_user_id().ok().flatten();
    let lists = app.context.lists();
    let (can_manage_members, can_manage_list, can_delete) = match me.as_deref() {
        Some(me) => (
            lists.can_manage_members(me, &list),
            lists.can_manage(me, &list),
            lists.can_delete(me, &list),
        ),
        None => (false, false, false),
    };

    Response::ok(serde_json::json!({
        "listId": list.id,
        "name": list.name,
        "ownerId": list.owner_id,
        "projectId": list.project_id,
        "statuses": statuses,
        // The coding-agent binding (task f44b4a0c): which agent picks this list's tasks up, and
        // which repository it commits to. The choices are a separate, networked answer.
        "defaultAgentId": list.ai_agent_config.as_ref().and_then(|config| config.default_agent_id.clone()),
        "githubRepositoryId": list.github_repository_id,
        // How the list looks and who can see it (task 53780e75). The colour is the one every
        // screen draws — sidebar mark, row chip, detail chip — and the choices are the web's
        // palette, so a colour picked here is a colour the web offers too. Privacy comes back
        // as stored, absent when the server's thinner responses left it out, so the flyout can
        // say "unknown" rather than guessing "private".
        "color": list.display_color(),
        "colorChoices": rows::list_picks::LIST_COLOR_PALETTE,
        "privacy": list.privacy,
        // The list's picture as stored (task 3a913e52); where to draw it from is `listImage`.
        "imageUrl": list.image_url,
        "isFavorite": list.is_favorite.unwrap_or(false),
        // What a task added to this list starts as (task c4102c67), as the list stores it: an
        // absent assignee is the creator, "unassigned" is nobody, an id is that member.
        "defaults": {
            "assigneeId": list.default_assignee_id,
            "priority": list.default_priority.unwrap_or(0),
            "repeating": list.default_repeating.clone().unwrap_or_else(|| "never".into()),
            "dueDate": list.default_due_date.clone().unwrap_or_else(|| "none".into()),
            "dueTime": list.default_due_time,
        },
        "canManageMembers": can_manage_members,
        "canManageList": can_manage_list,
        "canDeleteList": can_delete,
        // Leaving is for a list somebody else owns: an owner leaving their own list would strand
        // it, which is what deleting is for.
        "canLeave": me.is_some() && list.owner_id.as_deref() != me.as_deref(),
        "currentUserId": me,
        "members": members,
    }))
}

/// The choices for a list's coding-agent settings (task f44b4a0c).
///
/// Both halves reach the network. The agents are what the account may run — the web's
/// `available-agents` — and the repositories are what its GitHub connection can see. A GitHub
/// that is not connected answers with an error, which is not a failure here: it is "no
/// repositories, and say why".
pub(super) async fn list_agent_options(app: &App, list_id: &str) -> Response {
    let list = match app.context.lists().list(list_id) {
        Ok(Some(list)) => list,
        Ok(None) => return Response::failed(Failure::not_found("list", list_id)),
        Err(error) => return Response::failed(error.into()),
    };
    let client = &app.context.client;

    let agents: Vec<serde_json::Value> = match client
        .send(client.get(crate::api::endpoints::AVAILABLE_AGENTS))
        .await
    {
        Ok(answer) => answer
            .get("agents")
            .and_then(|value| value.as_array())
            .map(|agents| {
                agents
                    .iter()
                    .filter_map(|agent| {
                        let id = agent.get("id")?.as_str()?;
                        let name = agent
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or(id);
                        Some(serde_json::json!({ "id": id, "name": name }))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(error @ crate::api::ApiError::Unauthorized) => {
            return Response::failed(crate::services::ServiceError::Api(error).into())
        }
        Err(_) => Vec::new(),
    };

    let (repositories, github_connected) = match client
        .send(client.get(crate::api::endpoints::GITHUB_REPOSITORIES))
        .await
    {
        Ok(answer) => (
            answer
                .get("repositories")
                .and_then(|value| value.as_array())
                .map(|repositories| {
                    repositories
                        .iter()
                        .filter_map(|repository| {
                            let full_name = repository.get("fullName")?.as_str()?;
                            let name = repository
                                .get("name")
                                .and_then(|value| value.as_str())
                                .unwrap_or(full_name);
                            Some(serde_json::json!({ "fullName": full_name, "name": name }))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            true,
        ),
        Err(_) => (Vec::new(), false),
    };

    Response::ok(serde_json::json!({
        "defaultAgentId": list.ai_agent_config.as_ref().and_then(|config| config.default_agent_id.clone()),
        "githubRepositoryId": list.github_repository_id,
        "agents": agents,
        "repositories": repositories,
        "githubConnected": github_connected,
    }))
}

/// A board-status write's answer: the state that changed, or the web's own refusal as a bad
/// request (task e5214fba).
pub(super) fn status_outcome(
    outcome: crate::services::Result<crate::services::StatusOutcome>,
) -> Response {
    match outcome {
        Ok(crate::services::StatusOutcome::Written(state)) => {
            Response::ok(serde_json::json!({ "state": state }))
        }
        Ok(crate::services::StatusOutcome::Refused(refused)) => {
            Response::failed(Failure::bad_request(&refused.message))
        }
        Err(error) => Response::failed(error.into()),
    }
}

pub(super) fn list_changes_from_json(
    value: &serde_json::Value,
) -> Result<crate::services::ListChanges, Failure> {
    let object = value
        .as_object()
        .ok_or_else(|| Failure::bad_request("changes must be an object"))?;
    let mut changes = crate::services::ListChanges::default();

    for (key, value) in object {
        match key.as_str() {
            "name" => changes.name = value.as_str().map(str::to_string),
            "color" => changes.color = Some(value.as_str().map(str::to_string)),
            "description" => changes.description = Some(value.as_str().map(str::to_string)),
            "privacy" => changes.privacy = serde_json::from_value(value.clone()).ok(),
            "imageUrl" => changes.image_url = Some(value.as_str().map(str::to_string)),
            "isFavorite" => changes.is_favorite = value.as_bool(),
            "favoriteOrder" => changes.favorite_order = Some(value.as_i64()),
            "sortBy" => changes.sort_by = Some(value.as_str().map(str::to_string)),
            "manualSortOrder" => {
                changes.manual_sort_order = serde_json::from_value(value.clone()).ok()
            }
            "showSubtasks" => changes.show_subtasks = value.as_bool(),
            "defaultAssigneeId" => {
                changes.default_assignee_id = Some(value.as_str().map(str::to_string))
            }
            "defaultPriority" => changes.default_priority = Some(value.as_i64()),
            "defaultDueTime" => changes.default_due_time = Some(value.as_str().map(str::to_string)),
            "defaultRepeating" => {
                changes.default_repeating = Some(value.as_str().map(str::to_string))
            }
            "defaultDueDate" => changes.default_due_date = Some(value.as_str().map(str::to_string)),
            "defaultAgentId" => changes.default_agent_id = Some(value.as_str().map(str::to_string)),
            "githubRepositoryId" => {
                changes.github_repository_id = Some(value.as_str().map(str::to_string))
            }
            "filterPriority" => changes.filter_priority = Some(value.as_str().map(str::to_string)),
            "filterDueDate" => changes.filter_due_date = Some(value.as_str().map(str::to_string)),
            "filterAssignee" => changes.filter_assignee = Some(value.as_str().map(str::to_string)),
            "filterRepeating" => {
                changes.filter_repeating = Some(value.as_str().map(str::to_string))
            }
            "filterAssignedBy" => {
                changes.filter_assigned_by = Some(value.as_str().map(str::to_string))
            }
            "filterInLists" => changes.filter_in_lists = Some(value.as_str().map(str::to_string)),
            "filterCompletion" => {
                changes.filter_completion = Some(value.as_str().map(str::to_string))
            }
            "recentlyCompletedWindow" => {
                changes.recently_completed_window =
                    Some(serde_json::from_value(value.clone()).unwrap_or(None))
            }
            _ => {}
        }
    }
    Ok(changes)
}
