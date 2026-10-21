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
    /// Enough of the stored key to tell which one it is, exactly as the server masked it —
    /// `first4***last4`, or `***` for a key of eight characters or fewer, or the first twenty
    /// characters of an OpenClaw gateway URL. `None` when no key is stored.
    ///
    /// This is a preview, never the key. The masking is the server's
    /// (`app/api/v1/users/me/ai-credentials/route.ts`, `getKeyPreview`) and is not recomputed
    /// here: a client that re-masked would be a client that could disagree with the screen
    /// somebody pasted the key into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_preview: Option<String>,
    /// Whether the stored key worked the last time it was tested.
    ///
    /// Three states, not two. `None` is **never tested**, which is not "broken": a key saved a
    /// minute ago and a key the provider has revoked are different rows, and a screen that drew
    /// both as a red cross would send somebody to re-paste a key that is fine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_valid: Option<bool>,
    /// When it was last tested, as the server timestamped it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tested: Option<String>,
    /// What went wrong the last time it was tested, when the server kept a reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
            .map(|entry| {
                let service_id = string(entry, "serviceId");
                let name = string(entry, "name");
                CredentialRow {
                    name: if name.is_empty() {
                        label_for(&service_id)
                    } else {
                        name
                    },
                    service_id,
                    configured: entry
                        .get("configured")
                        .or_else(|| entry.get("hasKey"))
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    ..detail(Some(entry))
                }
            })
            .collect();
    }

    let keys = value.get("keys");
    SERVICES
        .iter()
        .map(|(id, label)| {
            let entry = keys.and_then(|map| map.get(*id));
            CredentialRow {
                service_id: (*id).to_string(),
                name: (*label).to_string(),
                configured: entry
                    .map(|entry| {
                        entry
                            .get("hasKey")
                            .and_then(serde_json::Value::as_bool)
                            // A service present in the map at all has a key; `hasKey` is how the
                            // API says so, and its absence is not a reason to call a stored key
                            // missing.
                            .unwrap_or(true)
                    })
                    .unwrap_or(false),
                ..detail(entry)
            }
        })
        .collect()
}

/// The preview and the last test result off one service's entry, whichever shape it arrived in.
///
/// Absent for a service with no entry, which is how a row somebody has not filled in yet carries
/// no preview rather than an empty one. `serviceId`, `name` and `configured` are the caller's:
/// only those differ between the two shapes.
fn detail(entry: Option<&serde_json::Value>) -> CredentialRow {
    let Some(entry) = entry else {
        return CredentialRow {
            service_id: String::new(),
            name: String::new(),
            configured: false,
            key_preview: None,
            is_valid: None,
            last_tested: None,
            error: None,
        };
    };
    CredentialRow {
        service_id: String::new(),
        name: String::new(),
        configured: false,
        key_preview: optional_string(entry, "keyPreview"),
        // Read as a tri-state deliberately: the server sends `isValid: null` for a key it has
        // never tested, and `as_bool` on null is `None`, which is exactly the answer wanted.
        is_valid: entry.get("isValid").and_then(serde_json::Value::as_bool),
        last_tested: optional_string(entry, "lastTested"),
        error: optional_string(entry, "error"),
    }
}

/// A string field, with an empty one read as absent: an empty preview is no preview.
fn optional_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|it| !it.is_empty())
        .map(str::to_string)
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

    /// Task 959b24b5: the preview and the last test result come from the server's answer, which
    /// has carried both all along. A screen cannot tell two saved keys apart without the preview.
    #[test]
    fn a_stored_key_carries_its_preview_and_what_testing_it_found() {
        let rows = rows(&json!({
            "keys": {
                "openai": {
                    "hasKey": true,
                    "keyPreview": "sk-p***4f9a",
                    "isValid": false,
                    "lastTested": "2026-10-20T11:02:00.000Z",
                    "error": "Incorrect API key provided",
                },
            },
        }));

        let openai = rows.iter().find(|row| row.service_id == "openai").unwrap();
        assert_eq!(openai.key_preview.as_deref(), Some("sk-p***4f9a"));
        assert_eq!(openai.is_valid, Some(false));
        assert_eq!(
            openai.last_tested.as_deref(),
            Some("2026-10-20T11:02:00.000Z")
        );
        assert_eq!(openai.error.as_deref(), Some("Incorrect API key provided"));
    }

    /// Never tested is not broken. The server sends `isValid: null` for a key it has not tried,
    /// and a row that read that as `false` would send somebody to re-paste a key that is fine.
    #[test]
    fn a_key_the_server_has_never_tested_is_neither_valid_nor_invalid() {
        let rows = rows(&json!({
            "keys": {
                "claude": { "hasKey": true, "keyPreview": "sk-a***9001", "isValid": null, "lastTested": null },
            },
        }));

        let claude = rows.iter().find(|row| row.service_id == "claude").unwrap();
        assert!(claude.configured);
        assert_eq!(claude.key_preview.as_deref(), Some("sk-a***9001"));
        assert_eq!(
            claude.is_valid, None,
            "null is never tested, which is not false"
        );
        assert_eq!(claude.last_tested, None);
    }

    /// A service nobody has filled in has nothing to preview, rather than an empty preview.
    #[test]
    fn a_service_with_no_key_has_no_preview_at_all() {
        let rows = rows(&json!({ "keys": {} }));
        assert!(rows
            .iter()
            .all(|row| row.key_preview.is_none() && row.is_valid.is_none()));

        // And the fields are left out of the wire shape entirely, so a shell cannot draw a
        // masked row for a service that has no key.
        let wire = serde_json::to_value(&rows[0]).unwrap();
        assert!(wire.get("keyPreview").is_none());
        assert!(wire.get("isValid").is_none());
    }

    /// The server masks a key it cannot decrypt as `***` and says why. That is a real row — it is
    /// how somebody finds out the key needs pasting again — so it must not be dropped.
    #[test]
    fn a_key_that_would_not_decrypt_still_draws_with_its_reason() {
        let rows = rows(&json!({
            "keys": {
                "gemini": { "hasKey": true, "keyPreview": "***", "isValid": false, "error": "Failed to decrypt key" },
            },
        }));

        let gemini = rows.iter().find(|row| row.service_id == "gemini").unwrap();
        assert_eq!(gemini.key_preview.as_deref(), Some("***"));
        assert_eq!(gemini.error.as_deref(), Some("Failed to decrypt key"));
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
