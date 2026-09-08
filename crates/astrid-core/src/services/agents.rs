//! The Agent Hub: how the AI agents run, and what they run with.
//!
//! Ports the service half of `astrid-ios/Astrid App/Core/Services/AgentHubModel.swift`, whose own
//! header says why it exists: "the views are thin: every rule that could drift between the two
//! platforms — which mode an ownership choice writes, that a write is optimistic and rolls back,
//! what 'needs setup' means — lives here."
//!
//! ## Ownership first, transport second
//!
//! An agent's mode is really two questions wearing one control. *Whose machine does the work* —
//! the account's own key, or Astrid's — and *how does the answer come back*. The modes the server
//! stores flatten both into one string, so the meaning of each is written down here rather than
//! being rediscovered from a picker:
//!
//! - `api` — Astrid runs it. Nothing to set up.
//! - `polling` and `webhook` — the account's own agent runs it, and needs a credential. Which of
//!   the two is a transport detail: whether the agent asks for work or is told about it.
//! - `off` — it does not run.
//!
//! ## A key is written, never read back
//!
//! The server answers with which services have a credential, never with the credential. So this
//! can say "OpenAI is set up" and cannot say what the key is — which is the right shape, and the
//! reason a screen shows a masked row rather than a text box with something in it.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;

/// How one agent runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Astrid runs it. Nothing to set up.
    Api,
    /// The account's own agent asks for work.
    Polling,
    /// The account's own agent is told about work.
    Webhook,
    /// It does not run.
    Off,
}

impl AgentMode {
    /// Whether this mode needs a credential of the account's own before it will do anything.
    ///
    /// The question a hub answers with "needs setup", and the reason it is here rather than in a
    /// view: two platforms deciding it separately is two definitions of "ready".
    pub fn needs_own_credential(self) -> bool {
        matches!(self, AgentMode::Polling | AgentMode::Webhook)
    }
}

/// One agent, and how it is set to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A service whose credential the account can hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    #[serde(alias = "id")]
    pub service_id: String,
    #[serde(default)]
    pub name: String,
    /// Whether a key is stored. Never the key: the server does not answer with it.
    #[serde(default)]
    pub configured: bool,
}

pub struct AgentService {
    context: Context,
}

impl AgentService {
    pub fn new(context: Context) -> Self {
        AgentService { context }
    }

