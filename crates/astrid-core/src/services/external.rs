//! Connecting other people's task systems, and mirroring Google Tasks.
//!
//! Ports the client half of `astrid-ios/Astrid App/Core/Sync/`. The decisions it makes live in
//! [`crate::external`] with their reasons and their tests; this is the pass that uses them.
//!
//! ## Two providers, two shapes
//!
//! **GitHub is the server's job.** A cron on astrid-web pulls issues and pushes tasks, so a client
//! only configures it: connect the account, link a list to a repository. A list linked from here
//! syncs whether or not this app is running.
//!
//! **Google Tasks is the client's job.** The server holds the tokens and proxies the API, but the
//! pulling and pushing are the client's — "clients poll on foreground/nudge", as the route says.
//!
//! ## What this pass deliberately does not do yet
//!
//! It does not delete a remote twin when a task is deleted here. Apple captures the link at delete
//! time in a local ledger, because the server's link row cascades away with the task and the
//! evidence is gone by the next pass. Until that ledger exists here, a deleted task simply stops
//! being pushed and its Google twin stays — which is the conservative failure: nothing is lost,
//! and somebody can delete it there. Written up in `docs/PARITY.md` rather than left as a surprise.
//!
//! It also acts on a remote deletion only when Google says so explicitly (`deleted`), never on
//! absence from a page. A cursor pull is not a full listing, and deleting local tasks because a
//! page did not mention them is how a dropped request wipes somebody's list.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;
use crate::external::decisions::{self, PullOutcome};
use crate::model::{date, Task};

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
            Provider::GoogleTasks => "GOOGLE_TASKS",
            Provider::GitHub => "GITHUB",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Provider::GoogleTasks => "google",
            Provider::GitHub => "github",
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

/// One remote item, as the proxy hands it over.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteItem {
    remote_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    due_date: Option<String>,
    #[serde(default)]
    deleted: Option<bool>,
    #[serde(default)]
    parent: Option<String>,
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassReport {
    pub pulled: usize,
    pub applied: usize,
    pub deleted_locally: usize,
    pub pushed: usize,
    /// True when the page was cut short, so nothing may be inferred from absence.
    pub truncated: bool,
}

pub struct ExternalSyncService {
    context: Context,
}

impl ExternalSyncService {
    pub fn new(context: Context) -> Self {
        ExternalSyncService { context }
    }

