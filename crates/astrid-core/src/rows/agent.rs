//! The agents the account's own harness can run, one row each.
//!
//! ## Why this is a projection and not the server's answer
//!
//! `GET /api/v1/users/me/agent-modes` describes each agent as `{ mailbox, email, mode, locked }`
//! — the coding-agent mailboxes the server knows, which since AWTD-937 come from one table on the
//! web (`lib/ai/harness-agents.ts`), so a CLI added there (Muse, August 2026) reaches this list
//! without a build here. The shell used to be handed that answer raw and read `id` and `name` off
//! it, which the answer has never carried: every row drew nameless, and joined the modes map on an
//! empty id, so every agent read as off. This projection is the fix, and the regression test in
//! `app::dispatch::tests` is what keeps it fixed.
//!
//! ## Locked means polling or off, never api or webhook
//!
//! A harness agent runs on the account's own machine and has no server executor. The server
//! refuses `api` and `webhook` for it with a 400 — which the web's AI Agents page used to draw as
//! three buttons that could only ever fail (task 42349da6). What it *can* be is `polling` (what it
//! already is) or `off` (not in use), because wanting the agent at all is a separate question from
//! who runs it. The row carries the modes a control may offer, so the shell draws a chooser that
//! can only ask for something the server will store — the same rule as web's `isModeSettableFor`.

use serde::Serialize;

use crate::services::AgentMode;

/// One agent, and the ways it may be set to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    /// The mailbox — the local part of the agent's address, and what a mode write names.
    pub id: String,
    /// What to call it on screen.
    pub name: String,
    /// The address comments and assignments show for it, when the server said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// How it runs now, as the server resolved it.
    pub mode: AgentMode,
    /// Whether it has no server-side executor and so can only be polled or switched off.
    pub locked: bool,
    /// The modes a control may offer for it. Never a mode the server would refuse.
    pub modes: Vec<AgentMode>,
}

/// Every mode, in the order the web's page lists them.
const ALL_MODES: [AgentMode; 4] = [
    AgentMode::Api,
    AgentMode::Polling,
    AgentMode::Webhook,
    AgentMode::Off,
];

/// The modes a locked agent can still be set to: it already polls, and it can be not in use.
const LOCKED_MODES: [AgentMode; 2] = [AgentMode::Polling, AgentMode::Off];

/// Which modes an agent may be set to (web `isModeSettableFor`).
pub fn settable_modes(locked: bool) -> &'static [AgentMode] {
    if locked {
        &LOCKED_MODES
    } else {
        &ALL_MODES
    }
}

/// How the product spells each mailbox. A mailbox not in the table is shown capitalised, so a
/// harness the server learns tomorrow has a name here today.
const LABELS: [(&str, &str); 6] = [
    ("claude", "Claude"),
    ("openai", "OpenAI"),
    ("codex", "Codex"),
    ("muse", "Muse"),
    ("copilot", "GitHub Copilot"),
    ("gemini", "Gemini"),
];

