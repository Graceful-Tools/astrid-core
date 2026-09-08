//! Which services the account can hold a key for, and whether it holds one.
//!
//! ## Why this is a projection and not the server's answer
//!
//! `GET /api/v1/users/me/ai-credentials` answers with a **map** of the services a key has already
//! been stored for — `{"keys": {"openai": {"hasKey": true, ...}}}`. A screen cannot be drawn from
//! that: a service with no key is simply absent, and a service with no key is exactly the row
//! somebody opens this screen to fill in.
//!
//! astrid-web solves it with a table of the four runtimes it supports (`components/agent-hub.tsx`,
//! `ROWS`) and looks each one up in the map. This is the same table, on this side of the boundary,
//! so the shell is handed rows rather than a map to reason about.
//!
//! The Windows settings panel crashed on open before this existed — it asked for a list and got an
//! object — which is the regression test at the bottom of this file.

use serde::Serialize;

/// One service the account can hold a key for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRow {
    /// What the API calls it, and what a save or a delete has to send back.
    pub service_id: String,
    /// What to call it on screen.
    pub name: String,
    /// Whether a key is stored. Never the key: the server does not answer with one.
    pub configured: bool,
}

/// The runtimes a key can be stored for, in the order astrid-web lists them.
///
/// Copilot is here even though it authenticates with GitHub rather than a pasted key — the screen
/// still has to say whether it is connected, and leaving it out would make "no Copilot row" mean
/// both "not connected" and "not supported".
const SERVICES: [(&str, &str); 4] = [
    ("claude", "Claude"),
    ("openai", "Codex"),
    ("copilot", "GitHub Copilot"),
    ("gemini", "Gemini"),
];

/// Project the credentials answer into one row per service.
///
/// Takes whatever the endpoint gave back. `keys` is the shape the v1 API answers with; `services`
/// is accepted because an older deployment answered with a list, and a client that fell over on
/// the older one would be a client that cannot be used to look at the newer.
pub fn rows(value: &serde_json::Value) -> Vec<CredentialRow> {
    if let Some(list) = value.get("services").and_then(|it| it.as_array()) {
        return list
            .iter()
            .map(|entry| CredentialRow {
                service_id: string(entry, "serviceId"),
                name: {
                    let name = string(entry, "name");
                    if name.is_empty() {
                        label_for(&string(entry, "serviceId"))
                    } else {
                        name
                    }
                },
                configured: entry
                    .get("configured")
                    .or_else(|| entry.get("hasKey"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
            .collect();
    }

    let keys = value.get("keys");
    SERVICES
        .iter()
        .map(|(id, label)| CredentialRow {
            service_id: (*id).to_string(),
            name: (*label).to_string(),
            configured: keys
                .and_then(|map| map.get(*id))
                .map(|entry| {
                    entry
                        .get("hasKey")
                        .and_then(serde_json::Value::as_bool)
                        // A service present in the map at all has a key; `hasKey` is how the API
                        // says so, and its absence is not a reason to call a stored key missing.
                        .unwrap_or(true)
                })
                .unwrap_or(false),
        })
        .collect()
}

/// What to call a service the server named but the table does not know.
fn label_for(service_id: &str) -> String {
    SERVICES
        .iter()
        .find(|(id, _)| *id == service_id)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| service_id.to_string())
}

fn string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The bug: the settings panel asked for a list of services and the server answered with a map
    /// of the ones already set up, so the panel could not be opened at all.
    #[test]
    fn the_keys_map_becomes_one_row_per_service() {
        let rows = rows(&json!({
            "keys": {
                "openai": { "hasKey": true, "keyPreview": "sk-...abc" },
            },
        }));

        assert_eq!(rows.len(), 4, "every service has a row, key or no key");
        let openai = rows.iter().find(|row| row.service_id == "openai").unwrap();
        assert!(openai.configured);
        assert_eq!(openai.name, "Codex", "the label astrid-web uses");
        let gemini = rows.iter().find(|row| row.service_id == "gemini").unwrap();
        assert!(
            !gemini.configured,
            "a service with no key is the row somebody came here to fill in"
        );
    }

    /// Nothing set up at all is still four rows. An empty screen would say the feature is missing.
    #[test]
    fn nothing_stored_still_draws_every_service() {
        let rows = rows(&json!({ "keys": {} }));
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| !row.configured));
    }

    /// The endpoint failing is answered with an empty object upstream, and that must not panic.
    #[test]
    fn an_answer_with_nothing_in_it_is_still_four_rows() {
        assert_eq!(rows(&json!({})).len(), 4);
    }

    /// An older deployment answered with a list. Falling over on it would leave a client unable to
    /// look at the account it needs to fix.
    #[test]
    fn a_list_of_services_is_taken_as_it_comes() {
        let rows = rows(&json!({
            "services": [
                { "serviceId": "claude", "configured": true },
                { "serviceId": "mystery", "name": "Mystery", "hasKey": true },
            ],
        }));

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].name, "Claude",
            "named from the table when it can be"
        );
        assert!(rows[0].configured);
        assert_eq!(rows[1].name, "Mystery");
        assert!(rows[1].configured, "hasKey means the same thing");
    }
}