    /// Which providers this account has connected.
    pub async fn status(&self) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::INTEGRATIONS);
        Ok(self.context.client.send(request).await?)
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
            Provider::GoogleTasks => endpoints::GOOGLE_TASKLISTS.to_string(),
            Provider::GitHub => endpoints::GITHUB_REPOSITORIES.to_string(),
        };
        let answer = self
            .context
            .client
            .send(self.context.client.get(path))
            .await?;
        let key = match provider {
            Provider::GoogleTasks => "tasklists",
            Provider::GitHub => "repositories",
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
        Ok(answer
            .get("links")
            .cloned()
            .map(serde_json::from_value::<Vec<ExternalLink>>)
            .transpose()
            .unwrap_or_default()
            .unwrap_or_default())
    }

    pub async fn link(
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

    /// One Google pass over one link: pull what changed there, then push what changed here.
    pub async fn sync_google_link(&self, link: &ExternalLink) -> Result<PassReport> {
        let mut report = self.pull(link).await?;
        report.pushed = self.push(link).await?;
        Ok(report)
    }

    /// Bring in what changed on the other side.
    async fn pull(&self, link: &ExternalLink) -> Result<PassReport> {
        let request = self
            .context
            .client
            .get(endpoints::GOOGLE_TASKS)
            .query("linkId", Some(link.id.clone()))
            // The cursor is committed after the pass has been applied, so a client killed halfway
            // re-pulls rather than skipping what it never wrote down.
            .query("deferCursor", Some("1".to_string()));
        let answer = self.context.client.send(request).await?;

        let truncated = answer
            .get("truncated")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let items: Vec<RemoteItem> = answer
            .get("items")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap_or_default()
            .unwrap_or_default();

        let mut report = PassReport {
            pulled: items.len(),
            truncated,
            ..Default::default()
        };

        let task_links = self.task_links(&link.remote_container_id).await?;
        for item in &items {
            let linked_task_id = task_links.get(&item.remote_id).cloned();
            let local = linked_task_id
                .as_deref()
                .and_then(|id| self.context.store.task(id).ok().flatten());

            let outcome = decisions::pull_outcome(
                item.deleted.unwrap_or(false),
                linked_task_id.is_some(),
                local.is_some(),
                false,
            );
            match outcome {
                PullOutcome::DeleteLocalTwin => {
                    if let Some(task) = &local {
                        self.context.store.delete_task(&task.id)?;
                        report.deleted_locally += 1;
                    }
                }
                PullOutcome::IgnoreDeletion | PullOutcome::SkipResurrection => {}
                PullOutcome::Apply => {
                    self.apply(item, link, local, &task_links)?;
                    report.applied += 1;
                }
            }
        }

        // Only now, and only when the page was whole: the cursor is a promise that everything
        // before it has been dealt with.
        if !truncated {
            if let Some(cursor) = answer.get("cursor").and_then(|value| value.as_str()) {
                let request = self
                    .context
                    .client
                    .post(endpoints::GOOGLE_TASKS)
                    .value(json!({
                        "action": "commitCursor",
                        "linkId": link.id,
                        "cursor": cursor,
                    }));
                self.context.client.send(request).await?;
            }
        }
        Ok(report)
    }

    /// The remote-id → task-id map the server keeps for one container.
    async fn task_links(
        &self,
        container_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let request = self
            .context
            .client
            .get(endpoints::GOOGLE_TASK_LINKS)
            .query("containerId", Some(container_id.to_string()));
        let answer = self.context.client.send(request).await?;
        let mut map = std::collections::HashMap::new();
        for link in answer
            .get("taskLinks")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            if let (Some(remote), Some(task)) = (
                link.get("remoteId").and_then(|value| value.as_str()),
                link.get("taskId").and_then(|value| value.as_str()),
            ) {
                map.insert(remote.to_string(), task.to_string());
            }
        }
        Ok(map)
    }

    /// Write one pulled item into the cache.
    fn apply(
        &self,
        item: &RemoteItem,
        link: &ExternalLink,
        local: Option<Task>,
        task_links: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let now = self.context.clock.now();
        let mut task = local.unwrap_or_else(|| {
            // A remote id makes a stable local id for something that came from over there, so a
            // second pass updates the same row rather than making another one.
            Task::new(format!("ext_{}", item.remote_id), item.title.clone())
        });

        task.title = item.title.clone();
        if let Some(notes) = &item.notes {
            task.description = notes.clone();
        }
        // Google Tasks has no time of day, so a due date is a calendar day — which is exactly what
        // an all-day task is here.
        if let Some(due) = &item.due_date {
            task.due_date_time = date::parse(due);
            task.is_all_day = true;
        }
        if decisions::should_adopt_remote_completion(
            item.completed,
            task.completed,
            task.completed_at,
            true,
            task.is_repeating(),
        ) {
            task.completed = item.completed;
            task.completed_at = item.completed.then_some(now);
        }
        // Nesting, when the parent is a task we hold. The key is scoped to the container because
        // Google reuses short task ids between lists — see `external::decisions::parent_key`.
        task.parent_task_id =
            decisions::parent_key(&link.remote_container_id, item.parent.as_deref())
                .and_then(|key| task_links.get(&key).cloned());

        // The link's list, so a pulled task appears where somebody expects it.
        task.list_ids = Some(vec![link.astrid_list_id.clone()]);
        task.updated_at = Some(now);
        self.context.store.upsert_task(&task)?;
        Ok(())
    }

    /// Send what changed here since the last pass.
    ///
    /// "Changed" is `updated_at` against a stamp kept per link. A task that has never reached
    /// astrid-web is skipped: its id is temporary, and linking a remote twin to it would attach
    /// the twin to something about to be given a different id.
    async fn push(&self, link: &ExternalLink) -> Result<usize> {
        let key = format!("external.pushed.{}", link.id);
        let since = self
            .context
            .store
            .metadata(&key)?
            .and_then(|stamp| date::parse(&stamp));
        let now = self.context.clock.now();

        let task_links = self.task_links(&link.remote_container_id).await?;
        let by_task: std::collections::HashMap<&String, &String> = task_links
            .iter()
            .map(|(remote, task)| (task, remote))
            .collect();

        let tasks = self.context.store.tasks_in_list(&link.astrid_list_id)?;
        let mut pushed = 0;
        for task in &tasks {
            if crate::model::is_temp_id(&task.id) {
                continue;
            }
            let changed = match (since, task.updated_at) {
                (Some(since), Some(updated)) => updated > since,
                // Never pushed before, or a task with no stamp: send it once.
                _ => true,
            };
            if !changed {
                continue;
            }

            let request = self
                .context
                .client
                .post(endpoints::GOOGLE_TASKS)
                .value(json!({
                    "linkId": link.id,
                    "title": task.title,
                    "notes": task.description,
                    "dueDate": task.due_date_time.map(date::format),
                    "completed": task.completed,
                    "remoteId": by_task.get(&task.id),
                }));
            self.context.client.send(request).await?;
            pushed += 1;
        }

        self.context.store.set_metadata(&key, &date::format(now))?;
        Ok(pushed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_travels_under_the_name_the_api_knows() {
        assert_eq!(Provider::GoogleTasks.wire(), "GOOGLE_TASKS");
        assert_eq!(Provider::GitHub.wire(), "GITHUB");
        assert_eq!(Provider::GoogleTasks.slug(), "google");
    }
}
