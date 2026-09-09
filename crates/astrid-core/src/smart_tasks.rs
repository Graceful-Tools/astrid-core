//! The account's task defaults and its task-detail layout, as the server stores them
//! (task c0f3db19): web's Task Settings page, and the layout switch on its Appearance page — both
//! `GET` / `PATCH /api/v1/users/me/smart-tasks`.
//!
//! The defaults are the web's (`TasksSettings.tsx`: `?? true`, `|| "1_week"`, `|| "17:00"`;
//! `lib/task-display-mode.ts`: everyone defaults to list), and the values a field may take are the
//! server's (`VALID_OFFSETS`, `TIME_RE`, `VALID_SUBTASK_DISPLAY`, `isTaskDisplayMode`), checked
//! here before a request goes — a write the server would refuse should be refused in the room, not
//! by a 400 after the control has already moved.
//!
//! Note what the due-date and due-time defaults are FOR: tasks created by email. The web's quick
//! add does not read them, so neither does this client's — a default applied on one client and not
//! the other is exactly the kind of divergence `docs/CONTRACTS.md` exists to prevent.

use crate::rows::DisplayMode;
use serde::Serialize;
use serde_json::Value;

/// How far out an emailed task is due, as the server spells them.
pub const DUE_OFFSETS: [&str; 4] = ["none", "1_day", "3_days", "1_week"];
/// The times of day the web offers for an emailed task.
pub const DUE_TIMES: [&str; 4] = ["09:00", "12:00", "17:00", "20:00"];
/// The two ways the list can lay out subtasks.
pub const SUBTASK_DISPLAYS: [&str; 2] = ["indented", "under_parent"];

/// The settings, shaped: every field present, defaulted as the web defaults it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartTaskSettings {
    pub email_to_task_enabled: bool,
    pub default_task_due_offset: String,
    pub default_due_time: String,
    /// `list` or `project` — the wire value, normalised the way `DisplayMode::from_stored` does.
    pub task_display_mode: String,
    pub subtask_display: String,
    pub smart_task_creation_enabled: bool,
}

impl SmartTaskSettings {
    /// Shape whatever the cache or the server holds. Absent, null or unrecognised fields take the
    /// web's defaults, so a server older than a field and a user who never chose read the same.
    pub fn from_stored(stored: &Value) -> Self {
        let text = |key: &str, fallback: &str| -> String {
            stored
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(fallback)
                .to_string()
        };
        let flag = |key: &str| stored.get(key).and_then(Value::as_bool).unwrap_or(true);
        let subtask_display = text("subtaskDisplay", "indented");
        SmartTaskSettings {
            email_to_task_enabled: flag("emailToTaskEnabled"),
            default_task_due_offset: text("defaultTaskDueOffset", "1_week"),
            default_due_time: text("defaultDueTime", "17:00"),
            task_display_mode: DisplayMode::from_stored(
                stored.get("taskDisplayMode").and_then(Value::as_str),
            )
            .wire_value()
            .to_string(),
            subtask_display: if SUBTASK_DISPLAYS.contains(&subtask_display.as_str()) {
                subtask_display
            } else {
                "indented".to_string()
            },
            smart_task_creation_enabled: flag("smartTaskCreationEnabled"),
        }
    }

    /// The layout rows and the detail draw with.
    pub fn display_mode(&self) -> DisplayMode {
        DisplayMode::from_stored(Some(&self.task_display_mode))
    }
}

/// Refuse what the server would refuse, in its words where it has them. The keys are the server's
/// `ALLOWED` list; anything else is not a task setting and must not be sent as one.
pub fn validate(changes: &Value) -> Result<(), String> {
    let Some(fields) = changes.as_object() else {
        return Err("changes must be an object of settings".to_string());
    };
    for (key, value) in fields {
        match key.as_str() {
            "emailToTaskEnabled" | "smartTaskCreationEnabled" => {
                if !value.is_boolean() {
                    return Err(format!("{key} must be true or false"));
                }
            }
            "defaultTaskDueOffset" => {
                if !value.as_str().is_some_and(|v| DUE_OFFSETS.contains(&v)) {
                    return Err("Invalid defaultTaskDueOffset value".to_string());
                }
            }
            "defaultDueTime" => {
                if !value.as_str().is_some_and(is_time) {
                    return Err("defaultDueTime must be HH:MM".to_string());
                }
            }
            "subtaskDisplay" => {
                if !value
                    .as_str()
                    .is_some_and(|v| SUBTASK_DISPLAYS.contains(&v))
                {
                    return Err("Invalid subtaskDisplay value".to_string());
                }
            }
            "taskDisplayMode" => {
                if !value
                    .as_str()
                    .is_some_and(|v| v == "list" || v == "project")
                {
                    return Err("taskDisplayMode must be list or project".to_string());
                }
            }
            "emailToTaskListId" => {
                if !(value.is_string() || value.is_null()) {
                    return Err("emailToTaskListId must be a list id or null".to_string());
                }
            }
            other => return Err(format!("{other} is not a task setting")),
        }
    }
    Ok(())
}

