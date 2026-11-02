//! What both providers share: how one is named, what a link is, and connecting or linking either.
//!
//! Nothing here knows how a provider syncs. The per-provider names come from [`super::github`] and
//! [`super::google`], so this file stays a table rather than a pair of special cases, and the pass
//! that uses Google's half lives in [`super::google`].

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{github, google, ExternalSyncService};
use crate::api::endpoints;
use crate::external::ledger;
use crate::services::Result;

/// Which system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    GoogleTasks,
    GitHub,
}

impl Provider {
    /// The name the API knows it by.
    pub fn wire(self) -> &'static str {
        match self {
            Provider::GoogleTasks => google::WIRE,
            Provider::GitHub => github::WIRE,
        }
    }

    /// The name this crate's own records key it by — the ledger's `google`.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Provider::GoogleTasks => google::SLUG,
            Provider::GitHub => github::SLUG,
        }
    }
}

/// One list mirrored to one container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLink {
    pub id: String,
    #[serde(default)]
    pub astrid_list_id: String,
    #[serde(default)]
    pub remote_container_id: String,
    /// What the other side calls it — the server fetches and stores this on every link precisely
    /// so no client has to render an opaque Google tasklist id, or make a second call to find a
    /// name it was already told (task 6db679c3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_container_name: Option<String>,
    /// Where the last pull got to. Held by the server, committed by the client after it has
    /// applied a pass — so a client killed mid-pass re-pulls rather than skipping for ever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A container on the other side: a Google task list, or a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

impl ExternalSyncService {
    /// Which providers this account has connected.
    ///
    /// Also the moment the server's tombstones arrive — the deletions made on the web and on other
    /// devices — so this device stops re-importing what somebody deleted elsewhere.
    pub async fn status(&self) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::INTEGRATIONS);
        let answer = self.context.client.send(request).await?;
        self.merge_server_tombstones(&answer)?;
        Ok(answer)
    }

    /// The URL to open in a browser to connect a provider.
    ///
    /// The same hand-off shape as signing in: the browser is where somebody's Google password
    /// belongs, and an app that asked for it in its own window would be teaching a bad habit.
    pub async fn authorize_url(&self, provider: Provider) -> Result<String> {
        let request = self
            .context
            .client
            .get(endpoints::integration_authorize(provider.slug()));
        let answer = self.context.client.send(request).await?;
        Ok(answer
            .get("url")
            .or_else(|| answer.get("authorizeUrl"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string())
    }

    pub async fn disconnect(&self, provider: Provider) -> Result<()> {
        let request = self
            .context
            .client
            .delete(endpoints::INTEGRATIONS)
            .query("provider", Some(provider.wire().to_string()));
        self.context.client.send(request).await?;
        Ok(())
    }

    /// The containers on the other side, and — for Google — which is the default list.
    pub async fn containers(&self, provider: Provider) -> Result<(Vec<Container>, Option<String>)> {
        let path = match provider {
            Provider::GoogleTasks => google::CONTAINERS_PATH.to_string(),
            Provider::GitHub => github::CONTAINERS_PATH.to_string(),
        };
        let answer = self
            .context
            .client
            .send(self.context.client.get(path))
            .await?;
        let key = match provider {
            Provider::GoogleTasks => google::CONTAINERS_KEY,
            Provider::GitHub => github::CONTAINERS_KEY,
        };
        let containers = answer
            .get(key)
            .cloned()
            .map(serde_json::from_value::<Vec<Container>>)
            .transpose()
            .unwrap_or_default()
            .unwrap_or_default();
        let default = answer
            .get("defaultId")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        Ok((containers, default))
    }

    pub async fn links(&self, provider: Provider) -> Result<Vec<ExternalLink>> {
        let answer = self
            .context
            .client
            .send(
                self.context
                    .client
                    .get(endpoints::sync_links(provider.slug())),
            )
            .await?;
        let links: Vec<ExternalLink> = answer
            .get("links")
            .cloned()
            .map(serde_json::from_value::<Vec<ExternalLink>>)
            .transpose()
            .unwrap_or_default()
            .unwrap_or_default();
        // Written down for the same reason the task links are: deleting a list is local and
        // offline, and by then the link is gone.
        if provider == Provider::GoogleTasks {
            ledger::remember_links(
                &self.context.store,
                google::LIST_PROVIDER_KEY,
                google::SLUG,
                links.iter().map(|link| {
                    (
                        link.astrid_list_id.clone(),
                        link.remote_container_id.clone(),
                    )
                }),
            )?;
        }
        Ok(links)
    }

    /// Link a list to a container by hand.
    ///
    /// Which is a yes to that container, so it takes back an earlier "no" — on this device and on
    /// the account — or the all-lists modes go on refusing a list somebody has since chosen.
    /// (Apple `linkList`; AWTD2-56.)
    pub async fn link(
        &self,
        provider: Provider,
        list_id: &str,
        container_id: &str,
    ) -> Result<serde_json::Value> {
        let answer = self.create_link(provider, list_id, container_id).await?;
        if provider == Provider::GoogleTasks {
            self.clear_exclusion(container_id).await;
        }
        Ok(answer)
    }

    pub(super) async fn create_link(
        &self,
        provider: Provider,
        list_id: &str,
        container_id: &str,
    ) -> Result<serde_json::Value> {
        let request = self
            .context
            .client
            .post(endpoints::sync_links(provider.slug()))
            .value(json!({
                "astridListId": list_id,
                "remoteContainerId": container_id,
            }));
        Ok(self.context.client.send(request).await?)
    }

    pub async fn unlink(&self, provider: Provider, link_id: &str) -> Result<()> {
        let request = self
            .context
            .client
            .delete(endpoints::sync_links(provider.slug()))
            .query("linkId", Some(link_id.to_string()));
        self.context.client.send(request).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names are the server's, not ours. `GITHUB_ISSUES` is what `PROVIDER_CAPABILITY` in
    /// astrid-web's `/api/v1/integrations` route accepts and what the Apple clients send; anything
    /// else is a 400 on disconnect and a provider that never reads as connected.
    #[test]
    fn a_provider_travels_under_the_name_the_api_knows() {
        assert_eq!(Provider::GoogleTasks.wire(), "GOOGLE_TASKS");
        assert_eq!(Provider::GitHub.wire(), "GITHUB_ISSUES");
        assert_eq!(Provider::GoogleTasks.slug(), "google");
        assert_eq!(Provider::GitHub.slug(), "github");
    }
}