    /// The agents, and the mode each is set to.
    pub async fn modes(&self) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::AGENT_MODES);
        Ok(self.context.client.send(request).await?)
    }

    pub async fn set_mode(&self, agent: &str, mode: AgentMode) -> Result<serde_json::Value> {
        let request = self
            .context
            .client
            .put(endpoints::AGENT_MODES)
            .value(json!({ "agent": agent, "mode": mode }));
        Ok(self.context.client.send(request).await?)
    }

    /// Which services have a credential stored.
    pub async fn credentials(&self) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::AI_CREDENTIALS);
        Ok(self.context.client.send(request).await?)
    }

    /// Store a key.
    ///
    /// Straight to the server and never into the cache: a key on this machine would be a copy of a
    /// secret that has no reason to be here, and the app works offline for everything except this.
    pub async fn save_credential(&self, service_id: &str, key: &str) -> Result<()> {
        let request = self
            .context
            .client
            .post(endpoints::AI_CREDENTIALS)
            .value(json!({ "serviceId": service_id, "apiKey": key }));
        self.context.client.send(request).await?;
        Ok(())
    }

    /// Ask the server whether a stored key actually works.
    pub async fn test_credential(&self, service_id: &str) -> Result<bool> {
        let request = self
            .context
            .client
            .post(endpoints::AI_CREDENTIALS_TEST)
            .value(json!({ "serviceId": service_id }));
        let answer = self.context.client.send(request).await?;
        Ok(answer
            .get("ok")
            .or_else(|| answer.get("success"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false))
    }

    pub async fn delete_credential(&self, service_id: &str) -> Result<()> {
        let request = self
            .context
            .client
            .delete(endpoints::AI_CREDENTIALS)
            .query("serviceId", Some(service_id.to_string()));
        self.context.client.send(request).await?;
        Ok(())
    }

    // ── The webhook: where an account's own agent is told about work ──────────────────────────

    /// The webhook settings, and the lists of events and agents a picker is built from.
    ///
    /// Answers for an account that has never configured one, which is why it is not a 404 on the
    /// server either: the screen needs the option lists before there is anything to option.
    pub async fn webhook_settings(&self) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::WEBHOOK_SETTINGS);
        Ok(self.context.client.send(request).await?)
    }

    /// Save where deliveries go, and what is delivered.
    ///
    /// `regenerate_secret` asks for a new signing secret. The server answers with it once and
    /// never again — it signs every delivery, and a server that echoed it would let any reader
    /// forge events into somebody's agent.
    pub async fn save_webhook(
        &self,
        url: &str,
        enabled: bool,
        events: &[String],
        agents: &[String],
        regenerate_secret: bool,
    ) -> Result<serde_json::Value> {
        let request = self
            .context
            .client
            .put(endpoints::WEBHOOK_SETTINGS)
            .value(json!({
                "webhookUrl": url,
                "enabled": enabled,
                "events": events,
                "agents": agents,
                "regenerateSecret": regenerate_secret,
            }));
        Ok(self.context.client.send(request).await?)
    }

    pub async fn delete_webhook(&self) -> Result<()> {
        let request = self.context.client.delete(endpoints::WEBHOOK_SETTINGS);
        self.context.client.send(request).await?;
        Ok(())
    }

    /// Send a `test.ping` to the configured URL.
    ///
    /// The only way to know a webhook works is to fire one: the URL is somebody else's server, and
    /// a saved URL that nothing has ever reached is a setting that looks configured and is not.
    pub async fn test_webhook(&self) -> Result<serde_json::Value> {
        let request = self.context.client.post(endpoints::WEBHOOK_SETTINGS);
        Ok(self.context.client.send(request).await?)
    }

    // ── Custom agents: the ones an account registers itself ───────────────────────────────────

    /// The agents this account has registered.
    ///
    /// Unwrapped here rather than in the shell: the envelope is the server's, and a screen that
    /// had to know its field name would be a screen that breaks when the server renames it.
    pub async fn custom_agents(&self) -> Result<Vec<serde_json::Value>> {
        let request = self.context.client.get(endpoints::CUSTOM_AGENTS);
        let answer = self.context.client.send(request).await?;
        Ok(answer
            .get("agents")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Register one, and answer with what the server made — including the credentials it will
    /// show only this once.
    ///
    /// `list_ids` limits what the agent can see. Absent means the account's lists, which is a
    /// bigger grant than most people want and is why the screen asks.
    pub async fn register_custom_agent(
        &self,
        name: &str,
        list_ids: Option<Vec<String>>,
    ) -> Result<serde_json::Value> {
        let mut body = json!({ "agentName": name });
        if let Some(list_ids) = list_ids {
            body["listIds"] = json!(list_ids);
        }
        let request = self
            .context
            .client
            .post(endpoints::CUSTOM_AGENT_REGISTER)
            .value(body);
        Ok(self.context.client.send(request).await?)
    }

    pub async fn delete_custom_agent(&self, id: &str) -> Result<()> {
        let request = self.context.client.delete(endpoints::custom_agent(id));
        self.context.client.send(request).await?;
        Ok(())
    }

    /// Whether the account's Copilot integration is connected.
    pub async fn copilot_status(&self) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::COPILOT_STATUS);
        Ok(self.context.client.send(request).await?)
    }

    /// The URL to open in a browser to connect Copilot.
    pub async fn copilot_authorize_url(&self) -> Result<String> {
        let request = self.context.client.get(endpoints::COPILOT_AUTHORIZE);
        let answer = self.context.client.send(request).await?;
        Ok(answer
            .get("url")
            .or_else(|| answer.get("authorizeUrl"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string())
    }

    pub async fn disconnect_copilot(&self) -> Result<()> {
        let request = self.context.client.delete(endpoints::COPILOT_STATUS);
        self.context.client.send(request).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Needs setup" is one definition, here, rather than one per platform.
    #[test]
    fn only_the_modes_that_run_somebody_elses_agent_need_a_credential() {
        assert!(!AgentMode::Api.needs_own_credential());
        assert!(!AgentMode::Off.needs_own_credential());
        assert!(AgentMode::Polling.needs_own_credential());
        assert!(AgentMode::Webhook.needs_own_credential());
    }

    /// The modes travel as the server spells them.
    #[test]
    fn a_mode_travels_lowercase() {
        assert_eq!(serde_json::to_value(AgentMode::Api).unwrap(), json!("api"));
        assert_eq!(
            serde_json::to_value(AgentMode::Webhook).unwrap(),
            json!("webhook")
        );
    }

    /// The server answers with which services are set up, never with the key.
    #[test]
    fn a_credential_row_carries_no_secret() {
        let row: Credential = serde_json::from_value(json!({
            "serviceId": "openai",
            "name": "OpenAI",
            "configured": true,
        }))
        .expect("decodes");
        assert!(row.configured);
        assert_eq!(row.service_id, "openai");

        // And the older shape, where the service is just `id`.
        let older: Credential =
            serde_json::from_value(json!({ "id": "anthropic" })).expect("decodes");
        assert_eq!(older.service_id, "anthropic");
        assert!(!older.configured);
    }
}
