//! Whether the GitHub App is installed, and where.
//!
//! ## Why this is a projection and not the server's answer
//!
//! `GET /api/v1/github/installations` answers a list and nothing else. "Connected" is not a field
//! on it: the web card decides it with `(installations?.length ?? 0) > 0`
//! (`components/Settings/GitHubConnectionCard.tsx`), and an empty list and a request that failed
//! look identical from there. Two platforms each deciding "connected" from a length is two places
//! that can disagree about what an absent list means, so it is decided here, once, and a screen is
//! handed the answer.
//!
//! A **suspended** installation is listed rather than hidden — GitHub keeps the row and stops
//! honouring it, so an account can have an installation and still have nothing work, which is
//! exactly the state somebody opens this screen to understand. The summary counts both, so a
//! message can distinguish "not installed" from "installed and suspended".

use serde::Serialize;

/// One installation of the GitHub App, as the server summarises it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationRow {
    /// GitHub's installation id. A number on the wire; kept as text because it is only ever an
    /// identifier here, and a client that rounded it would address the wrong installation.
    pub id: String,
    /// The user or organisation it is installed on.
    pub account_login: String,
    /// `User` or `Organization`, when the server said. Absent is not an error: an older
    /// deployment did not record it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_type: Option<String>,
    /// `all` or `selected` — whether the App can see every repository or a chosen few.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_selection: Option<String>,
    /// How many repositories it reaches.
    pub repo_count: u32,
    /// Suspended on GitHub: the row exists and nothing it grants works.
    pub suspended: bool,
}

/// What a GitHub App panel needs in one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubApp {
    /// Whether the App is installed anywhere at all.
    pub installed: bool,
    /// Whether at least one installation is actually honoured — installed, and not suspended.
    /// The question "will anything work", which `installed` alone does not answer.
    pub active: bool,
    /// Every installation, oldest access first, as the server ordered them.
    pub installations: Vec<InstallationRow>,
}

/// Project the installations answer.
///
/// Takes whatever the endpoint gave back. A failed request is answered upstream with an empty
/// object, which arrives here as "not installed" — the honest thing to draw, and the state the
/// panel exists to let somebody fix.
pub fn of(value: &serde_json::Value) -> GithubApp {
    let installations: Vec<InstallationRow> = value
        .get("installations")
        .and_then(|it| it.as_array())
        .map(|list| list.iter().map(row).collect())
        .unwrap_or_default();
    GithubApp {
        installed: !installations.is_empty(),
        active: installations.iter().any(|it| !it.suspended),
        installations,
    }
}

fn row(entry: &serde_json::Value) -> InstallationRow {
    InstallationRow {
        id: match entry.get("id") {
            // The server sends a number; a client that had already been handed a string should
            // not then fail to find the installation it is looking at.
            Some(serde_json::Value::Number(number)) => number.to_string(),
            Some(serde_json::Value::String(text)) => text.clone(),
            _ => String::new(),
        },
        account_login: optional_string(entry, "accountLogin").unwrap_or_default(),
        account_type: optional_string(entry, "accountType"),
        repository_selection: optional_string(entry, "repositorySelection"),
        repo_count: entry
            .get("repoCount")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        suspended: entry
            .get("suspended")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }
}

/// A string field, with `null` and an empty string both read as absent.
fn optional_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|it| !it.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Task 959b24b5: the status a screen shows is the length of a list, decided here rather than
    /// in each shell.
    #[test]
    fn an_installation_means_the_app_is_installed() {
        let app = of(&json!({
            "installations": [{
                "id": 54872311,
                "accountLogin": "Graceful-Tools",
                "accountType": "Organization",
                "repositorySelection": "selected",
                "repoCount": 4,
                "suspended": false,
            }],
        }));

        assert!(app.installed);
        assert!(app.active);
        assert_eq!(app.installations.len(), 1);
        let row = &app.installations[0];
        assert_eq!(row.id, "54872311", "an identifier, never arithmetic");
        assert_eq!(row.account_login, "Graceful-Tools");
        assert_eq!(row.account_type.as_deref(), Some("Organization"));
        assert_eq!(row.repository_selection.as_deref(), Some("selected"));
        assert_eq!(row.repo_count, 4);
        assert!(!row.suspended);
    }

    /// Installed and suspended is its own state: the row exists and nothing it grants works. A
    /// panel that only knew `installed` would tell somebody everything was fine.
    #[test]
    fn a_suspended_installation_is_installed_but_not_active() {
        let app = of(&json!({
            "installations": [
                { "id": 1, "accountLogin": "jonparis", "repoCount": 0, "suspended": true },
            ],
        }));

        assert!(app.installed);
        assert!(!app.active, "suspended grants nothing");
        assert!(app.installations[0].suspended);
    }

    /// One good installation beside a suspended one is still a working connection.
    #[test]
    fn one_honoured_installation_is_enough_to_be_active() {
        let app = of(&json!({
            "installations": [
                { "id": 1, "accountLogin": "old-org", "repoCount": 2, "suspended": true },
                { "id": 2, "accountLogin": "new-org", "repoCount": 7, "suspended": false },
            ],
        }));
        assert!(app.active);
        assert_eq!(app.installations.len(), 2, "both are listed, never hidden");
    }

    /// Nothing installed, and a request that failed (answered upstream with an empty object),
    /// both draw as "not installed" rather than panicking or looking connected.
    #[test]
    fn nothing_installed_is_not_an_error() {
        for value in [json!({ "installations": [] }), json!({})] {
            let app = of(&value);
            assert!(!app.installed, "{value}");
            assert!(!app.active);
            assert!(app.installations.is_empty());
        }
    }

    /// An older deployment did not record the account type or the selection. Those rows still draw.
    #[test]
    fn a_row_missing_what_an_older_server_did_not_record_still_draws() {
        let app = of(&json!({
            "installations": [{ "id": "7", "accountLogin": "jonparis", "accountType": null }],
        }));
        let row = &app.installations[0];
        assert_eq!(row.id, "7", "a string id is taken as it comes");
        assert_eq!(row.account_type, None);
        assert_eq!(row.repository_selection, None);
        assert_eq!(row.repo_count, 0);
        assert!(!row.suspended, "absent is not suspended");
    }
}