/// `HH:MM`, twenty-four hour — the server's `TIME_RE`.
fn is_time(value: &str) -> bool {
    let Some((hours, minutes)) = value.split_once(':') else {
        return false;
    };
    hours.len() == 2
        && minutes.len() == 2
        && hours.chars().all(|c| c.is_ascii_digit())
        && minutes.chars().all(|c| c.is_ascii_digit())
        && hours.parse::<u8>().is_ok_and(|h| h < 24)
        && minutes.parse::<u8>().is_ok_and(|m| m < 60)
}

/// One entry a combo offers: the value the server takes, and the key the shell words it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub value: String,
    pub title_key: String,
    /// A longer line under the title, when the choice needs one — the layouts do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_key: Option<String>,
}

fn choice(value: &str, title_key: String) -> Choice {
    Choice {
        value: value.to_string(),
        title_key,
        description_key: None,
    }
}

/// The due-date offsets, in the web's order.
pub fn offset_choices() -> Vec<Choice> {
    DUE_OFFSETS
        .iter()
        .map(|offset| choice(offset, format!("smart.offset.{offset}")))
        .collect()
}

/// The due times, in the web's order. The key replaces the colon, which a resource name cannot hold.
pub fn time_choices() -> Vec<Choice> {
    DUE_TIMES
        .iter()
        .map(|time| choice(time, format!("smart.time.{}", time.replace(':', "_"))))
        .collect()
}

/// The two task-detail layouts, each with the line the web shows under it.
pub fn layout_choices() -> Vec<Choice> {
    [DisplayMode::List, DisplayMode::Project]
        .iter()
        .map(|mode| {
            let name = mode.wire_value();
            Choice {
                value: name.to_string(),
                title_key: format!("smart.layout.{name}"),
                description_key: Some(format!("smart.layout.{name}_desc")),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Nothing stored reads as the web's defaults; what is stored reads as itself; what the server
    /// would refuse is refused here first (task c0f3db19).
    #[test]
    fn settings_take_the_web_s_defaults_and_refuse_what_the_server_would_task_c0f3db19() {
        let defaults = SmartTaskSettings::from_stored(&json!({}));
        assert!(defaults.email_to_task_enabled);
        assert_eq!(defaults.default_task_due_offset, "1_week");
        assert_eq!(defaults.default_due_time, "17:00");
        assert_eq!(defaults.task_display_mode, "list");
        assert_eq!(defaults.subtask_display, "indented");
        assert_eq!(defaults.display_mode(), DisplayMode::List);

        let stored = SmartTaskSettings::from_stored(&json!({
            "emailToTaskEnabled": false, "defaultTaskDueOffset": "3_days",
            "defaultDueTime": "09:00", "taskDisplayMode": "PROJECT", "subtaskDisplay": "sideways"
        }));
        assert!(!stored.email_to_task_enabled);
        assert_eq!(stored.default_task_due_offset, "3_days");
        assert_eq!(stored.default_due_time, "09:00");
        assert_eq!(
            stored.task_display_mode, "project",
            "normalised as the web normalises"
        );
        assert_eq!(
            stored.subtask_display, "indented",
            "an unknown layout is the safe one"
        );
        assert_eq!(stored.display_mode(), DisplayMode::Project);

        assert!(
            validate(&json!({ "taskDisplayMode": "project", "defaultDueTime": "20:00" })).is_ok()
        );
        assert_eq!(
            validate(&json!({ "defaultTaskDueOffset": "2_weeks" })),
            Err("Invalid defaultTaskDueOffset value".to_string())
        );
        assert!(validate(&json!({ "defaultDueTime": "25:00" })).is_err());
        assert!(validate(&json!({ "defaultDueTime": "9:00" })).is_err());
        assert!(validate(&json!({ "taskDisplayMode": "board" })).is_err());
        assert!(validate(&json!({ "emailToTaskEnabled": "yes" })).is_err());
        assert!(validate(&json!({ "favouriteColour": "blue" })).is_err());
        assert!(validate(&json!("project")).is_err());
    }

    #[test]
    fn the_choices_are_the_web_s_in_the_web_s_order() {
        let offsets = offset_choices();
        let values: Vec<&str> = offsets.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, DUE_OFFSETS);
        assert_eq!(offset_choices()[0].title_key, "smart.offset.none");
        assert_eq!(time_choices()[2].title_key, "smart.time.17_00");
        let layouts = layout_choices();
        assert_eq!(layouts[0].value, "list");
        assert_eq!(
            layouts[1].description_key.as_deref(),
            Some("smart.layout.project_desc")
        );
    }
}
