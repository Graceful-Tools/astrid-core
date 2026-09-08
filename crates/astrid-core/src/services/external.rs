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
//! ## Deletions go through a ledger, and the pass is what feeds it
//!
//! The server's link row cascades away with the task, so a deletion has to be captured *at delete
//! time* — and deleting is local, offline and synchronous, with no server to ask. So every pass
//! writes down the links it fetched ([`ledger::remember_links`]), a deletion reads them, and the
//! next pass removes the twin and refuses to import the id again. See [`crate::external::ledger`].
//!
//! It acts on a remote deletion only when Google says so explicitly (`deleted`), never on
//! absence from a page. A cursor pull is not a full listing, and deleting local tasks because a
//! page did not mention them is how a dropped request wipes somebody's list.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;
use crate::external::auto_link::{self, SyncMode};
use crate::external::decisions::{self, PullOutcome};
use crate::external::ledger;
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
    ///
    /// GitHub is `GITHUB_ISSUES` over the wire, which reads oddly beside the enum but is the
    /// server's own name for it — the one `/api/v1/integrations` validates against and the one the
    /// Apple clients send. Anything else is a 400 on disconnect and a provider that never shows as
    /// connected.
    pub fn wire(self) -> &'static str {
        match self {
            Provider::GoogleTasks => "GOOGLE_TASKS",
            Provider::GitHub => "GITHUB_ISSUES",
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

/// Where a pulled task goes.
///
/// A linked list has one; My Tasks has none — it is "assigned to me, in no list", which is a
/// property of the task rather than a place to put it.
enum Placement {
    InList(String),
    MyTasks(String),
}

/// The name the ledger files Google's deletions under.
const PROVIDER_KEY: &str = "google";

/// And the one it files list-to-container links under, so deleting a list can be remembered the
/// same way deleting a task is. Separate from the tasks' own store: the ids mean different things.
const LIST_PROVIDER_KEY: &str = "google.lists";

/// How this account wants its lists linked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoLinkSettings {
    pub mode: SyncMode,
    /// Appended to the name of an Astrid list made for a remote one, when set.
    pub suffix: String,
    /// Remote lists somebody has said no to — a list deleted here, most often.
    pub excluded: Vec<String>,
}

/// What one round of auto-linking did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoLinkReport {
    pub linked: usize,
    pub lists_created: usize,
    pub containers_created: usize,
    /// Lists made here that cannot be linked until they reach the server.
    pub waiting_to_be_created: usize,
    pub failed: usize,
    /// Google's default list, when My Tasks should mirror against it.
    ///
    /// Absent in manual mode, and absent when an older setup linked that list to an ordinary
    /// Astrid list by hand — then the link is authoritative and this phase must not sync the same
    /// thing twice.
    pub my_tasks_container: Option<String>,
}