pub fn label_for(mailbox: &str) -> String {
    LABELS
        .iter()
        .find(|(id, _)| *id == mailbox)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| {
            let mut chars = mailbox.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
}

/// Project the `agent-modes` answer into rows.
///
/// Reads the `agents` list the route describes; a row's mode comes from the row itself, or — for
/// a server that only sends the `modes` map — from the map. An agent with no mode anywhere is off,
/// as the web resolves it.
pub fn rows(answer: &serde_json::Value) -> Vec<AgentRow> {
    let modes_map = answer.get("modes");
    let Some(agents) = answer.get("agents").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    agents
        .iter()
        .filter_map(|agent| {
            let mailbox = agent
                .get("mailbox")
                .or_else(|| agent.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|mailbox| !mailbox.is_empty())?;
            let locked = agent
                .get("locked")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let mode = agent
                .get("mode")
                .or_else(|| modes_map.and_then(|map| map.get(mailbox)))
                .and_then(|value| serde_json::from_value::<AgentMode>(value.clone()).ok())
                .unwrap_or(AgentMode::Off);
            Some(AgentRow {
                id: mailbox.to_string(),
                name: agent
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| label_for(mailbox)),
                email: agent
                    .get("email")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                mode,
                locked,
                modes: settable_modes(locked).to_vec(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The shape the route has answered with since 2026-08-24, and which the shell read by the
    /// wrong keys until this projection existed.
    #[test]
    fn the_servers_mailbox_rows_become_named_agents_with_their_mode() {
        let rows = rows(&json!({
            "agents": [
                { "mailbox": "claude", "email": "claude@astrid.cc", "mode": "api", "locked": false },
                { "mailbox": "codex", "email": "codex@astrid.cc", "mode": "polling", "locked": true },
            ],
            "modes": { "claude": "api", "codex": "polling" },
        }));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "claude");
        assert_eq!(rows[0].name, "Claude");
        assert_eq!(rows[0].email.as_deref(), Some("claude@astrid.cc"));
        assert_eq!(rows[0].mode, AgentMode::Api);
        assert_eq!(rows[1].name, "Codex");
        assert_eq!(rows[1].mode, AgentMode::Polling);
    }

    /// Muse is a CLI the account runs (Meta, August 2026): no executor on the server, so the row
    /// is locked and offers polling or off — never api or webhook, which the server would refuse
    /// (task 42349da6). It needs no build here: the server lists it and this names it.
    #[test]
    fn a_locked_agent_offers_only_polling_and_off() {
        let rows = rows(&json!({
            "agents": [{ "mailbox": "muse", "email": "muse@astrid.cc", "mode": "polling", "locked": true }],
            "modes": { "muse": "polling" },
        }));
        assert_eq!(rows[0].name, "Muse");
        assert!(rows[0].locked);
        assert_eq!(rows[0].modes, vec![AgentMode::Polling, AgentMode::Off]);

        let open = super::rows(&json!({
            "agents": [{ "mailbox": "claude", "mode": "api", "locked": false }],
        }));
        assert_eq!(
            open[0].modes,
            vec![
                AgentMode::Api,
                AgentMode::Polling,
                AgentMode::Webhook,
                AgentMode::Off
            ]
        );
    }

    /// A locked agent that has been switched off stays off: `off` is the one stored value the
    /// lock does not overwrite (task 42349da6).
    #[test]
    fn a_locked_agent_can_be_off() {
        let rows = rows(&json!({
            "agents": [{ "mailbox": "muse", "mode": "off", "locked": true }],
        }));
        assert_eq!(rows[0].mode, AgentMode::Off);
    }

    /// An older server sends the map and no per-row mode; an agent in neither is off, not
    /// unknown.
    #[test]
    fn the_mode_falls_back_to_the_map_and_then_to_off() {
        let rows = rows(&json!({
            "agents": [{ "mailbox": "claude" }, { "mailbox": "gemini" }],
            "modes": { "claude": "webhook" },
        }));
        assert_eq!(rows[0].mode, AgentMode::Webhook);
        assert_eq!(rows[1].mode, AgentMode::Off);
    }

    /// A mailbox the table has never heard of is still a row with a readable name.
    #[test]
    fn an_unknown_mailbox_is_named_by_capitalising_it() {
        assert_eq!(label_for("hermes"), "Hermes");
        assert_eq!(label_for("openai"), "OpenAI");
        assert_eq!(label_for(""), "");
        let rows = rows(&json!({ "agents": [{ "mailbox": "hermes", "mode": "polling" }] }));
        assert_eq!(rows[0].name, "Hermes");
    }

    /// No list, or a list of rows with no mailbox, is an empty hub — not a failure.
    #[test]
    fn nothing_listed_is_an_empty_hub() {
        assert!(rows(&json!({})).is_empty());
        assert!(rows(&json!({ "agents": [{ "email": "x@y" }] })).is_empty());
    }
}
