//! Projects — the status boards.
//!
//! Ported from `astrid-ios/Astrid App/Models/Project.swift`.
//!
//! A board exists when `project_id` is set on a [`TaskList`]. The project holds the shared
//! metadata and members; its `lists` array contains both regular (domain) lists and status lists,
//! one per board column. Inbox and Done stay virtual columns derived from task state and are never
//! stored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::date;
use super::list::TaskList;
use super::user::User;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<User>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<ProjectMember>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lists: Option<Vec<TaskList>>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<DateTime<Utc>>,
}

impl Project {
    pub fn display_color(&self) -> &str {
        self.color
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or("#3b82f6")
    }

    /// The board's columns, in the order the server gave them, excluding the domain lists that
    /// merely belong to the project.
    pub fn status_lists(&self) -> Vec<&TaskList> {
        self.lists
            .iter()
            .flatten()
            .filter(|list| list.is_status_list())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMember {
    /// Omitted by the embedded member arrays, like [`super::list::ListMember`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub user_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
}

impl ProjectMember {
    pub fn identity(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_column_is_a_status_list_and_a_domain_list_is_not() {
        let project: Project = serde_json::from_str(
            r#"{"id":"p1","name":"Launch","lists":[
                 {"id":"l1","name":"Ready","listType":"status","statusRole":"ready"},
                 {"id":"l2","name":"Marketing","listType":"regular"}
               ]}"#,
        )
        .expect("decodes");
        let columns: Vec<&str> = project
            .status_lists()
            .into_iter()
            .map(|l| l.id.as_str())
            .collect();
        assert_eq!(columns, vec!["l1"]);
    }

    #[test]
    fn a_project_needs_only_an_id() {
        let project: Project = serde_json::from_str(r#"{"id":"p1"}"#).expect("decodes");
        assert_eq!(project.display_color(), "#3b82f6");
        assert!(project.status_lists().is_empty());
    }

    #[test]
    fn an_embedded_member_without_an_id_is_keyed_by_its_user() {
        let member: ProjectMember =
            serde_json::from_str(r#"{"userId":"u1","role":"admin"}"#).expect("decodes");
        assert_eq!(member.identity(), "u1");
    }
}