/// The name a mode travels under in the integration's metadata.
fn mode_wire(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Manual => "manual",
        SyncMode::AllGoogleToAstrid => "all_google_to_astrid",
        SyncMode::AllAstridToGoogle => "all_astrid_to_google",
        SyncMode::AllBidirectional => "all_bidirectional",
    }
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassReport {
    pub pulled: usize,
    pub applied: usize,
    pub deleted_locally: usize,
    /// Task links written down this pass, so a twin is patched next time rather than remade.
    pub linked: usize,
    /// Twins removed over there, for tasks deleted here.
    pub removed_remotely: usize,
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
    ///
    /// Also the moment the server's tombstones arrive — the deletions made on the web and on other
    /// devices — so this device stops re-importing what somebody deleted elsewhere.
    pub async fn status(&self) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::INTEGRATIONS);
        let answer = self.context.client.send(request).await?;
        self.merge_server_tombstones(&answer)?;
        Ok(answer)
    }

    /// Take the tombstones out of Google's integration metadata.
    ///
    /// They arrive as one comma-separated string, which is how the server stores its metadata, and
    /// they go into their own store — never this device's — so a large merge cannot evict a local
    /// deletion. See [`crate::external::ledger`].
    fn merge_server_tombstones(&self, answer: &serde_json::Value) -> Result<()> {
        let Some(integrations) = answer
            .get("integrations")
            .and_then(|value| value.as_array())
        else {
            return Ok(());
        };
        let Some(google) = integrations.iter().find(|integration| {
            integration.get("provider").and_then(|value| value.as_str())
                == Some(Provider::GoogleTasks.wire())
        }) else {
            return Ok(());
        };
        let ids: Vec<String> = google
            .get("metadata")
            .and_then(|metadata| metadata.get("tombstonedRemoteIds"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect();
        ledger::merge_server_tombstones(&self.context.store, PROVIDER_KEY, &ids)?;
        Ok(())
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
                LIST_PROVIDER_KEY,
                "google",
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

    /// One Google pass over one link: remove what was deleted here, pull, then push.
    ///
    /// Deletions first. A pull that ran before them can re-import the very task somebody just
    /// deleted — the tombstone stops that, but only if the pull is not racing the removal.
    pub async fn sync_google_link(&self, link: &ExternalLink) -> Result<PassReport> {
        let removed = self.remove_deleted_twins(link).await?;
        let mut report = self.pull(link).await?;
        report.removed_remotely = removed;
        report.pushed = self.push(link).await?;
        Ok(report)
    }

    /// Remove the remote twins of tasks deleted on this machine.
    ///
    /// A twin that is already gone counts as done: 404 and 410 both mean the work is finished, and
    /// retrying for ever because somebody deleted it over there too is not a failure worth keeping.
    /// Anything else is left pending, so a server having a bad minute does not lose the deletion.
    async fn remove_deleted_twins(&self, link: &ExternalLink) -> Result<usize> {
        self.remove_twins(&link.remote_container_id, &[("linkId", &link.id)])
            .await
    }

    /// The same, addressed either by link or by remote list — My Tasks has no link to name.
    async fn remove_twins(&self, container_id: &str, address: &[(&str, &str)]) -> Result<usize> {
        let store = &self.context.store;
        let mut removed = 0;
        for (remote_id, pending_container) in ledger::pending(store, PROVIDER_KEY) {
            // This container's only: a pass for one list must not delete out of another.
            if pending_container != container_id {
                continue;
            }
            let mut request = self.context.client.delete(endpoints::GOOGLE_TASKS);
            for (name, value) in address {
                request = request.query(name, Some((*value).to_string()));
            }
            let request = request.query("remoteId", Some(remote_id.clone()));
            match self.context.client.send(request).await {
                Ok(_) => {
                    ledger::clear_pending(store, PROVIDER_KEY, &remote_id)?;
                    removed += 1;
                }
                Err(crate::api::ApiError::Http { status, .. })
                    if decisions::remote_already_gone(status) =>
                {
                    ledger::clear_pending(store, PROVIDER_KEY, &remote_id)?;
                }
                Err(_) => {}
            }
        }
        Ok(removed)
    }

    // ── Auto-linking ─────────────────────────────────────────────────────────────────────────

    /// How this account links lists, and what it calls the ones it makes.
    ///
    /// The choice lives in the integration's metadata rather than on this machine, so somebody who
    /// turns on "every list" at a desk does not have to turn it on again on a laptop.
    pub async fn auto_link_settings(&self) -> Result<AutoLinkSettings> {
        Ok(Self::read_settings(&self.status().await?))
    }

    fn read_settings(status: &serde_json::Value) -> AutoLinkSettings {
        let metadata = status
            .get("integrations")
            .and_then(|value| value.as_array())
            .and_then(|integrations| {
                integrations.iter().find(|integration| {
                    integration.get("provider").and_then(|value| value.as_str())
                        == Some(Provider::GoogleTasks.wire())
                })
            })
            .and_then(|integration| integration.get("metadata"));
        let text = |key: &str| {
            metadata
                .and_then(|metadata| metadata.get(key))
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        };
        AutoLinkSettings {
            // An unknown mode is manual. A metadata value this build does not recognise must not
            // start creating lists on somebody's account.
            mode: match text("googleSyncMode").as_str() {
                "all_google_to_astrid" => SyncMode::AllGoogleToAstrid,
                "all_astrid_to_google" => SyncMode::AllAstridToGoogle,
                "all_bidirectional" => SyncMode::AllBidirectional,
                _ => SyncMode::Manual,
            },
            suffix: text("listSuffix"),
            excluded: text("excludedTasklists")
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }

    /// Choose how lists get linked.
    pub async fn set_auto_link_mode(&self, mode: SyncMode, suffix: Option<&str>) -> Result<()> {
        let mut metadata = json!({ "googleSyncMode": mode_wire(mode) });
        if let Some(suffix) = suffix {
            metadata["listSuffix"] = json!(suffix);
        }
        let request = self
            .context
            .client
            .patch(endpoints::INTEGRATIONS)
            .value(json!({
                "provider": Provider::GoogleTasks.wire(),
                "metadata": metadata,
            }));
        self.context.client.send(request).await?;
        Ok(())
    }

    /// Give every unlinked list on either side a counterpart, according to the account's mode.
    ///
    /// Nothing here decides anything: the plan comes from [`crate::external::auto_link`], which is
    /// where the adoption rules and their tests live. This is the part that carries it out.
    ///
    /// One failure does not stop the rest. Linking eight lists and giving up at the first one that
    /// answers badly leaves seven unlinked for a reason nobody can see.
    pub async fn auto_link_google(&self) -> Result<AutoLinkReport> {
        let settings = self.auto_link_settings().await?;
        let mut report = AutoLinkReport::default();
        if settings.mode == SyncMode::Manual {
            return Ok(report);
        }

        let (containers, default_id) = self.containers(Provider::GoogleTasks).await?;
        let links = self.links(Provider::GoogleTasks).await?;
        // What this device has said no to, plus what the account has. Pushed up when they differ,
        // so a list deleted on this machine stops being offered on the others.
        let excluded = self.share_exclusions(&settings).await;
        let linked_container_ids: Vec<String> = links
            .iter()
            .map(|link| link.remote_container_id.clone())
            .collect();
        let linked_list_ids: Vec<String> = links
            .iter()
            .map(|link| link.astrid_list_id.clone())
            .collect();

        // The default remote list pairs with My Tasks rather than with a list of its own, unless
        // an older setup linked it by hand — see `auto_link::candidates`.
        let inward = matches!(
            settings.mode,
            SyncMode::AllGoogleToAstrid | SyncMode::AllBidirectional
        );
        let all: Vec<auto_link::ListRef> = containers
            .iter()
            .map(|container| auto_link::ListRef {
                id: container.id.clone(),
                name: container.name.clone(),
            })
            .collect();
        let tasklists: Vec<auto_link::ListRef> =
            auto_link::candidates(&all, default_id.as_deref(), &linked_container_ids, inward)
                .into_iter()
                .filter(|tasklist| !excluded.contains(&tasklist.id))
                .cloned()
                .collect();

        let lists: Vec<auto_link::ListRef> = self
            .context
            .store
            .lists()?
            .into_iter()
            .filter(|list| list.is_domain_list() && list.is_virtual != Some(true))
            .map(|list| auto_link::ListRef {
                id: list.id,
                name: list.name,
            })
            .collect();

        report.my_tasks_container = default_id
            .as_deref()
            .filter(|id| {
                auto_link::my_tasks_phase_active(settings.mode, Some(id), &linked_container_ids)
            })
            .map(str::to_string);

        match settings.mode {
            SyncMode::Manual => {}
            SyncMode::AllBidirectional => {
                let (here, there) = auto_link::bidirectional(
                    &tasklists,
                    &lists,
                    &linked_container_ids,
                    &linked_list_ids,
                    &settings.suffix,
                );
                self.link_inward(&here, &mut report).await;
                self.link_outward(&there, &mut report).await;
            }
            SyncMode::AllGoogleToAstrid => {
                let unlinked: Vec<auto_link::ListRef> = lists
                    .iter()
                    .filter(|list| !linked_list_ids.contains(&list.id))
                    .cloned()
                    .collect();
                let plan = auto_link::google_to_astrid(
                    &tasklists,
                    &linked_container_ids,
                    &unlinked,
                    &settings.suffix,
                );
                self.link_inward(&plan, &mut report).await;
            }
            SyncMode::AllAstridToGoogle => {
                let unlinked: Vec<auto_link::ListRef> = tasklists
                    .iter()
                    .filter(|tasklist| !linked_container_ids.contains(&tasklist.id))
                    .cloned()
                    .collect();
                let plan = auto_link::astrid_to_google(&lists, &linked_list_ids, &unlinked);
                self.link_outward(&plan, &mut report).await;
            }
        }
        Ok(report)
    }

    /// The exclusions this device and the account hold between them.
    ///
    /// Best effort on the sharing: an auto-link that refused to run because it could not write a
    /// setting would be worse than one whose other devices learn a pass later.
    async fn share_exclusions(&self, settings: &AutoLinkSettings) -> Vec<String> {
        let mine = ledger::excluded(&self.context.store, PROVIDER_KEY);
        let mut union = settings.excluded.clone();
        for id in mine {
            if !union.contains(&id) {
                union.push(id);
            }
        }
        if union.len() != settings.excluded.len() {
            let request = self
                .context
                .client
                .patch(endpoints::INTEGRATIONS)
                .value(json!({
                    "provider": Provider::GoogleTasks.wire(),
                    "metadata": { "excludedTasklists": union.join(",") },
                }));
            let _ = self.context.client.send(request).await;
        }
        union
    }

    /// Remote lists that need an Astrid one.
    async fn link_inward(
        &self,
        plan: &[auto_link::AdoptOrCreateHere],
        report: &mut AutoLinkReport,
    ) {
        for action in plan {
            let list_id = match &action.adopt_list_id {
                Some(id) => id.clone(),
                None => match self.context.lists().create(&action.new_list_name, None) {
                    Ok(list) => {
                        report.lists_created += 1;
                        list.id
                    }
                    Err(_) => {
                        report.failed += 1;
                        continue;
                    }
                },
            };
            // A list that has not reached the server has a temporary id, and linking a remote list
            // to it would attach the link to something about to be given a different id. It waits
            // for the next pass, which adopts it by name rather than making a second one.
            if crate::model::is_temp_id(&list_id) {
                report.waiting_to_be_created += 1;
                continue;
            }
            match self
                .link(Provider::GoogleTasks, &list_id, &action.tasklist_id)
                .await
            {
                Ok(_) => report.linked += 1,
                Err(_) => report.failed += 1,
            }
        }
    }

    /// Astrid lists that need a remote one.
    async fn link_outward(
        &self,
        plan: &[auto_link::AdoptOrCreateThere],
        report: &mut AutoLinkReport,
    ) {
        for action in plan {
            let container_id = match &action.adopt_tasklist_id {
                Some(id) => id.clone(),
                None => match self.create_container(&action.new_tasklist_name).await {
                    Ok(id) => {
                        report.containers_created += 1;
                        id
                    }
                    Err(_) => {
                        report.failed += 1;
                        continue;
                    }
                },
            };
            match self
                .link(Provider::GoogleTasks, &action.list_id, &container_id)
                .await
            {
                Ok(_) => report.linked += 1,
                Err(_) => report.failed += 1,
            }
        }
    }

    /// Make a Google task list, and answer with its id.
    async fn create_container(&self, name: &str) -> Result<String> {
        let request = self
            .context
            .client
            .post(endpoints::GOOGLE_TASKLISTS)
            .value(json!({ "title": name }));
        let answer = self.context.client.send(request).await?;
        Ok(answer
            .get("tasklist")
            .and_then(|tasklist| tasklist.get("id"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string())
    }

    // ── My Tasks ↔ the default remote list ───────────────────────────────────────────────────

    /// Mirror My Tasks — unlisted tasks assigned to you — against Google's default list.
    ///
    /// Google's default list is where its own apps put a task nobody filed anywhere, which is what
    /// My Tasks is here. Pairing them with an ordinary list link would need an Astrid list that
    /// does not exist, so this runs beside the links rather than through one: no link row, no
    /// cursor, always the full listing.
    ///
    /// Which half runs follows the mode, the same as everywhere else: a mode that only mirrors
    /// outward does not pull, and one that only mirrors inward does not push.
    pub async fn sync_my_tasks(&self, tasklist_id: &str) -> Result<PassReport> {
        let Some(user_id) = self.context.account().current_user_id()? else {
            return Ok(PassReport::default());
        };
        let settings = self.auto_link_settings().await?;
        let pulls = matches!(
            settings.mode,
            SyncMode::AllGoogleToAstrid | SyncMode::AllBidirectional
        );
        let pushes = matches!(
            settings.mode,
            SyncMode::AllAstridToGoogle | SyncMode::AllBidirectional
        );

        let mut report = PassReport {
            removed_remotely: self
                .remove_twins(tasklist_id, &[("tasklistId", tasklist_id)])
                .await?,
            ..Default::default()
        };

        let request = self
            .context
            .client
            .get(endpoints::GOOGLE_TASKS)
            .query("tasklistId", Some(tasklist_id.to_string()));
        let answer = self.context.client.send(request).await?;
        let items: Vec<RemoteItem> = answer
            .get("items")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap_or_default()
            .unwrap_or_default();
        report.pulled = items.len();
        report.truncated = answer
            .get("truncated")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        let task_links = self.task_links(tasklist_id).await?;
        if pulls {
            let tombstoned = ledger::tombstoned(&self.context.store, PROVIDER_KEY);
            for item in &items {
                let linked_task_id = task_links.get(&item.remote_id).cloned().or_else(|| {
                    ledger::local_task_for(&self.context.store, PROVIDER_KEY, &item.remote_id)
                });
                let local = linked_task_id
                    .as_deref()
                    .and_then(|id| self.context.store.task(id).ok().flatten());
                match decisions::pull_outcome(
                    item.deleted.unwrap_or(false),
                    linked_task_id.is_some(),
                    local.is_some(),
                    tombstoned.contains(&item.remote_id),
                ) {
                    PullOutcome::DeleteLocalTwin => {
                        if let Some(task) = &local {
                            self.context.tasks().delete(&task.id)?;
                            report.deleted_locally += 1;
                        }
                        ledger::record_tombstone(
                            &self.context.store,
                            PROVIDER_KEY,
                            &item.remote_id,
                        )?;
                    }
                    PullOutcome::IgnoreDeletion | PullOutcome::SkipResurrection => {}
                    PullOutcome::Apply => {
                        let task = self.apply(
                            item,
                            tasklist_id,
                            &Placement::MyTasks(user_id.clone()),
                            local,
                            &task_links,
                        )?;
                        report.applied += 1;
                        ledger::remember_links(
                            &self.context.store,
                            PROVIDER_KEY,
                            tasklist_id,
                            [(task.id.clone(), item.remote_id.clone())],
                        )?;
                        if linked_task_id.is_none()
                            && self
                                .record_task_link(&task.id, &item.remote_id, tasklist_id)
                                .await
                        {
                            report.linked += 1;
                        }
                    }
                }
            }
        }

        if pushes {
            report.pushed = self
                .push_my_tasks(tasklist_id, &user_id, &task_links)
                .await?;
        }
        Ok(report)
    }

    /// Send the unlisted tasks assigned to you, and close out the twins of tasks that have left.
    async fn push_my_tasks(
        &self,
        tasklist_id: &str,
        user_id: &str,
        task_links: &std::collections::HashMap<String, String>,
    ) -> Result<usize> {
        let key = format!("external.pushed.myTasks.{tasklist_id}");
        let since = self
            .context
            .store
            .metadata(&key)?
            .and_then(|stamp| date::parse(&stamp));
        let now = self.context.clock.now();
        let by_task: std::collections::HashMap<&String, &String> = task_links
            .iter()
            .map(|(remote, task)| (task, remote))
            .collect();

        let held = self.context.store.tasks()?;
        let mine = |task: &Task| {
            task.list_ids.as_ref().is_none_or(|lists| lists.is_empty())
                && task.assignee_id.as_deref() == Some(user_id)
        };

        let mut pushed = 0;
        for task in held.iter().filter(|task| mine(task)) {
            if crate::model::is_temp_id(&task.id) {
                continue;
            }
            let changed = match (since, task.updated_at) {
                (Some(since), Some(updated)) => updated > since,
                _ => true,
            };
            if !changed {
                continue;
            }
            let known = by_task.get(&task.id).copied();
            let request = self
                .context
                .client
                .post(endpoints::GOOGLE_TASKS)
                .value(json!({
                    "tasklistId": tasklist_id,
                    "title": task.title,
                    "notes": task.description,
                    "dueDate": task.due_date_time.map(date::format),
                    "completed": task.completed,
                    "remoteId": known,
                }));
            let answer = self.context.client.send(request).await?;
            pushed += 1;
            if known.is_none() {
                if let Some(remote_id) = answer.get("remoteId").and_then(|value| value.as_str()) {
                    self.record_task_link(&task.id, remote_id, tasklist_id)
                        .await;
                }
            }
        }

        // A task that has LEFT My Tasks — it gained a list, or lost the assignment that put it
        // here — still has a twin in the default list, which then acts as a second home for
        // something that already has one. Close it out. A deleted task is not this: that goes
        // through the ledger.
        for (remote_id, task_id) in task_links {
            let Some(task) = self.context.store.task(task_id).ok().flatten() else {
                continue;
            };
            if mine(&task) {
                continue;
            }
            let request = self
                .context
                .client
                .delete(endpoints::GOOGLE_TASKS)
                .query("tasklistId", Some(tasklist_id.to_string()))
                .query("remoteId", Some(remote_id.clone()));
            match self.context.client.send(request).await {
                Ok(_) => {}
                Err(crate::api::ApiError::Http { status, .. })
                    if decisions::remote_already_gone(status) => {}
                // A real failure is worth another go rather than a tombstone on a twin that is
                // still there.
                Err(_) => continue,
            }
            ledger::record_tombstone(&self.context.store, PROVIDER_KEY, remote_id)?;
            ledger::forget_link(&self.context.store, PROVIDER_KEY, task_id)?;
        }

        self.context.store.set_metadata(&key, &date::format(now))?;
        Ok(pushed)
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
        let tombstoned = ledger::tombstoned(&self.context.store, PROVIDER_KEY);
        for item in &items {
            // The server's map first, then this device's own. A task pulled while offline is
            // not on the server's map yet, and without the local answer the next pass would pull
            // the same item in a second time.
            let linked_task_id = task_links.get(&item.remote_id).cloned().or_else(|| {
                ledger::local_task_for(&self.context.store, PROVIDER_KEY, &item.remote_id)
            });
            let local = linked_task_id
                .as_deref()
                .and_then(|id| self.context.store.task(id).ok().flatten());

            let outcome = decisions::pull_outcome(
                item.deleted.unwrap_or(false),
                linked_task_id.is_some(),
                local.is_some(),
                tombstoned.contains(&item.remote_id),
            );
            match outcome {
                PullOutcome::DeleteLocalTwin => {
                    if let Some(task) = &local {
                        self.context.store.delete_task(&task.id)?;
                        report.deleted_locally += 1;
                    }
                    // Tombstoned, not pushed back: the deletion came from over there, and echoing
                    // it would be this device deleting an item that is already gone.
                    ledger::record_tombstone(&self.context.store, PROVIDER_KEY, &item.remote_id)?;
                }
                PullOutcome::IgnoreDeletion | PullOutcome::SkipResurrection => {}
                PullOutcome::Apply => {
                    let task = self.apply(
                        item,
                        &link.remote_container_id,
                        &Placement::InList(link.astrid_list_id.clone()),
                        local,
                        &task_links,
                    )?;
                    report.applied += 1;
                    // Written down here as well as on the server: this is what a second pass reads
                    // when the first one's task has not reached astrid-web yet.
                    ledger::remember_links(
                        &self.context.store,
                        PROVIDER_KEY,
                        &link.remote_container_id,
                        [(task.id.clone(), item.remote_id.clone())],
                    )?;
                    // Only when it is new to us. Re-sending an existing link every pass is a write
                    // per task per five minutes for something that has not changed.
                    if linked_task_id.is_none()
                        && self
                            .record_task_link(&task.id, &item.remote_id, &link.remote_container_id)
                            .await
                    {
                        report.linked += 1;
                    }
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
        // Written down for the delete-time capture: a deletion cannot ask the server which remote
        // item a task was, so a pass has to have said so first.
        ledger::remember_links(
            &self.context.store,
            PROVIDER_KEY,
            container_id,
            map.iter()
                .map(|(remote, task)| (task.clone(), remote.clone())),
        )?;
        Ok(map)
    }

    /// Write one pulled item, and answer with the local task it became.
    ///
    /// Through the task service, not straight into the cache. A pulled task has to reach
    /// astrid-web — it is an Astrid task now, and one that existed only in this machine's cache
    /// would be invisible on the web, absent on the phone, and gone at the next sign-out.
    fn apply(
        &self,
        item: &RemoteItem,
        container_id: &str,
        placement: &Placement,
        local: Option<Task>,
        task_links: &std::collections::HashMap<String, String>,
    ) -> Result<Task> {
        let tasks = self.context.tasks();
        // Nesting, when the parent is a task we hold. The key is scoped to the container because
        // Google reuses short task ids between lists — see `external::decisions::parent_key`.
        let parent = decisions::parent_key(container_id, item.parent.as_deref())
            .and_then(|key| task_links.get(&key).cloned());
        // Google Tasks has no time of day, so a due date is a calendar day — which is exactly what
        // an all-day task is here.
        let due = item.due_date.as_deref().and_then(date::parse);

        let Some(task) = local else {
            let mut draft = crate::services::TaskDraft::new(item.title.clone());
            draft.description = item.notes.clone().unwrap_or_default();
            match placement {
                // The link's list, so a pulled task appears where somebody expects it.
                Placement::InList(list_id) => draft.list_ids = vec![list_id.clone()],
                // My Tasks is not a list: it is "assigned to me, in no list", so that is what a
                // task pulled from the default remote list has to become.
                Placement::MyTasks(user_id) => draft.assignee_id = Some(user_id.clone()),
            }
            draft.due_date_time = due;
            draft.is_all_day = due.is_some();
            draft.parent_task_id = parent;
            return tasks.create(&draft);
        };

        let mut changes = crate::services::TaskChanges::default();
        if task.title != item.title {
            changes.title = Some(item.title.clone());
        }
        if let Some(notes) = &item.notes {
            if &task.description != notes {
                changes.description = Some(notes.clone());
            }
        }
        if due.is_some() && task.due_date_time != due {
            changes.due_date_time = Some(due);
            changes.is_all_day = Some(true);
        }
        if task.parent_task_id != parent {
            changes.parent_task_id = Some(parent);
        }
        if decisions::should_adopt_remote_completion(
            item.completed,
            task.completed,
            task.completed_at,
            true,
            task.is_repeating(),
        ) {
            // Through the completion path, because a repeating task rolls forward rather than
            // being ticked off — see `TaskService::complete`.
            return tasks.complete(&task.id, item.completed, Some(&task), None);
        }
        if changes == crate::services::TaskChanges::default() {
            return Ok(task);
        }
        tasks.update(&task.id, &changes)
    }

    /// Tell the server which remote item a task mirrors.
    ///
    /// Without this the link exists nowhere: the next pass reads an empty map, sees a task with no
    /// remote twin, and creates a second one over there — every pass, for ever.
    ///
    /// A task still carrying a temporary id is skipped rather than sent: the link row is a foreign
    /// key onto the task, and the server rejects an id it has never seen. The next pass, once the
    /// Outbox has been through, does it.
    async fn record_task_link(&self, task_id: &str, remote_id: &str, container_id: &str) -> bool {
        let task_id = self
            .context
            .store
            .resolve_id(task_id)
            .unwrap_or_else(|_| task_id.to_string());
        if crate::model::is_temp_id(&task_id) {
            return false;
        }
        let request = self
            .context
            .client
            .put(endpoints::GOOGLE_TASK_LINKS)
            .value(json!({
                "astridTaskId": task_id,
                "remoteId": remote_id,
                "remoteContainerId": container_id,
            }));
        self.context.client.send(request).await.is_ok()
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

            let known = by_task.get(&task.id).copied();
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
                    "remoteId": known,
                }));
            let answer = self.context.client.send(request).await?;
            pushed += 1;

            // A create has made a remote twin that only this response knows about. Writing the
            // link down is what stops the next pass making a second one — and a third, and one
            // every five minutes after that.
            if known.is_none() {
                if let Some(remote_id) = answer.get("remoteId").and_then(|value| value.as_str()) {
                    self.record_task_link(&task.id, remote_id, &link.remote_container_id)
                        .await;
                }
            }
        }

        self.context.store.set_metadata(&key, &date::format(now))?;
        Ok(pushed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::api::{ApiClient, StubTransport};
    use crate::model::date;
    use crate::platform::{FixedClock, MemorySecureStore};
    use crate::store::Store;
    use std::sync::Arc;

    struct Fixture {
        service: ExternalSyncService,
        store: Arc<Store>,
        transport: Arc<StubTransport>,
    }

    /// A pass over one link, with the four requests it makes scripted in the order it makes them:
    /// the deletion, the pull, and the task-link fetch each of pull and push does.
    fn fixture(delete_status: u16, pulled: serde_json::Value) -> Fixture {
        let transport = Arc::new(
            StubTransport::new()
                // The deletion goes first, so its answer is queued first.
                .push_json("google/tasks", delete_status, json!({}))
                .push_json("google/tasks", 200, json!({ "items": pulled }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] })),
        );
        let store = Arc::new(Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                transport.clone(),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            Arc::new(FixedClock::at(
                date::parse("2026-09-07T12:00:00Z").expect("an instant"),
            )),
        );
        Fixture {
            service: context.external(),
            store,
            transport,
        }
    }

    /// The same, with the caller scripting the whole conversation.
    fn fixture_with(transport: StubTransport) -> Fixture {
        let transport = Arc::new(transport);
        let store = Arc::new(Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                transport.clone(),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            Arc::new(FixedClock::at(
                date::parse("2026-09-07T12:00:00Z").expect("an instant"),
            )),
        );
        Fixture {
            service: context.external(),
            store,
            transport,
        }
    }

    /// An account in one of the all-lists modes, with one remote list and no links.
    fn auto_link_transport(mode: &str) -> StubTransport {
        StubTransport::new()
            .push_json(
                "/api/v1/integrations",
                200,
                json!({
                    "integrations": [{
                        "provider": "GOOGLE_TASKS",
                        "metadata": { "googleSyncMode": mode },
                    }],
                }),
            )
            .push_json(
                "google/tasklists",
                200,
                json!({
                    "tasklists": [{ "id": "c1", "name": "Groceries" }],
                    "defaultId": "default-list",
                }),
            )
            .push_json("google/links", 200, json!({ "links": [] }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            }))
    }

    fn a_list(id: &str, name: &str) -> crate::model::TaskList {
        crate::model::TaskList::new(id, name)
    }

    fn link() -> ExternalLink {
        ExternalLink {
            id: "link-1".into(),
            astrid_list_id: "l1".into(),
            remote_container_id: "tasklist-1".into(),
            cursor: None,
        }
    }

    /// A deletion made on the web reaches this device as metadata on the integration. Without
    /// this, the next pull imports it again and somebody's deleted task is back.
    #[tokio::test]
    async fn the_servers_tombstones_arrive_with_the_status() {
        let fixture = fixture(200, json!([]));
        let answer = json!({
            "integrations": [{
                "provider": "GOOGLE_TASKS",
                "metadata": { "tombstonedRemoteIds": "r1, r2" },
            }],
        });
        fixture
            .service
            .merge_server_tombstones(&answer)
            .expect("merges");

        let held = ledger::tombstoned(&fixture.store, PROVIDER_KEY);
        assert!(held.contains(&"r1".to_string()));
        assert!(held.contains(&"r2".to_string()));
    }

    // ── The link that stops a twin being made twice ──────────────────────────────────────────

    /// The bug this pins: a push that does not write the link down leaves the next pass with no
    /// remote id, so it creates a *second* Google task — and one more every five minutes after.
    #[tokio::test]
    async fn a_pushed_task_is_linked_so_the_next_pass_patches_it_rather_than_making_another() {
        let fixture = fixture_with(
            StubTransport::new()
                // The pull: nothing to bring in.
                .push_json("google/tasks", 200, json!({ "items": [] }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                // The push, and the link that follows it.
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .push_json(
                    "google/tasks",
                    200,
                    json!({ "remoteId": "tasklist-1:r9", "remoteUpdatedAt": "2026-09-07T12:00:00Z" }),
                )
                .push_json("google/task-links", 200, json!({ "link": {} }))
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );
        let mut task = crate::model::Task::new("cm3real", "Buy milk");
        task.list_ids = Some(vec!["l1".into()]);
        fixture.store.upsert_task(&task).expect("writes");

        fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        let linked = fixture
            .transport
            .requests()
            .into_iter()
            .find(|request| {
                request.method.as_str() == "PUT" && request.url.contains("google/task-links")
            })
            .expect("the link was written down");
        let body: serde_json::Value =
            serde_json::from_slice(&linked.body.unwrap_or_default()).expect("a body");
        assert_eq!(body["astridTaskId"], "cm3real");
        assert_eq!(body["remoteId"], "tasklist-1:r9");
        assert_eq!(body["remoteContainerId"], "tasklist-1");
    }

    /// The link row is a foreign key onto the task, so the server rejects an id it has never seen.
    /// Sending one would be a guaranteed 400 on every pass until the Outbox caught up.
    #[tokio::test]
    async fn a_task_that_has_not_reached_the_server_is_not_linked_yet() {
        let fixture = fixture_with(
            StubTransport::new()
                .push_json("google/tasks", 200, json!({ "items": [] }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );
        let mut task = crate::model::Task::new("temp_abc", "Buy milk");
        task.list_ids = Some(vec!["l1".into()]);
        fixture.store.upsert_task(&task).expect("writes");

        fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        assert!(
            !fixture
                .transport
                .requests()
                .iter()
                .any(|request| request.method.as_str() == "PUT"),
            "nothing was linked, and nothing was pushed either"
        );
    }

    /// A pulled task is an Astrid task now. One written only to this machine's cache would be
    /// invisible on the web, absent on the phone, and gone at the next sign-out.
    #[tokio::test]
    async fn a_pulled_task_is_written_through_the_journal_so_it_reaches_the_server() {
        let fixture = fixture_with(
            StubTransport::new()
                .push_json(
                    "google/tasks",
                    200,
                    json!({ "items": [{ "remoteId": "tasklist-1:r1", "title": "Buy milk" }] }),
                )
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );

        let report = fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        assert_eq!(report.applied, 1);
        let queued = crate::outbox::journal::all(&fixture.store).expect("reads");
        assert!(
            queued
                .iter()
                .any(|entry| entry.kind == crate::outbox::kind::CREATE_TASK),
            "the create is on its way to astrid-web, not only in the cache"
        );
    }

    /// Two passes before the Outbox has been through must not pull the same item in twice. The
    /// server does not know the link yet, so only this device's own note stops the duplicate.
    #[tokio::test]
    async fn a_second_pass_before_the_task_reaches_the_server_does_not_pull_it_in_again() {
        let items = json!({ "items": [{ "remoteId": "tasklist-1:r1", "title": "Buy milk" }] });
        let fixture = fixture_with(
            StubTransport::new()
                .push_json("google/tasks", 200, items.clone())
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .push_json("google/tasks", 200, items)
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );

        fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("one");
        fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("two");

        let held = fixture.store.tasks().expect("reads");
        assert_eq!(
            held.iter().filter(|task| task.title == "Buy milk").count(),
            1,
            "one task, not one per pass"
        );
    }

    // ── My Tasks ↔ the default remote list ───────────────────────────────────────────────────

    /// The account has to be known before My Tasks means anything: it is "assigned to me", and
    /// without a "me" there is nothing to mirror.
    fn signed_in(fixture: &Fixture) {
        let mut user = crate::model::User::new("u1");
        user.name = Some("Ada".into());
        fixture
            .store
            .set_metadata(
                "account.current-user",
                &serde_json::to_string(&user).expect("encodes"),
            )
            .expect("writes");
    }

    /// One conversation, in the order the pass has it: the deletion pass, the full listing, and
    /// the task links each half asks for.
    fn my_tasks_transport(mode: &str, items: serde_json::Value) -> StubTransport {
        StubTransport::new()
            .push_json(
                "/api/v1/integrations",
                200,
                json!({
                    "integrations": [{
                        "provider": "GOOGLE_TASKS",
                        "metadata": { "googleSyncMode": mode },
                    }],
                }),
            )
            .push_json("google/tasks", 200, json!({ "items": items }))
            .push_json("google/task-links", 200, json!({ "taskLinks": [] }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            }))
    }

    /// A task Google filed nowhere becomes a task Astrid filed nowhere, assigned to you. Putting
    /// it in a list would be inventing a list nobody made.
    #[tokio::test]
    async fn a_task_from_the_default_remote_list_becomes_an_unlisted_task_of_yours() {
        let fixture = fixture_with(my_tasks_transport(
            "all_google_to_astrid",
            json!([{ "remoteId": "default-list:r1", "title": "Ring the dentist" }]),
        ));
        signed_in(&fixture);

        let report = fixture
            .service
            .sync_my_tasks("default-list")
            .await
            .expect("a pass");

        assert_eq!(report.applied, 1);
        let made = fixture
            .store
            .tasks()
            .expect("reads")
            .into_iter()
            .find(|task| task.title == "Ring the dentist")
            .expect("the task");
        assert_eq!(made.assignee_id.as_deref(), Some("u1"));
        assert!(
            made.list_ids.unwrap_or_default().is_empty(),
            "unlisted, which is what My Tasks means"
        );
    }

    /// A mode that only mirrors outward does not pull, here as everywhere else.
    #[tokio::test]
    async fn my_tasks_does_not_pull_in_a_mode_that_only_mirrors_outward() {
        let fixture = fixture_with(my_tasks_transport(
            "all_astrid_to_google",
            json!([{ "remoteId": "default-list:r1", "title": "Ring the dentist" }]),
        ));
        signed_in(&fixture);

        let report = fixture
            .service
            .sync_my_tasks("default-list")
            .await
            .expect("a pass");

        assert_eq!(report.applied, 0);
        assert!(fixture.store.tasks().expect("reads").is_empty());
    }

    /// A task that gained a list, or lost the assignment that put it in My Tasks, still has a twin
    /// in the default list — a second home for something that already has one.
    #[tokio::test]
    async fn a_task_that_has_left_my_tasks_has_its_twin_closed_out() {
        let fixture = fixture_with(
            StubTransport::new()
                .push_json(
                    "/api/v1/integrations",
                    200,
                    json!({
                        "integrations": [{
                            "provider": "GOOGLE_TASKS",
                            "metadata": { "googleSyncMode": "all_bidirectional" },
                        }],
                    }),
                )
                .push_json("google/tasks", 200, json!({ "items": [] }))
                .push_json(
                    "google/task-links",
                    200,
                    json!({
                        "taskLinks": [{ "remoteId": "default-list:r1", "taskId": "cm3real" }],
                    }),
                )
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );
        signed_in(&fixture);
        // It has a list now, so it is not My Tasks any more.
        let mut task = crate::model::Task::new("cm3real", "Ring the dentist");
        task.assignee_id = Some("u1".into());
        task.list_ids = Some(vec!["l1".into()]);
        fixture.store.upsert_task(&task).expect("writes");

        fixture
            .service
            .sync_my_tasks("default-list")
            .await
            .expect("a pass");

        assert!(
            fixture.transport.requests().iter().any(|request| {
                request.method.as_str() == "DELETE" && request.url.contains("remoteId=default-list")
            }),
            "the twin in the default list was closed out"
        );
        assert!(
            ledger::tombstoned(&fixture.store, PROVIDER_KEY)
                .contains(&"default-list:r1".to_string()),
            "and never re-imported"
        );
        assert!(
            fixture.store.task("cm3real").expect("reads").is_some(),
            "the task itself is untouched — it lives in its list now"
        );
    }

    /// Signed out there is no "me", so there is nothing this could mean.
    #[tokio::test]
    async fn my_tasks_does_nothing_when_nobody_is_signed_in() {
        let fixture = fixture_with(my_tasks_transport("all_bidirectional", json!([])));

        let report = fixture
            .service
            .sync_my_tasks("default-list")
            .await
            .expect("a pass");

        assert_eq!(report, PassReport::default());
    }

    // ── Auto-linking ─────────────────────────────────────────────────────────────────────────

    /// The whole point of the adoption rules: somebody with "Groceries" on both sides ends up with
    /// one list, not two called the same thing.
    #[tokio::test]
    async fn a_remote_list_adopts_the_local_one_of_the_same_name() {
        let fixture = fixture_with(auto_link_transport("all_google_to_astrid"));
        fixture
            .store
            .upsert_list(&a_list("cm3real", "Groceries"))
            .expect("writes");

        let report = fixture.service.auto_link_google().await.expect("links");

        assert_eq!(report.linked, 1);
        assert_eq!(report.lists_created, 0, "nothing was duplicated");
        let linked = fixture
            .transport
            .requests()
            .into_iter()
            .find(|request| {
                request.url.contains("google/links") && request.method.as_str() == "POST"
            })
            .expect("a link was made");
        let body: serde_json::Value =
            serde_json::from_slice(&linked.body.unwrap_or_default()).expect("a body");
        assert_eq!(body["astridListId"], "cm3real");
        assert_eq!(body["remoteContainerId"], "c1");
    }

    /// Manual is the default and has to stay one: a mode this build does not recognise must not
    /// start making lists on somebody's account.
    #[tokio::test]
    async fn manual_mode_links_nothing() {
        let fixture = fixture_with(auto_link_transport("manual"));
        fixture
            .store
            .upsert_list(&a_list("cm3real", "Groceries"))
            .expect("writes");

        let report = fixture.service.auto_link_google().await.expect("links");

        assert_eq!(report, AutoLinkReport::default());
    }

    #[tokio::test]
    async fn a_mode_this_build_does_not_know_is_manual() {
        let fixture = fixture_with(auto_link_transport("all_the_things_v3"));
        let settings = fixture.service.auto_link_settings().await.expect("reads");
        assert_eq!(settings.mode, SyncMode::Manual);
    }

    /// A list made here has a temporary id until it reaches the server. Linking a remote list to
    /// that id would attach the link to something about to be given a different one.
    #[tokio::test]
    async fn a_list_made_here_waits_for_its_real_id_before_it_is_linked() {
        let fixture = fixture_with(auto_link_transport("all_google_to_astrid"));

        let report = fixture.service.auto_link_google().await.expect("links");

        assert_eq!(report.lists_created, 1);
        assert_eq!(report.waiting_to_be_created, 1);
        assert_eq!(report.linked, 0);
        assert!(
            fixture
                .store
                .lists()
                .expect("reads")
                .iter()
                .any(|list| list.name == "Groceries"),
            "and the list is there, so the next pass adopts it rather than making another"
        );
    }

    /// Without this, deleting an auto-linked list is pointless: the next pass sees an unlinked
    /// remote list and makes it again, and again after that.
    #[tokio::test]
    async fn a_remote_list_somebody_said_no_to_is_not_offered_again() {
        let fixture = fixture_with(auto_link_transport("all_google_to_astrid"));
        ledger::exclude(&fixture.store, PROVIDER_KEY, "c1").expect("excludes");

        let report = fixture.service.auto_link_google().await.expect("links");

        assert_eq!(report.linked, 0);
        assert_eq!(report.lists_created, 0);
        assert_eq!(report.failed, 0);
        assert!(
            fixture.transport.requests().iter().any(|request| {
                request.method.as_str() == "PATCH" && request.url.contains("integrations")
            }),
            "and the account is told, so the other devices stop offering it too"
        );
    }

    /// The default remote list pairs with My Tasks, not with a list of its own — so a mode that
    /// only mirrors outward leaves it alone.
    #[tokio::test]
    async fn the_default_remote_list_is_not_made_into_an_ordinary_list_when_mirroring_outward() {
        let fixture = fixture_with(
            StubTransport::new()
                .push_json(
                    "/api/v1/integrations",
                    200,
                    json!({
                        "integrations": [{
                            "provider": "GOOGLE_TASKS",
                            "metadata": { "googleSyncMode": "all_astrid_to_google" },
                        }],
                    }),
                )
                .push_json(
                    "google/tasklists",
                    200,
                    json!({
                        "tasklists": [{ "id": "default-list", "name": "My Tasks" }],
                        "defaultId": "default-list",
                    }),
                )
                .push_json("google/links", 200, json!({ "links": [] }))
                .push_json(
                    "google/tasklists",
                    200,
                    json!({ "tasklist": { "id": "c9", "name": "Work" } }),
                )
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );
        fixture
            .store
            .upsert_list(&a_list("cm3real", "Work"))
            .expect("writes");

        let report = fixture.service.auto_link_google().await.expect("links");

        assert_eq!(
            report.containers_created, 1,
            "the local list got a remote one of its own"
        );
        assert_eq!(report.linked, 1);
    }

    /// An account with no Google integration, and an account whose metadata has no tombstones,
    /// both have to be ordinary rather than an error.
    #[tokio::test]
    async fn a_status_without_tombstones_is_not_a_problem() {
        let fixture = fixture(200, json!([]));
        fixture
            .service
            .merge_server_tombstones(&json!({ "integrations": [] }))
            .expect("merges");
        fixture
            .service
            .merge_server_tombstones(&json!({}))
            .expect("merges");

        assert!(ledger::tombstoned(&fixture.store, PROVIDER_KEY).is_empty());
    }

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

    /// The whole point of the ledger: a task deleted here takes its twin with it, on a later pass,
    /// with nothing but what was written down at delete time.
    #[tokio::test]
    async fn a_task_deleted_here_has_its_twin_removed_over_there() {
        let fixture = fixture(200, json!([]));
        ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

        let report = fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        assert_eq!(report.removed_remotely, 1);
        assert!(
            ledger::pending(&fixture.store, PROVIDER_KEY).is_empty(),
            "the work is done, so it stops being pending"
        );
        assert!(
            ledger::tombstoned(&fixture.store, PROVIDER_KEY).contains(&"r1".to_string()),
            "but the deletion stays a fact, or the next pull brings it back"
        );
        let sent = fixture.transport.requests();
        assert_eq!(sent[0].method.as_str(), "DELETE");
        assert!(sent[0].url.contains("remoteId=r1"), "{}", sent[0].url);
    }

    /// A pass covers one container. Deleting out of another would remove somebody's task from a
    /// list this pass has nothing to do with.
    #[tokio::test]
    async fn a_pending_deletion_from_another_container_is_left_alone() {
        let fixture = fixture(200, json!([]));
        ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "other-list").expect("records");

        let report = fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        assert_eq!(report.removed_remotely, 0);
        assert_eq!(ledger::pending(&fixture.store, PROVIDER_KEY).len(), 1);
    }

    /// Somebody deleted it over there too. That is the work finished, not a failure to retry for
    /// ever.
    #[tokio::test]
    async fn a_twin_that_is_already_gone_stops_being_retried() {
        let fixture = fixture(404, json!([]));
        ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

        let report = fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        assert_eq!(report.removed_remotely, 0, "nothing was removed by us");
        assert!(ledger::pending(&fixture.store, PROVIDER_KEY).is_empty());
    }

    /// A server having a bad minute must not lose a deletion — the twin would stay for ever.
    #[tokio::test]
    async fn a_deletion_the_server_refused_is_kept_for_the_next_pass() {
        let fixture = fixture(500, json!([]));
        ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

        fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        assert_eq!(ledger::pending(&fixture.store, PROVIDER_KEY).len(), 1);
    }

    /// Without this, the deletion undoes itself: the twin is removed, the pull still lists it, and
    /// the task comes back on every pass for ever.
    #[tokio::test]
    async fn a_pull_refuses_to_bring_back_what_was_deleted_here() {
        let fixture = fixture(200, json!([{ "remoteId": "r1", "title": "Buy milk" }]));
        ledger::record_deletion(&fixture.store, PROVIDER_KEY, "r1", "tasklist-1").expect("records");

        let report = fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");

        assert_eq!(report.applied, 0);
        assert!(
            fixture.store.task("ext_r1").expect("reads").is_none(),
            "the task somebody deleted did not come back"
        );
    }
}
