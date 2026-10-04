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
//! A remote deletion is acted on when Google says so explicitly (`deleted`), or when a twin is
//! absent from a **complete** listing — never from a cursor page, a truncated listing or a failed
//! one. Deleting local tasks because a page did not mention them is how a dropped request wipes
//! somebody's list; see [`decisions::local_deletions`].
//!
//! ## Parity with Apple (AWTD2-56)
//!
//! The pass is `GoogleTasksSyncService.sync(link:)` and `syncMyTasks` from astrid-ios, in the same
//! order and with the same rules: the two watermarks on each server link and last-write-wins,
//! same-title adoption, no twin created without a complete listing, parents before children,
//! completion drift repair, deletion by absence, the completed backfill, and a cursor committed
//! only when every pulled item was dealt with.

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

    /// The name this crate's own records key it by — the ledger's `google`.
    pub(crate) fn slug(self) -> &'static str {
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

/// One remote item, as the proxy hands it over (`GET api/v1/sync/google/tasks`).
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
    /// Where the proxy actually puts deletion and nesting: strings under `metadata`
    /// (`deleted: "1"`, `parent: "<google id>"`). The top-level fields above are read too, for a
    /// proxy that ever sends them flat. Kept whole, because it is written back onto the link.
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    /// Google's completion time, when completed.
    #[serde(default)]
    completed_at: Option<String>,
    /// Google's `updated` — what the pull watermark is compared against. Kept as sent, so the
    /// watermark written back is exactly the stamp Google gave.
    #[serde(default)]
    remote_updated_at: Option<String>,
}

impl RemoteItem {
    fn meta(&self, key: &str) -> &str {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.get(key))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
    }

    fn is_deleted(&self) -> bool {
        self.deleted.unwrap_or(false) || self.meta("deleted") == "1"
    }

    /// Google's own id of the parent item, when nested.
    fn raw_parent(&self) -> Option<&str> {
        self.parent
            .as_deref()
            .filter(|parent| !parent.is_empty())
            .or_else(|| Some(self.meta("parent")).filter(|parent| !parent.is_empty()))
    }

    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.remote_updated_at.as_deref().and_then(date::parse)
    }
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
    /// Completed remote items imported as completed tasks this pass.
    pub backfilled: usize,
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
            Provider::GitHub => endpoints::GITHUB_SYNC_REPOS.to_string(),
        };
        let answer = self
            .context
            .client
            .send(self.context.client.get(path))
            .await?;
        let key = match provider {
            Provider::GoogleTasks => "tasklists",
            Provider::GitHub => "repos",
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

    async fn create_link(
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

    /// Best effort: the link is made either way, and a setting that could not be written is put
    /// right by the next manual link.
    async fn clear_exclusion(&self, container_id: &str) {
        let _ = ledger::include(&self.context.store, PROVIDER_KEY, container_id);
        let Ok(settings) = self.auto_link_settings().await else {
            return;
        };
        if !settings.excluded.iter().any(|id| id == container_id) {
            return;
        }
        let rest: Vec<String> = settings
            .excluded
            .into_iter()
            .filter(|id| id != container_id)
            .collect();
        let request = self
            .context
            .client
            .patch(endpoints::INTEGRATIONS)
            .value(json!({
                "provider": Provider::GoogleTasks.wire(),
                "metadata": { "excludedTasklists": rest.join(",") },
            }));
        let _ = self.context.client.send(request).await;
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

    /// One Google pass over one link. See [`Self::run_pass`] for the order and why.
    pub async fn sync_google_link(&self, link: &ExternalLink) -> Result<PassReport> {
        self.run_pass(Scope {
            container_id: &link.remote_container_id,
            link: Some(link),
            placement: Placement::InList(link.astrid_list_id.clone()),
            pulls: true,
            pushes: true,
        })
        .await
    }

    /// Remove the remote twins of tasks deleted on this machine, addressed either by link or by
    /// remote list — My Tasks has no link to name.
    ///
    /// A twin that is already gone counts as done: 404 and 410 both mean the work is finished, and
    /// retrying for ever because somebody deleted it over there too is not a failure worth keeping.
    /// Anything else is left pending, so a server having a bad minute does not lose the deletion.
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
                .create_link(Provider::GoogleTasks, &list_id, &action.tasklist_id)
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
                .create_link(Provider::GoogleTasks, &action.list_id, &container_id)
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
        self.run_pass(Scope {
            container_id: tasklist_id,
            link: None,
            placement: Placement::MyTasks(user_id),
            pulls: matches!(
                settings.mode,
                SyncMode::AllGoogleToAstrid | SyncMode::AllBidirectional
            ),
            pushes: matches!(
                settings.mode,
                SyncMode::AllAstridToGoogle | SyncMode::AllBidirectional
            ),
        })
        .await
    }

    // ── The pass ─────────────────────────────────────────────────────────────────────────────

    /// One pass over one container, in Apple's order (`GoogleTasksSyncService.sync(link:)` and
    /// `syncMyTasks`, AWTD2-56): remove what was deleted here, pull, push, then what only a
    /// complete listing can show — completion drift, deletions by absence, completed history —
    /// and last, for a linked list, commit the cursor.
    async fn run_pass(&self, scope: Scope<'_>) -> Result<PassReport> {
        let store = &self.context.store;
        let container = scope.container_id;
        let (address, address_value) = scope.address();
        let mut report = PassReport {
            removed_remotely: self
                .remove_twins(container, &[(address, &address_value)])
                .await?,
            ..Default::default()
        };

        let mut links = self.task_links(container).await?;
        let tombstoned = ledger::tombstoned(store, PROVIDER_KEY);
        report.linked += self.heal_links(container, &mut links, &tombstoned).await;

        // ── Pull ──
        let pulled = match scope.link {
            // The cursor is committed after the pass has been applied, so a client killed halfway
            // re-pulls rather than skipping what it never wrote down.
            Some(link) => {
                self.listing(&[("linkId", &link.id), ("deferCursor", "1")])
                    .await?
            }
            // My Tasks has no link row and so no cursor: its pull is the complete listing.
            None => self.listing(&[("tasklistId", container)]).await?,
        };
        report.pulled = pulled.items.len();
        report.truncated = pulled.truncated;
        let mut complete = Complete {
            listing: scope.link.is_none().then(|| pulled.clone()),
            tried: scope.link.is_none(),
        };

        // A cursor is an acknowledgement, not a best effort: one pulled item that could not be
        // applied and linked keeps the whole window replayable. (Apple `SyncPassAcknowledgement`.)
        let mut acknowledged = true;
        if scope.pulls {
            acknowledged = self
                .pull_items(&scope, &pulled.items, &mut links, &tombstoned, &mut report)
                .await?;
        }

        // The links an absent item may delete: those that existed before the push. A twin the push
        // creates is absent from a listing fetched before it, and must not read as deleted.
        let mut deletable: Vec<decisions::Link> = links
            .in_container(container)
            .map(|link| decisions::Link {
                task_id: link.task_id.clone(),
                remote_id: link.remote_id.clone(),
            })
            .collect();

        // ── Push ──
        let mut pushed_remote_ids = std::collections::HashSet::new();
        if scope.pushes {
            let pulled_by_remote: std::collections::HashMap<&str, &RemoteItem> = pulled
                .items
                .iter()
                .map(|item| (item.remote_id.as_str(), item))
                .collect();
            report.pushed = self
                .push_tasks(
                    &scope,
                    &mut links,
                    &pulled_by_remote,
                    &mut complete,
                    &tombstoned,
                    &mut pushed_remote_ids,
                    &mut report,
                )
                .await?;
            if let Placement::MyTasks(user_id) = &scope.placement {
                let retired = self
                    .retire_my_tasks_twins(container, user_id, &mut links)
                    .await?;
                deletable.retain(|link| !retired.contains(&link.remote_id));
            }
        }

        // ── What only a complete listing can show ──
        if scope.pulls {
            if scope.link.is_some() {
                // Throttled: a linked list's complete listing is fetched for deletions at most
                // every five minutes when the push did not need one. Only a complete one counts.
                let key = format!("external.fullPull.{container}");
                let now = self.context.clock.now();
                let last = store.metadata(&key)?.and_then(|stamp| date::parse(&stamp));
                if !complete.tried && decisions::full_pull_due(last, now) {
                    complete.listing = self.complete_listing(&scope).await;
                    complete.tried = true;
                }
                if complete
                    .listing
                    .as_ref()
                    .is_some_and(|listing| !listing.truncated)
                {
                    store.set_metadata(&key, &date::format(now))?;
                }
            }
            if let Some(listing) = &complete.listing {
                report.applied += self
                    .repair_drift(container, listing, &mut links, &pushed_remote_ids)
                    .await;
                report.deleted_locally += self.delete_absent(listing, &deletable)?;
                report.backfilled += self
                    .backfill(&scope, listing, &mut links, &tombstoned)
                    .await?;
            }
        }

        // ── The cursor ── only when the page was whole and every item was dealt with.
        if let Some(link) = scope.link {
            if acknowledged && !pulled.truncated {
                if let Some(cursor) = pulled.cursor.as_deref().filter(|cursor| !cursor.is_empty()) {
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
        }
        Ok(report)
    }

    /// One page of Google's items, as the proxy hands them over.
    async fn listing(&self, query: &[(&str, &str)]) -> Result<Listing> {
        let mut request = self.context.client.get(endpoints::GOOGLE_TASKS);
        for (name, value) in query {
            request = request.query(name, Some((*value).to_string()));
        }
        let answer = self.context.client.send(request).await?;
        // Item by item: one this build cannot read makes the listing incomplete, not empty. Read
        // as empty, every link in the list would look deleted.
        let raw = answer.get("items").and_then(|value| value.as_array());
        let mut whole = raw.is_some();
        let items = raw
            .into_iter()
            .flatten()
            .filter_map(|item| match serde_json::from_value(item.clone()) {
                Ok(item) => Some(item),
                Err(_) => {
                    whole = false;
                    None
                }
            })
            .collect();
        Ok(Listing {
            items,
            // Only an explicit "no" is trusted. A listing that does not say whether it was cut
            // short is treated as cut short, because absence from it proves nothing either way.
            truncated: !whole
                || answer
                    .get("truncated")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true),
            cursor: answer
                .get("cursor")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        })
    }

    /// A linked list's complete listing (`full=1`, cursor-free), or `None` when it could not be
    /// had — which is "unknown", never "empty".
    async fn complete_listing(&self, scope: &Scope<'_>) -> Option<Listing> {
        let (address, value) = scope.address();
        self.listing(&[(address, &value), ("full", "1")]).await.ok()
    }

    /// Apply one pull's items. Answers whether every one was dealt with, for the cursor.
    async fn pull_items(
        &self,
        scope: &Scope<'_>,
        items: &[RemoteItem],
        links: &mut LinkMap,
        tombstoned: &[String],
        report: &mut PassReport,
    ) -> Result<bool> {
        let store = &self.context.store;
        let container = scope.container_id;
        let mut acknowledged = true;
        // Parents before children, so a subtask created this pass finds its parent's fresh link.
        let ordered = decisions::parents_first(
            items,
            |item| item.remote_id.clone(),
            |item| decisions::parent_key(container, item.raw_parent()),
        );
        // Same-title adoption: a local task with no twin, the only one of its title, is the
        // item's twin rather than a reason to make a second. (Apple's adopt-candidates.)
        let mut adoptable = decisions::TitleIndex::new(
            store
                .tasks()?
                .into_iter()
                .filter(|task| {
                    scope.holds(task)
                        && !crate::model::is_temp_id(&task.id)
                        && !links.by_task.contains_key(&task.id)
                        && ledger::twin(store, PROVIDER_KEY, &task.id).is_none()
                })
                .map(|task| (task.id, task.title)),
        );

        for item in &ordered {
            let server_link = links.by_remote.get(&item.remote_id).cloned();
            // The server's map first, then this device's own. A task pulled while offline is not
            // on the server's map yet, and without the local answer the next pass would pull the
            // same item in a second time.
            let linked_task_id = server_link
                .as_ref()
                .map(|link| link.task_id.clone())
                .or_else(|| ledger::local_task_for(store, PROVIDER_KEY, &item.remote_id));
            let local = linked_task_id
                .as_deref()
                .and_then(|id| store.task(id).ok().flatten());

            match decisions::pull_outcome(
                item.is_deleted(),
                linked_task_id.is_some(),
                local.is_some(),
                tombstoned.contains(&item.remote_id),
            ) {
                PullOutcome::DeleteLocalTwin => {
                    // Tombstoned first, not pushed back: the deletion came from over there, and
                    // echoing it would be this device deleting an item that is already gone.
                    ledger::record_tombstone(store, PROVIDER_KEY, &item.remote_id)?;
                    // Through the task service, journalled: removed from the cache alone, the
                    // task came back with the next pull from astrid-web.
                    if let Some(task) = &local {
                        self.context.tasks().delete(&task.id)?;
                        links.remove_remote(&item.remote_id);
                        report.deleted_locally += 1;
                    }
                }
                PullOutcome::IgnoreDeletion | PullOutcome::SkipResurrection => {}
                PullOutcome::Apply => match local {
                    Some(task) => {
                        let remote_updated = item.updated_at();
                        if let Some(link) = &server_link {
                            // Our own echo, or older than what we last saw.
                            if !decisions::should_apply_remote(
                                remote_updated,
                                link.remote_updated_at,
                            ) {
                                continue;
                            }
                        }
                        // Last write wins: a Google change that lost the race to an edit made
                        // here must not clobber it — the push carries the local state out instead.
                        let local_unchanged = server_link.as_ref().is_some_and(|link| {
                            !decisions::should_push_local(task.updated_at, link.astrid_updated_at)
                        });
                        let (task, agreed_at) =
                            if decisions::remote_wins(remote_updated, task.updated_at)
                                || local_unchanged
                            {
                                let (task, changed) =
                                    self.apply_remote(item, container, task, links)?;
                                if changed {
                                    report.applied += 1;
                                }
                                let at = task.updated_at;
                                (task, at)
                            } else if server_link.is_some() {
                                continue;
                            } else {
                                // Known only here: nothing applied, but the server still lacks the
                                // link. Written without a local watermark, so the newer local
                                // state is still pushed rather than taken as agreed.
                                (task, None)
                            };
                        let written = self
                            .write_link(links, &task.id, item, container, agreed_at)
                            .await;
                        if !written {
                            acknowledged = false;
                        } else if server_link.is_none() {
                            report.linked += 1;
                        }
                    }
                    // Linked to a task this machine does not hold — not loaded yet, or deleted
                    // here and not yet on the server. Making one would be the duplicate; the
                    // window is kept so the item is seen again once the task is.
                    None if linked_task_id.is_some() => acknowledged = false,
                    None => {
                        // A completed item with nothing here is history, not work: importing it
                        // made an open task of it (Apple's "imported-open flood"). The backfill
                        // brings it in as completed.
                        if item.completed {
                            continue;
                        }
                        let task = match adoptable
                            .take_unique(&item.title)
                            .and_then(|id| store.task(&id).ok().flatten())
                        {
                            Some(adopted) => adopted,
                            None => self.create_from(item, container, &scope.placement, links)?,
                        };
                        report.applied += 1;
                        // Written down here as well as on the server: this is what a second pass
                        // reads when the first one's task has not reached astrid-web yet.
                        ledger::remember_links(
                            store,
                            PROVIDER_KEY,
                            container,
                            [(task.id.clone(), item.remote_id.clone())],
                        )?;
                        if self
                            .write_link(links, &task.id, item, container, task.updated_at)
                            .await
                        {
                            report.linked += 1;
                        } else {
                            acknowledged = false;
                        }
                    }
                },
            }
        }
        Ok(acknowledged)
    }

    /// Send what changed here: patch known twins, adopt or create the rest.
    #[allow(clippy::too_many_arguments)]
    async fn push_tasks(
        &self,
        scope: &Scope<'_>,
        links: &mut LinkMap,
        pulled: &std::collections::HashMap<&str, &RemoteItem>,
        complete: &mut Complete,
        tombstoned: &[String],
        pushed_remote_ids: &mut std::collections::HashSet<String>,
        report: &mut PassReport,
    ) -> Result<usize> {
        let container = scope.container_id;
        let zone = self.context.clock.time_zone();
        let mut held: Vec<Task> = self
            .context
            .store
            .tasks()?
            .into_iter()
            // A task that has never reached astrid-web has a temporary id, and linking a twin to
            // it would attach the twin to something about to be given a different id.
            .filter(|task| scope.holds(task) && !crate::model::is_temp_id(&task.id))
            .collect();
        // Parents before children, so a new subtask's parent already has its twin.
        held.sort_by_key(|task| task.parent_task_id.is_some());

        let mut pushed = 0;
        for task in &held {
            let due = task
                .due_date_time
                .map(|due| decisions::push_due(due, task.is_all_day, zone));
            if let Some(link) = links.for_task(&task.id).cloned() {
                // A link filed under another container is that container's pass's to push.
                if !decisions::may_push(&link.container_id, container) {
                    continue;
                }
                if !decisions::should_push_local(task.updated_at, link.astrid_updated_at) {
                    continue;
                }
                // An unchanged PATCH still moves Google's stamp, which comes back as a change.
                // Move the watermark instead.
                let snapshot = pulled.get(link.remote_id.as_str()).copied().or_else(|| {
                    complete.listing.as_ref().and_then(|listing| {
                        listing
                            .items
                            .iter()
                            .find(|item| item.remote_id == link.remote_id)
                    })
                });
                if snapshot.is_some_and(|remote| same_content(remote, task, due.as_deref())) {
                    self.record_task_link(
                        &task.id,
                        &link.remote_id,
                        container,
                        task.updated_at,
                        link.remote_updated_at.map(date::format).as_deref(),
                        None,
                    )
                    .await;
                    continue;
                }
                // One failure is that task's, not the list's: its watermark does not move, so it
                // is tried again next pass, and the rest still go.
                match self
                    .send_push(scope, task, due, Some(&link.remote_id), None)
                    .await
                {
                    Ok((_, remote_updated)) => {
                        self.record_task_link(
                            &task.id,
                            &link.remote_id,
                            container,
                            task.updated_at,
                            remote_updated.as_deref(),
                            None,
                        )
                        .await;
                        links.insert(TaskLink {
                            task_id: task.id.clone(),
                            remote_id: link.remote_id.clone(),
                            container_id: container.to_string(),
                            astrid_updated_at: task.updated_at,
                            remote_updated_at: remote_updated.as_deref().and_then(date::parse),
                        });
                        pushed_remote_ids.insert(link.remote_id.clone());
                        pushed += 1;
                    }
                    Err(error) => {
                        tracing::debug!(task = %task.id, %error, "push failed; retried next pass")
                    }
                }
                continue;
            }

            // A completed task with no twin is history: My Tasks mirrors what is to be done, and
            // making twins for it pushed the whole completed history into the default list.
            if matches!(scope.placement, Placement::MyTasks(_)) && task.completed {
                continue;
            }
            if !complete.tried {
                complete.listing = self.complete_listing(scope).await;
                complete.tried = true;
            }
            // Before creating a twin, an unlinked remote item of the same title in the complete
            // listing is adopted instead. Not a deleted one: linking to it would have the next
            // absence pass delete this task. (docs/CONTRACTS.md D39.)
            let candidate = complete.listing.as_ref().and_then(|listing| {
                listing
                    .items
                    .iter()
                    .find(|item| {
                        item.title == task.title
                            && !item.is_deleted()
                            && !links.by_remote.contains_key(&item.remote_id)
                            && !tombstoned.contains(&item.remote_id)
                    })
                    .cloned()
            });
            if let Some(candidate) = candidate {
                if self
                    .write_link(links, &task.id, &candidate, container, task.updated_at)
                    .await
                {
                    report.linked += 1;
                }
                continue;
            }
            // The twin may be past the end of a truncated listing, and a failed one is unknown,
            // not empty. Known twins are patched above; none is created on a guess.
            let truncated = complete
                .listing
                .as_ref()
                .is_none_or(|listing| listing.truncated);
            if !decisions::may_create_remote(complete.listing.is_some(), truncated, false) {
                continue;
            }
            let parent_remote_id = task
                .parent_task_id
                .as_ref()
                .and_then(|parent| links.by_task.get(parent))
                .cloned();
            match self
                .send_push(scope, task, due, None, parent_remote_id.as_deref())
                .await
            {
                Ok((remote_id, remote_updated)) if !remote_id.is_empty() => {
                    pushed += 1;
                    ledger::remember_links(
                        &self.context.store,
                        PROVIDER_KEY,
                        container,
                        [(task.id.clone(), remote_id.clone())],
                    )?;
                    // A create has made a twin only this response knows about. Writing the link
                    // down is what stops the next pass making a second one.
                    if self
                        .record_task_link(
                            &task.id,
                            &remote_id,
                            container,
                            task.updated_at,
                            remote_updated.as_deref(),
                            None,
                        )
                        .await
                    {
                        report.linked += 1;
                    }
                    links.insert(TaskLink {
                        task_id: task.id.clone(),
                        remote_id,
                        container_id: container.to_string(),
                        astrid_updated_at: task.updated_at,
                        remote_updated_at: remote_updated.as_deref().and_then(date::parse),
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(task = %task.id, %error, "push failed; retried next pass")
                }
            }
        }
        Ok(pushed)
    }

    /// One push: a patch when the twin is known, a create (under its parent's twin) when not.
    /// Answers with the twin's id and Google's new stamp.
    async fn send_push(
        &self,
        scope: &Scope<'_>,
        task: &Task,
        due: Option<String>,
        remote_id: Option<&str>,
        parent_remote_id: Option<&str>,
    ) -> Result<(String, Option<String>)> {
        let (address, value) = scope.address();
        let mut body = serde_json::Map::new();
        body.insert(address.to_string(), json!(value));
        body.insert("title".into(), json!(task.title));
        body.insert("notes".into(), json!(task.description));
        body.insert("dueDate".into(), json!(due));
        body.insert("completed".into(), json!(task.completed));
        body.insert("remoteId".into(), json!(remote_id));
        if let Some(parent) = parent_remote_id {
            body.insert("parentRemoteId".into(), json!(parent));
        }
        let request = self
            .context
            .client
            .post(endpoints::GOOGLE_TASKS)
            .value(serde_json::Value::Object(body));
        let answer = self.context.client.send(request).await?;
        let remote_id = answer
            .get("remoteId")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| remote_id.map(str::to_string))
            .unwrap_or_default();
        let updated = answer
            .get("remoteUpdatedAt")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        Ok((remote_id, updated))
    }

    /// A task that has LEFT My Tasks — it gained a list, or lost the assignment that put it
    /// here — still has a twin in the default list, which then acts as a second home for
    /// something that already has one. Close it out. A deleted task is not this: that goes
    /// through the ledger. Answers with the remote ids retired.
    async fn retire_my_tasks_twins(
        &self,
        tasklist_id: &str,
        user_id: &str,
        links: &mut LinkMap,
    ) -> Result<Vec<String>> {
        let mut retired = Vec::new();
        let candidates: Vec<TaskLink> = links.in_container(tasklist_id).cloned().collect();
        for link in candidates {
            let Some(task) = self.context.store.task(&link.task_id).ok().flatten() else {
                continue;
            };
            if is_my_task(&task, user_id) {
                continue;
            }
            let request = self
                .context
                .client
                .delete(endpoints::GOOGLE_TASKS)
                .query("tasklistId", Some(tasklist_id.to_string()))
                .query("remoteId", Some(link.remote_id.clone()));
            match self.context.client.send(request).await {
                Ok(_) => {}
                Err(crate::api::ApiError::Http { status, .. })
                    if decisions::remote_already_gone(status) => {}
                // A real failure is worth another go rather than a tombstone on a twin that is
                // still there.
                Err(_) => continue,
            }
            ledger::record_tombstone(&self.context.store, PROVIDER_KEY, &link.remote_id)?;
            ledger::forget_link(&self.context.store, PROVIDER_KEY, &link.task_id)?;
            links.remove_remote(&link.remote_id);
            retired.push(link.remote_id);
        }
        Ok(retired)
    }

    /// Completion drift: a linked pair whose completion disagrees, where the local task has not
    /// changed since the last pass, takes Google's. Old items never re-enter a cursor window, so
    /// without this one botched pass leaves the drift for ever. A pair pushed this pass has a
    /// stale snapshot here and is skipped. (Apple's drift repair, `CompletionDriftPolicy`.)
    async fn repair_drift(
        &self,
        container: &str,
        listing: &Listing,
        links: &mut LinkMap,
        pushed_remote_ids: &std::collections::HashSet<String>,
    ) -> usize {
        let mut repaired = 0;
        for item in listing.items.iter().filter(|item| !item.is_deleted()) {
            if pushed_remote_ids.contains(&item.remote_id) {
                continue;
            }
            let Some(link) = links.by_remote.get(&item.remote_id).cloned() else {
                continue;
            };
            let Some(task) = self.context.store.task(&link.task_id).ok().flatten() else {
                continue;
            };
            let local_unchanged =
                !decisions::should_push_local(task.updated_at, link.astrid_updated_at);
            if !decisions::should_adopt_remote_completion(
                item.completed,
                task.completed,
                task.completed_at,
                local_unchanged,
                task.is_repeating(),
            ) {
                continue;
            }
            let origin = crate::services::task::Origin {
                at: item.completed_at.as_deref().and_then(date::parse),
                source: Some("google".to_string()),
            };
            let Ok(done) = self.context.tasks().complete_as(
                &task.id,
                item.completed,
                Some(&task),
                None,
                &origin,
            ) else {
                continue;
            };
            repaired += 1;
            // The repair is Google's own state; watermarked, so it is not pushed back as an edit.
            self.write_link(links, &done.id, item, container, done.updated_at)
                .await;
        }
        repaired
    }

    /// Deletions by absence: a twin missing from a COMPLETE listing was deleted over there, and
    /// its local task goes.
    ///
    /// Delete, not detach: a Google id is scoped to its task list and Google models a move as a
    /// delete and an insert, so there is no same-task-elsewhere to preserve. The one mass-delete
    /// risk — incomplete data — is [`decisions::local_deletions`]'s: a truncated listing deletes
    /// nothing, and a failed one never gets here. (Apple's decision note on the same pass.)
    fn delete_absent(&self, listing: &Listing, deletable: &[decisions::Link]) -> Result<usize> {
        let present: Vec<String> = listing
            .items
            .iter()
            .filter(|item| !item.is_deleted())
            .map(|item| item.remote_id.clone())
            .collect();
        let mut deleted = 0;
        for link in decisions::local_deletions(deletable, Some(&present), listing.truncated, &[]) {
            if self.context.store.task(&link.task_id)?.is_none() {
                continue;
            }
            // Tombstoned first, so the delete's own capture sees a twin already gone rather than
            // queueing its removal — and the pull never brings it back.
            ledger::record_tombstone(&self.context.store, PROVIDER_KEY, &link.remote_id)?;
            self.context.tasks().delete(&link.task_id)?;
            deleted += 1;
        }
        Ok(deleted)
    }

    /// Completed history, imported gradually as completed tasks: newest first, a budget a pass,
    /// never ahead of the live items. A completed local task of the same unambiguous title is
    /// adopted rather than duplicated — a pass that made it but could not link it. (Apple's
    /// completed backfill and `BackfillAdoptionIndex`.)
    async fn backfill(
        &self,
        scope: &Scope<'_>,
        listing: &Listing,
        links: &mut LinkMap,
        tombstoned: &[String],
    ) -> Result<usize> {
        let store = &self.context.store;
        let container = scope.container_id;
        let noted: std::collections::HashSet<String> =
            ledger::links_in(store, PROVIDER_KEY, container)
                .into_iter()
                .map(|(_, remote_id)| remote_id)
                .collect();
        let candidates: Vec<decisions::BackfillCandidate> = listing
            .items
            .iter()
            .map(|item| decisions::BackfillCandidate {
                remote_id: item.remote_id.clone(),
                completed: item.completed,
                deleted: item.is_deleted(),
                updated_at: item.remote_updated_at.clone().unwrap_or_default(),
            })
            .collect();
        let chosen: Vec<String> = decisions::backfill_selection(
            &candidates,
            |remote_id| links.by_remote.contains_key(remote_id) || noted.contains(remote_id),
            tombstoned,
            decisions::BACKFILL_BUDGET,
        )
        .into_iter()
        .map(|candidate| candidate.remote_id.clone())
        .collect();
        if chosen.is_empty() {
            return Ok(0);
        }

        let mut adoptable = decisions::TitleIndex::new(
            store
                .tasks()?
                .into_iter()
                .filter(|task| {
                    scope.holds(task)
                        && task.completed
                        && !crate::model::is_temp_id(&task.id)
                        && !links.by_task.contains_key(&task.id)
                })
                .map(|task| (task.id, task.title)),
        );
        let mut imported = 0;
        for remote_id in chosen {
            let Some(item) = listing
                .items
                .iter()
                .find(|item| item.remote_id == remote_id)
            else {
                continue;
            };
            let task = match adoptable
                .take_unique(&item.title)
                .and_then(|id| store.task(&id).ok().flatten())
            {
                Some(adopted) => adopted,
                None => {
                    let created = self.create_from(item, container, &scope.placement, links)?;
                    let origin = crate::services::task::Origin {
                        at: item
                            .completed_at
                            .as_deref()
                            .and_then(date::parse)
                            .or_else(|| item.updated_at()),
                        source: Some("google".to_string()),
                    };
                    self.context.tasks().complete_as(
                        &created.id,
                        true,
                        Some(&created),
                        None,
                        &origin,
                    )?
                }
            };
            ledger::remember_links(
                store,
                PROVIDER_KEY,
                container,
                [(task.id.clone(), item.remote_id.clone())],
            )?;
            self.write_link(links, &task.id, item, container, task.updated_at)
                .await;
            imported += 1;
        }
        Ok(imported)
    }

    /// The links this device noted and the server lacks, written up now that their tasks have
    /// reached astrid-web. A task pulled or backfilled while its create was still in the journal
    /// could not be linked then; its item may never come back into a cursor window, and every
    /// other device would make a second twin of it.
    async fn heal_links(
        &self,
        container: &str,
        links: &mut LinkMap,
        tombstoned: &[String],
    ) -> usize {
        let mut healed = 0;
        for (task_id, remote_id) in ledger::links_in(&self.context.store, PROVIDER_KEY, container) {
            if crate::model::is_temp_id(&task_id)
                || links.by_remote.contains_key(&remote_id)
                || links.by_task.contains_key(&task_id)
                || tombstoned.contains(&remote_id)
                || self.context.store.task(&task_id).ok().flatten().is_none()
            {
                continue;
            }
            // No watermarks: nothing here says when either side last agreed, and claiming they
            // did would swallow an edit.
            if self
                .record_task_link(&task_id, &remote_id, container, None, None, None)
                .await
            {
                healed += 1;
                links.insert(TaskLink {
                    task_id,
                    remote_id,
                    container_id: container.to_string(),
                    astrid_updated_at: None,
                    remote_updated_at: None,
                });
            }
        }
        healed
    }

    /// The links the server keeps for one container, watermarks included.
    async fn task_links(&self, container_id: &str) -> Result<LinkMap> {
        let request = self
            .context
            .client
            .get(endpoints::GOOGLE_TASK_LINKS)
            .query("containerId", Some(container_id.to_string()));
        let answer = self.context.client.send(request).await?;
        let mut map = LinkMap::default();
        // `{ links: [{ astridTaskId, remoteId, remoteContainerId, astridUpdatedAt,
        // remoteUpdatedAt, … }] }` is what the route answers (`api/v1/sync/google/task-links`);
        // the other spellings are kept for an older proxy.
        for link in answer
            .get("links")
            .or_else(|| answer.get("taskLinks"))
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            let text = |key: &str| link.get(key).and_then(|value| value.as_str());
            let (Some(remote), Some(task)) = (
                text("remoteId"),
                text("astridTaskId").or_else(|| text("taskId")),
            ) else {
                continue;
            };
            map.insert(TaskLink {
                task_id: task.to_string(),
                remote_id: remote.to_string(),
                // The row's own container; else the one a `container:task` remote id names.
                container_id: text("remoteContainerId")
                    .map(str::to_string)
                    .or_else(|| {
                        remote
                            .split_once(':')
                            .map(|(container, _)| container.to_string())
                    })
                    .unwrap_or_else(|| container_id.to_string()),
                astrid_updated_at: text("astridUpdatedAt").and_then(date::parse),
                remote_updated_at: text("remoteUpdatedAt").and_then(date::parse),
            });
        }
        // Written down for the delete-time capture: a deletion cannot ask the server which remote
        // item a task was, so a pass has to have said so first.
        ledger::remember_links(
            &self.context.store,
            PROVIDER_KEY,
            container_id,
            map.in_container(container_id)
                .map(|link| (link.task_id.clone(), link.remote_id.clone())),
        )?;
        Ok(map)
    }

    /// Make a local task of one pulled item. Through the task service, not straight into the
    /// cache: a pulled task has to reach astrid-web — it is an Astrid task now, and one that
    /// existed only in this machine's cache would be invisible on the web, absent on the phone,
    /// and gone at the next sign-out.
    fn create_from(
        &self,
        item: &RemoteItem,
        container_id: &str,
        placement: &Placement,
        links: &LinkMap,
    ) -> Result<Task> {
        let mut draft = crate::services::TaskDraft::new(item.title.clone());
        draft.description = item.notes.clone().unwrap_or_default();
        match placement {
            // The link's list, so a pulled task appears where somebody expects it — and the
            // person's, as the Apple apps make it: a task from their own Google list is theirs.
            Placement::InList(list_id) => {
                draft.list_ids = vec![list_id.clone()];
                draft.assignee_id = self.context.account().current_user_id().ok().flatten();
            }
            // My Tasks is not a list: it is "assigned to me, in no list", so that is what a task
            // pulled from the default remote list has to become.
            Placement::MyTasks(user_id) => draft.assignee_id = Some(user_id.clone()),
        }
        // Google Tasks has no time of day, so a due date is a calendar day — which is exactly what
        // an all-day task is here.
        draft.due_date_time = item.due_date.as_deref().and_then(date::parse);
        draft.is_all_day = draft.due_date_time.is_some();
        draft.parent_task_id = parent_of(item, container_id, links);
        self.context.tasks().create(&draft)
    }

    /// Bring one pulled item's changes onto its local twin. Answers with the task, and whether
    /// anything changed.
    fn apply_remote(
        &self,
        item: &RemoteItem,
        container_id: &str,
        task: Task,
        links: &LinkMap,
    ) -> Result<(Task, bool)> {
        let tasks = self.context.tasks();
        let parent = parent_of(item, container_id, links);
        let mut changes = crate::services::TaskChanges::default();
        if task.title != item.title {
            changes.title = Some(item.title.clone());
        }
        // Google leaves `notes` out when there are none, so absent is empty — as on Apple. Safe
        // now that last-write-wins guards the call: a pending local edit never gets here.
        let notes = item.notes.clone().unwrap_or_default();
        if task.description != notes {
            changes.description = Some(notes);
        }
        let due = item.due_date.as_deref().and_then(date::parse);
        if let Some(adopted) = decisions::adopted_due(due, task.due_date_time, task.is_all_day) {
            changes.due_date_time = Some(Some(adopted));
            changes.is_all_day = Some(true);
        }
        // Only a parent that resolves: one this pass cannot place is not evidence the task was
        // un-nested over there.
        if parent.is_some() && task.parent_task_id != parent {
            changes.parent_task_id = Some(parent);
        }
        let mut changed = changes != crate::services::TaskChanges::default();
        let task = if changed {
            tasks.update(&task.id, &changes)?
        } else {
            task
        };
        // Unchanged-here is already established by the caller's last-write-wins guard.
        if decisions::should_adopt_remote_completion(
            item.completed,
            task.completed,
            task.completed_at,
            true,
            task.is_repeating(),
        ) {
            // Through the completion path, because a repeating task rolls forward rather than
            // being ticked off — after the item's other changes, which completing must not drop.
            // Google's own completion time and source, so the history says when and from where.
            let origin = crate::services::task::Origin {
                at: item.completed_at.as_deref().and_then(date::parse),
                source: Some("google".to_string()),
            };
            changed = true;
            return Ok((
                tasks.complete_as(&task.id, item.completed, Some(&task), None, &origin)?,
                changed,
            ));
        }
        Ok((task, changed))
    }

    /// Link a task to a pulled or listed item, watermarked with both sides' stamps, in the pass's
    /// map and on the server. Answers whether the server took it.
    async fn write_link(
        &self,
        links: &mut LinkMap,
        task_id: &str,
        item: &RemoteItem,
        container_id: &str,
        astrid_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> bool {
        let task_id = self
            .context
            .store
            .resolve_id(task_id)
            .unwrap_or_else(|_| task_id.to_string());
        links.insert(TaskLink {
            task_id: task_id.clone(),
            remote_id: item.remote_id.clone(),
            container_id: container_id.to_string(),
            astrid_updated_at,
            remote_updated_at: item.updated_at(),
        });
        self.record_task_link(
            &task_id,
            &item.remote_id,
            container_id,
            astrid_updated_at,
            item.remote_updated_at.as_deref(),
            item.metadata.as_ref(),
        )
        .await
    }

    /// Tell the server which remote item a task mirrors, and the watermarks both sides agreed at.
    ///
    /// Without this the link exists nowhere: the next pass reads an empty map, sees a task with no
    /// remote twin, and creates a second one over there — every pass, for ever.
    ///
    /// A task still carrying a temporary id is skipped rather than sent: the link row is a foreign
    /// key onto the task, and the server rejects an id it has never seen. A later pass, once the
    /// Outbox has been through, does it.
    async fn record_task_link(
        &self,
        task_id: &str,
        remote_id: &str,
        container_id: &str,
        astrid_updated_at: Option<chrono::DateTime<chrono::Utc>>,
        remote_updated_at: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> bool {
        let task_id = self
            .context
            .store
            .resolve_id(task_id)
            .unwrap_or_else(|_| task_id.to_string());
        if crate::model::is_temp_id(&task_id) {
            return false;
        }
        let mut body = json!({
            "astridTaskId": task_id,
            "remoteId": remote_id,
            "remoteContainerId": container_id,
        });
        // Absent rather than null when unknown: the route leaves a stamp it is not sent alone.
        if let Some(at) = astrid_updated_at {
            body["astridUpdatedAt"] = json!(date::format(at));
        }
        if let Some(at) = remote_updated_at {
            body["remoteUpdatedAt"] = json!(at);
        }
        if let Some(metadata) = metadata {
            body["metadata"] = metadata.clone();
        }
        let request = self
            .context
            .client
            .put(endpoints::GOOGLE_TASK_LINKS)
            .value(body);
        self.context.client.send(request).await.is_ok()
    }
}

/// What one pass covers: a linked list, or My Tasks against the default remote list.
struct Scope<'a> {
    container_id: &'a str,
    /// The link, for a linked list: its id addresses every request, and it has a cursor.
    link: Option<&'a ExternalLink>,
    placement: Placement,
    pulls: bool,
    pushes: bool,
}

impl Scope<'_> {
    /// How the proxy is told which container: by link, or — My Tasks has no link — by task list.
    fn address(&self) -> (&'static str, String) {
        match self.link {
            Some(link) => ("linkId", link.id.clone()),
            None => ("tasklistId", self.container_id.to_string()),
        }
    }

    /// Whether a local task belongs to what this pass mirrors.
    fn holds(&self, task: &Task) -> bool {
        match &self.placement {
            Placement::InList(list_id) => task
                .list_ids
                .as_ref()
                .is_some_and(|lists| lists.contains(list_id)),
            Placement::MyTasks(user_id) => is_my_task(task, user_id),
        }
    }
}

/// My Tasks: unlisted, and assigned to you.
fn is_my_task(task: &Task, user_id: &str) -> bool {
    task.list_ids.as_ref().is_none_or(|lists| lists.is_empty())
        && task.assignee_id.as_deref() == Some(user_id)
}

/// One answer from the proxy's listing.
#[derive(Debug, Clone)]
struct Listing {
    items: Vec<RemoteItem>,
    truncated: bool,
    cursor: Option<String>,
}

/// The complete listing, fetched at most once a pass and only when something needs it.
struct Complete {
    listing: Option<Listing>,
    tried: bool,
}

/// One task link as the server holds it, with the two watermarks the Apple apps write on it:
/// `astridUpdatedAt` guards the push, `remoteUpdatedAt` the pull.
#[derive(Debug, Clone)]
struct TaskLink {
    task_id: String,
    remote_id: String,
    container_id: String,
    astrid_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    remote_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The pass's view of the links, kept current as it writes new ones.
#[derive(Debug, Default)]
struct LinkMap {
    by_remote: std::collections::HashMap<String, TaskLink>,
    /// Task id → remote id.
    by_task: std::collections::HashMap<String, String>,
}

impl LinkMap {
    fn insert(&mut self, link: TaskLink) {
        if let Some(previous) = self.by_task.get(&link.task_id).cloned() {
            if previous != link.remote_id {
                self.by_remote.remove(&previous);
            }
        }
        if let Some(previous) = self.by_remote.get(&link.remote_id) {
            if previous.task_id != link.task_id {
                self.by_task.remove(&previous.task_id);
            }
        }
        self.by_task
            .insert(link.task_id.clone(), link.remote_id.clone());
        self.by_remote.insert(link.remote_id.clone(), link);
    }

    fn remove_remote(&mut self, remote_id: &str) {
        if let Some(link) = self.by_remote.remove(remote_id) {
            self.by_task.remove(&link.task_id);
        }
    }

    fn for_task(&self, task_id: &str) -> Option<&TaskLink> {
        self.by_remote.get(self.by_task.get(task_id)?)
    }

    fn in_container<'a>(&'a self, container_id: &'a str) -> impl Iterator<Item = &'a TaskLink> {
        self.by_remote
            .values()
            .filter(move |link| link.container_id == container_id)
    }
}

/// The local parent of a pulled subtask, when it is a task we hold. The key is scoped to the
/// container because Google reuses short task ids between lists — see `decisions::parent_key`.
fn parent_of(item: &RemoteItem, container_id: &str, links: &LinkMap) -> Option<String> {
    let key = decisions::parent_key(container_id, item.raw_parent())?;
    links.by_remote.get(&key).map(|link| link.task_id.clone())
}

/// Whether a remote item already says what the local task says, so a patch would change nothing
/// but Google's stamp.
fn same_content(remote: &RemoteItem, task: &Task, due: Option<&str>) -> bool {
    remote.title == task.title
        && remote.notes.as_deref().unwrap_or_default() == task.description
        && remote.completed == task.completed
        && remote.due_date.as_deref().and_then(date::parse) == due.and_then(date::parse)
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
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .push_json("google/task-links", 200, json!({ "links": [] })),
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
                .push_json("google/task-links", 200, json!({ "links": [] }))
                // The push: the complete listing that proves no twin exists (AWTD2-56), the
                // create, and the link that follows it.
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .push_json(
                    "google/tasks",
                    200,
                    json!({ "items": [], "cursor": null, "truncated": false }),
                )
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
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .push_json("google/task-links", 200, json!({ "links": [] }))
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
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .push_json("google/task-links", 200, json!({ "links": [] }))
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
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .push_json("google/tasks", 200, items)
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .push_json("google/task-links", 200, json!({ "links": [] }))
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
            .push_json("google/task-links", 200, json!({ "links": [] }))
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
                        "links": [{ "remoteId": "default-list:r1", "astridTaskId": "cm3real" }],
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

    /// The proxy's real shapes (`api/v1/sync/google/{tasks,task-links}`): links under `links`
    /// naming `astridTaskId`, and a deletion as `metadata.deleted: "1"`. Read with other names,
    /// every link looked absent and no remote deletion was ever seen.
    #[tokio::test]
    async fn a_deletion_in_google_reaches_the_linked_task_in_the_proxys_shapes() {
        let fixture = fixture_with(
            StubTransport::new()
                .push_json(
                    "google/tasks",
                    200,
                    json!({ "items": [{
                        "remoteId": "tasklist-1:r1", "title": "Buy milk",
                        "metadata": { "googleTaskId": "r1", "parent": "", "deleted": "1" }
                    }], "cursor": null, "truncated": false }),
                )
                .push_json(
                    "google/task-links",
                    200,
                    json!({ "links": [{ "remoteId": "tasklist-1:r1", "astridTaskId": "t-local",
                                        "remoteContainerId": "tasklist-1" }] }),
                )
                .push_json("google/task-links", 200, json!({ "links": [] }))
                .fallback(Ok(crate::api::HttpResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: b"{}".to_vec(),
                })),
        );
        fixture
            .store
            .upsert_task(&crate::model::Task::new("t-local", "Buy milk"))
            .expect("stores");

        let report = fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");
        assert_eq!(report.deleted_locally, 1, "{report:?}");
        assert!(fixture.store.task("t-local").expect("reads").is_none());
        let journal = crate::outbox::journal::all(&fixture.store).expect("reads");
        assert!(
            journal
                .iter()
                .any(|entry| entry.kind == crate::outbox::kind::DELETE_TASK),
            "the deletion reaches astrid-web, or the next pull brings the task back"
        );
    }

    /// A task pulled before its create had reached astrid-web is matched through this device's own
    /// note on the next pass — and must then be linked on the server too, or every other device
    /// makes a second twin of it.
    #[tokio::test]
    async fn a_twin_known_only_here_is_linked_on_the_server_once_it_can_be() {
        let transport = StubTransport::new()
            .push_json(
                "google/tasks",
                200,
                json!({ "items": [{ "remoteId": "tasklist-1:r1", "title": "Buy milk" }] }),
            )
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .push_json("google/task-links", 200, json!({ "links": [] }))
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            }));
        let sent = transport.recorded.clone();
        let fixture = fixture_with(transport);
        fixture
            .store
            .upsert_task(&crate::model::Task::new("t-real", "Buy milk"))
            .expect("stores");
        ledger::remember_links(
            &fixture.store,
            PROVIDER_KEY,
            "tasklist-1",
            [("t-real".to_string(), "tasklist-1:r1".to_string())],
        )
        .expect("remembers");

        fixture
            .service
            .sync_google_link(&link())
            .await
            .expect("a pass");
        let linked = sent.lock().expect("lock").iter().any(|request| {
            request.method == crate::api::Method::Put
                && request.url.contains("google/task-links")
                && request
                    .body
                    .as_ref()
                    .is_some_and(|body| String::from_utf8_lossy(body).contains("t-real"))
        });
        assert!(linked, "the server was told which task mirrors r1");
    }
}

/// AWTD2-56 — the Google pass at parity with Apple's `GoogleTasksSyncService`.
///
/// Against a stateful fake of astrid-web's proxy (`api/v1/sync/google/{tasks,task-links}`), so a
/// test asserts what a pass leaves behind — on this machine and over there — rather than the order
/// it happened to ask in.
#[cfg(test)]
mod awtd2_56 {
    use super::*;

    use crate::api::{ApiClient, HttpRequest, HttpResponse, HttpTransport, Method, TransportError};
    use crate::model::date;
    use crate::platform::{FixedClock, MemorySecureStore};
    use crate::store::Store;
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    const NOW: &str = "2026-09-07T12:00:00Z";
    const CONTAINER: &str = "tasklist-1";

    /// What the proxy holds: Google's items for one task list, and the server's link rows.
    #[derive(Default)]
    struct Proxy {
        /// Proxy-shaped items (`remoteId`, `title`, `metadata`, …).
        items: Vec<Value>,
        /// What a cursor pull answers with, when it is not everything.
        window: Option<Vec<Value>>,
        links: Vec<Value>,
        /// The complete listing (`full=1`) says it was cut short.
        full_truncated: bool,
        /// The complete listing fails.
        full_fails: bool,
        /// Titles whose push Google refuses.
        refuse: Vec<String>,
        next_id: u32,
        sync_mode: String,
        excluded: String,
    }

    struct FakeProxy {
        state: Mutex<Proxy>,
        sent: Mutex<Vec<HttpRequest>>,
    }

    fn ok(body: Value) -> std::result::Result<HttpResponse, TransportError> {
        status(200, body)
    }

    fn status(code: u16, body: Value) -> std::result::Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: code,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        })
    }

    #[async_trait]
    impl HttpTransport for FakeProxy {
        async fn send(
            &self,
            request: HttpRequest,
        ) -> std::result::Result<HttpResponse, TransportError> {
            self.sent.lock().expect("lock").push(request.clone());
            let body: Value = request
                .body
                .as_deref()
                .and_then(|body| serde_json::from_slice(body).ok())
                .unwrap_or(Value::Null);
            let mut proxy = self.state.lock().expect("lock");
            let url = request.url.as_str();
            if url.contains("/api/v1/integrations") {
                return ok(json!({ "integrations": [{
                    "provider": "GOOGLE_TASKS",
                    "metadata": {
                        "googleSyncMode": proxy.sync_mode,
                        "excludedTasklists": proxy.excluded,
                    },
                }] }));
            }
            if url.contains("google/task-links") {
                return match request.method {
                    Method::Get => ok(json!({ "links": proxy.links })),
                    Method::Put => {
                        let task_id = body["astridTaskId"].clone();
                        proxy.links.retain(|link| link["astridTaskId"] != task_id);
                        proxy.links.push(json!({
                            "astridTaskId": task_id,
                            "remoteId": body["remoteId"],
                            "remoteContainerId": body["remoteContainerId"],
                            "astridUpdatedAt": body["astridUpdatedAt"],
                            "remoteUpdatedAt": body["remoteUpdatedAt"],
                        }));
                        ok(json!({ "link": {} }))
                    }
                    _ => ok(json!({})),
                };
            }
            if url.contains("google/tasks") {
                match request.method {
                    Method::Get if url.contains("full=1") => {
                        if proxy.full_fails {
                            return status(502, json!({ "error": "Google error" }));
                        }
                        return ok(json!({
                            "items": proxy.items,
                            "cursor": null,
                            "truncated": proxy.full_truncated,
                        }));
                    }
                    Method::Get => {
                        let items = proxy.window.clone().unwrap_or_else(|| proxy.items.clone());
                        return ok(
                            json!({ "items": items, "cursor": "c-next", "truncated": false }),
                        );
                    }
                    Method::Post if body["action"] == "commitCursor" => {
                        return ok(json!({ "ok": true }))
                    }
                    Method::Post => {
                        let title = body["title"].as_str().unwrap_or_default().to_string();
                        if proxy.refuse.contains(&title) {
                            return status(404, json!({ "error": "Google error" }));
                        }
                        let stamp = "2026-09-07T12:00:05.000Z";
                        let completed = body["completed"].as_bool().unwrap_or(false);
                        if let Some(remote_id) = body["remoteId"].as_str() {
                            let Some(item) = proxy
                                .items
                                .iter_mut()
                                .find(|item| item["remoteId"] == remote_id)
                            else {
                                return status(404, json!({ "error": "Google error" }));
                            };
                            item["title"] = json!(title);
                            item["notes"] = body["notes"].clone();
                            item["completed"] = json!(completed);
                            item["remoteUpdatedAt"] = json!(stamp);
                            return ok(json!({ "remoteId": remote_id, "remoteUpdatedAt": stamp }));
                        }
                        proxy.next_id += 1;
                        let remote_id = format!("{CONTAINER}:new{}", proxy.next_id);
                        let parent = body["parentRemoteId"]
                            .as_str()
                            .and_then(|parent| parent.split(':').next_back())
                            .unwrap_or_default()
                            .to_string();
                        proxy.items.push(json!({
                            "remoteId": remote_id,
                            "title": title,
                            "notes": body["notes"],
                            "completed": completed,
                            "remoteUpdatedAt": stamp,
                            "metadata": { "parent": parent, "deleted": "" },
                        }));
                        return ok(json!({ "remoteId": remote_id, "remoteUpdatedAt": stamp }));
                    }
                    Method::Delete => {
                        let remote = url
                            .split("remoteId=")
                            .nth(1)
                            .unwrap_or_default()
                            .to_string();
                        let remote = remote.replace("%3A", ":");
                        proxy
                            .items
                            .retain(|item| item["remoteId"] != remote.as_str());
                        return ok(json!({ "success": true }));
                    }
                    _ => {}
                }
            }
            ok(json!({}))
        }
    }

    struct Pass {
        service: ExternalSyncService,
        store: Arc<Store>,
        proxy: Arc<FakeProxy>,
    }

    impl Pass {
        fn new(proxy: Proxy) -> Self {
            let proxy = Arc::new(FakeProxy {
                state: Mutex::new(proxy),
                sent: Mutex::new(Vec::new()),
            });
            let store = Arc::new(Store::in_memory().expect("opens"));
            let context = Context::new(
                Arc::new(ApiClient::new(
                    "https://astrid.cc",
                    proxy.clone(),
                    Arc::new(MemorySecureStore::new()),
                )),
                store.clone(),
                Arc::new(FixedClock::at(date::parse(NOW).expect("an instant"))),
            );
            let mut user = crate::model::User::new("u1");
            user.name = Some("Ada".into());
            store
                .set_metadata(
                    "account.current-user",
                    &serde_json::to_string(&user).expect("encodes"),
                )
                .expect("writes");
            Pass {
                service: context.external(),
                store,
                proxy,
            }
        }

        async fn run(&self) -> PassReport {
            self.service
                .sync_google_link(&ExternalLink {
                    id: "link-1".into(),
                    astrid_list_id: "l1".into(),
                    remote_container_id: CONTAINER.into(),
                    cursor: None,
                })
                .await
                .expect("a pass")
        }

        fn task(&self, id: &str) -> Option<Task> {
            self.store.task(id).expect("reads")
        }

        fn titled(&self, title: &str) -> Vec<Task> {
            self.store
                .tasks()
                .expect("reads")
                .into_iter()
                .filter(|task| task.title == title)
                .collect()
        }

        fn sent(&self, method: Method, fragment: &str) -> Vec<Value> {
            self.proxy
                .sent
                .lock()
                .expect("lock")
                .iter()
                .filter(|request| request.method == method && request.url.contains(fragment))
                .map(|request| {
                    request
                        .body
                        .as_deref()
                        .and_then(|body| serde_json::from_slice(body).ok())
                        .unwrap_or(Value::Null)
                })
                .collect()
        }

        /// Pushes to Google: creates and patches, not cursor commits.
        fn pushes(&self) -> Vec<Value> {
            self.sent(Method::Post, "google/tasks")
                .into_iter()
                .filter(|body| body["action"].is_null())
                .collect()
        }

        fn remote(&self, remote_id: &str) -> Option<Value> {
            let proxy = self.proxy.state.lock().expect("lock");
            proxy
                .items
                .iter()
                .find(|item| item["remoteId"] == remote_id)
                .cloned()
        }

        fn server_link(&self, task_id: &str) -> Option<Value> {
            let proxy = self.proxy.state.lock().expect("lock");
            proxy
                .links
                .iter()
                .find(|link| link["astridTaskId"] == task_id)
                .cloned()
        }
    }

    fn item(id: &str, title: &str, updated: &str) -> Value {
        json!({
            "remoteId": format!("{CONTAINER}:{id}"),
            "title": title,
            "notes": null,
            "completed": false,
            "dueDate": null,
            "completedAt": null,
            "remoteUpdatedAt": updated,
            "metadata": { "googleTaskId": id, "parent": "", "position": "", "deleted": "" },
        })
    }

    fn link_row(task: &str, remote: &str, astrid_at: &str, remote_at: &str) -> Value {
        json!({
            "astridTaskId": task,
            "remoteId": format!("{CONTAINER}:{remote}"),
            "remoteContainerId": CONTAINER,
            "astridUpdatedAt": astrid_at,
            "remoteUpdatedAt": remote_at,
        })
    }

    /// A task in the linked list, with a server id and a stamp.
    fn listed(id: &str, title: &str, updated: &str) -> Task {
        let mut task = Task::new(id, title);
        task.list_ids = Some(vec!["l1".into()]);
        task.updated_at = date::parse(updated);
        task
    }

    // ── Last write wins ─────────────────────────────────────────────────────────────────────

    /// AWTD2-56: an edit made here and not yet mirrored is newer than Google's change, so the pull
    /// must leave it alone — and the push carries it out. The old pass overwrote it.
    #[tokio::test]
    async fn awtd2_56_a_pull_does_not_overwrite_a_fresher_local_edit() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Google's title", "2026-09-07T11:30:00.000Z")],
            links: vec![link_row(
                "t1",
                "r1",
                "2026-09-07T11:00:00.000Z",
                "2026-09-07T11:00:00.000Z",
            )],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "My edit", "2026-09-07T11:45:00Z"))
            .expect("writes");

        pass.run().await;

        assert_eq!(pass.task("t1").expect("kept").title, "My edit");
        assert_eq!(
            pass.remote("tasklist-1:r1").expect("there")["title"],
            "My edit",
            "and the local edit went out"
        );
    }

    /// AWTD2-56: the other half — a Google change newer than anything here is taken.
    #[tokio::test]
    async fn awtd2_56_a_newer_remote_change_is_taken() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Google's title", "2026-09-07T11:50:00.000Z")],
            links: vec![link_row(
                "t1",
                "r1",
                "2026-09-07T11:00:00.000Z",
                "2026-09-07T11:00:00.000Z",
            )],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Old title", "2026-09-07T11:00:00Z"))
            .expect("writes");

        let report = pass.run().await;

        assert_eq!(report.applied, 1);
        assert_eq!(pass.task("t1").expect("kept").title, "Google's title");
        let link = pass.server_link("t1").expect("linked");
        assert_eq!(
            link["remoteUpdatedAt"], "2026-09-07T11:50:00.000Z",
            "the pull watermark moved, so the change is not applied twice"
        );
        assert!(
            link["astridUpdatedAt"].is_string(),
            "and the local one, so the change is not echoed back"
        );
        assert!(
            pass.pushes().is_empty(),
            "nothing was echoed back: {:?}",
            pass.pushes()
        );
    }

    /// AWTD2-56: an item at its own watermark is our echo; nothing applies and nothing is sent.
    #[tokio::test]
    async fn awtd2_56_an_echo_is_neither_applied_nor_sent_back() {
        let stamp = "2026-09-07T11:00:00.000Z";
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Same", stamp)],
            links: vec![link_row("t1", "r1", stamp, stamp)],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Same", "2026-09-07T11:00:00Z"))
            .expect("writes");

        let report = pass.run().await;

        assert_eq!(report.applied, 0);
        assert!(pass.pushes().is_empty());
    }

    /// AWTD2-56: a twin known only to this device, whose local edit is newer than Google's, is
    /// linked on the server without claiming the two sides agree — so the edit still goes out.
    #[tokio::test]
    async fn awtd2_56_a_twin_known_only_here_still_sends_its_newer_edit() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Google's title", "2026-09-07T11:30:00.000Z")],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "My edit", "2026-09-07T11:45:00Z"))
            .expect("writes");
        ledger::remember_links(
            &pass.store,
            PROVIDER_KEY,
            CONTAINER,
            [("t1".to_string(), "tasklist-1:r1".to_string())],
        )
        .expect("remembers");
        pass.run().await;

        assert_eq!(pass.task("t1").expect("kept").title, "My edit");
        assert_eq!(
            pass.remote("tasklist-1:r1").expect("there")["title"],
            "My edit"
        );
    }

    // ── Duplicates ──────────────────────────────────────────────────────────────────────────

    /// AWTD2-56: a link whose task this machine does not hold (not loaded yet, or deleted here and
    /// not yet on the server) is not an invitation to make a second task.
    #[tokio::test]
    async fn awtd2_56_a_linked_item_whose_task_is_missing_here_is_not_imported_again() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
            links: vec![link_row(
                "t-elsewhere",
                "r1",
                "2026-09-07T11:00:00.000Z",
                "2026-09-07T11:00:00.000Z",
            )],
            ..Default::default()
        });

        pass.run().await;

        assert!(pass.titled("Buy milk").is_empty(), "no second task");
    }

    /// AWTD2-56: the first pass over a list somebody already keeps on both sides adopts the local
    /// task of the same title rather than making a second one.
    #[tokio::test]
    async fn awtd2_56_a_pulled_item_adopts_the_one_local_task_of_its_title() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert_eq!(pass.titled("Buy milk").len(), 1, "adopted, not duplicated");
        assert_eq!(
            pass.server_link("t1").expect("linked")["remoteId"],
            "tasklist-1:r1"
        );
        assert!(
            pass.pushes().is_empty(),
            "and no twin was made for it either"
        );
    }

    /// AWTD2-56: two local tasks of that title is a guess, and a guess is not adopted.
    #[tokio::test]
    async fn awtd2_56_an_ambiguous_title_is_not_adopted() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
            ..Default::default()
        });
        for id in ["t1", "t2"] {
            pass.store
                .upsert_task(&listed(id, "Buy milk", "2026-09-07T10:00:00Z"))
                .expect("writes");
        }

        pass.run().await;

        for id in ["t1", "t2"] {
            assert_ne!(
                pass.server_link(id).map(|link| link["remoteId"].clone()),
                Some(json!("tasklist-1:r1")),
                "{id} was not guessed at"
            );
        }
    }

    /// AWTD2-56: before creating a twin, the push looks through the complete listing for an
    /// unlinked one of the same title — an item outside this pass's cursor window included.
    #[tokio::test]
    async fn awtd2_56_a_push_adopts_an_unlinked_remote_item_of_the_same_title() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Buy milk", "2026-09-01T00:00:00.000Z")],
            window: Some(Vec::new()),
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert!(
            pass.pushes().is_empty(),
            "no twin made: {:?}",
            pass.pushes()
        );
        assert_eq!(
            pass.server_link("t1").expect("linked")["remoteId"],
            "tasklist-1:r1"
        );
    }

    /// AWTD2-56: a deleted Google item is not a twin to adopt — linking to it would have the next
    /// absence pass delete the local task. (Apple adopts it; see docs/CONTRACTS.md D39.)
    #[tokio::test]
    async fn awtd2_56_a_deleted_remote_item_is_not_adopted() {
        let mut deleted = item("r1", "Buy milk", "2026-09-01T00:00:00.000Z");
        deleted["metadata"]["deleted"] = json!("1");
        let pass = Pass::new(Proxy {
            items: vec![deleted],
            window: Some(Vec::new()),
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert_ne!(
            pass.server_link("t1").expect("linked")["remoteId"],
            "tasklist-1:r1"
        );
        assert!(pass.task("t1").is_some(), "and the task is still here");
    }

    /// AITD-463 (D39): an item this device deleted is not a twin to adopt either, though Google
    /// still lists it live — the iOS guard this replaces (`GooglePushTwin.find`) refused it too.
    #[tokio::test]
    async fn aitd463_a_tombstoned_remote_item_is_not_adopted() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Buy milk", "2026-09-01T00:00:00.000Z")],
            window: Some(Vec::new()),
            ..Default::default()
        });
        ledger::record_tombstone(&pass.store, PROVIDER_KEY, "tasklist-1:r1").expect("records");
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert_ne!(
            pass.server_link("t1").expect("linked")["remoteId"],
            "tasklist-1:r1"
        );
        assert!(pass.task("t1").is_some(), "and the task is still here");
    }

    /// AITD-463 (D39): among same-title items the deleted one and the one already linked are
    /// passed over, and the live unlinked one is adopted.
    #[tokio::test]
    async fn aitd463_adoption_skips_deleted_and_linked_items_for_the_live_unlinked_one() {
        let mut deleted = item("r1", "Buy milk", "2026-09-01T00:00:00.000Z");
        deleted["metadata"]["deleted"] = json!("1");
        let stamp = "2026-09-07T11:00:00.000Z";
        let pass = Pass::new(Proxy {
            items: vec![
                deleted,
                item("r2", "Buy milk", stamp),
                item("r3", "Buy milk", "2026-09-01T00:00:00.000Z"),
                item("r4", "Bread", "2026-09-01T00:00:00.000Z"),
            ],
            window: Some(Vec::new()),
            links: vec![link_row("t0", "r2", stamp, stamp)],
            ..Default::default()
        });
        let mut other = listed("t0", "Buy milk", stamp);
        other.updated_at = date::parse(stamp);
        pass.store.upsert_task(&other).expect("writes");
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert_eq!(
            pass.server_link("t1").expect("linked")["remoteId"],
            "tasklist-1:r3"
        );
    }

    /// AITD-463 (D39): My Tasks' push makes the same refusal — the iOS guard covered both sites.
    #[tokio::test]
    async fn aitd463_my_tasks_does_not_adopt_a_deleted_remote_item() {
        let mut deleted = item("r1", "Ring the dentist", "2026-09-01T00:00:00.000Z");
        deleted["metadata"]["deleted"] = json!("1");
        let pass = Pass::new(Proxy {
            items: vec![deleted],
            sync_mode: "all_bidirectional".into(),
            ..Default::default()
        });
        let mut mine = Task::new("t1", "Ring the dentist");
        mine.assignee_id = Some("u1".into());
        mine.updated_at = date::parse("2026-09-07T10:00:00Z");
        pass.store.upsert_task(&mine).expect("writes");

        pass.service.sync_my_tasks(CONTAINER).await.expect("a pass");

        assert_ne!(
            pass.server_link("t1").map(|link| link["remoteId"].clone()),
            Some(json!("tasklist-1:r1"))
        );
        assert!(pass.task("t1").is_some(), "and the task is still here");
    }

    /// AWTD2-56: when the complete listing was cut short, the twin may simply be past its end —
    /// so known twins are patched and none is created.
    #[tokio::test]
    async fn awtd2_56_no_twin_is_created_when_the_listing_was_truncated() {
        let pass = Pass::new(Proxy {
            window: Some(Vec::new()),
            full_truncated: true,
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        let report = pass.run().await;

        assert!(pass.pushes().is_empty(), "{:?}", pass.pushes());
        assert_eq!(report.pushed, 0);
    }

    /// AWTD2-56: nor when the listing could not be had at all — unknown is not empty.
    #[tokio::test]
    async fn awtd2_56_no_twin_is_created_when_the_listing_failed() {
        let pass = Pass::new(Proxy {
            window: Some(Vec::new()),
            full_fails: true,
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert!(pass.pushes().is_empty(), "{:?}", pass.pushes());
    }

    /// AWTD2-56: a subtask is created under its parent's twin, not at the top level, so the round
    /// trip keeps it nested.
    #[tokio::test]
    async fn awtd2_56_a_new_subtask_is_created_under_its_parents_twin() {
        let stamp = "2026-09-07T10:00:00.000Z";
        let pass = Pass::new(Proxy {
            items: vec![item("p", "Parent", stamp)],
            window: Some(Vec::new()),
            links: vec![link_row("parent", "p", stamp, stamp)],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("parent", "Parent", "2026-09-07T10:00:00Z"))
            .expect("writes");
        let mut child = listed("child", "Child", "2026-09-07T10:00:00Z");
        child.parent_task_id = Some("parent".into());
        pass.store.upsert_task(&child).expect("writes");

        pass.run().await;

        let created = pass
            .pushes()
            .into_iter()
            .find(|body| body["title"] == "Child")
            .expect("the child was pushed");
        assert_eq!(created["parentRemoteId"], "tasklist-1:p");
    }

    /// AWTD2-56: a link the server files under another container is that container's pass's to
    /// push. Patching it from here addresses the wrong task list.
    #[tokio::test]
    async fn awtd2_56_a_link_from_another_container_is_not_pushed_here() {
        let pass = Pass::new(Proxy {
            links: vec![json!({
                "astridTaskId": "t1",
                "remoteId": "other-list:r1",
                "remoteContainerId": "other-list",
                "astridUpdatedAt": null,
                "remoteUpdatedAt": null,
            })],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert!(
            !pass
                .pushes()
                .iter()
                .any(|body| body["remoteId"] == "other-list:r1"),
            "{:?}",
            pass.pushes()
        );
    }

    /// AWTD2-56: a failed push is not watermarked, so it is tried again next pass without any
    /// stamp of its own; the rest of the list still goes.
    #[tokio::test]
    async fn awtd2_56_a_failed_push_is_retried_and_does_not_stop_the_list() {
        let pass = Pass::new(Proxy {
            refuse: vec!["First".into()],
            ..Default::default()
        });
        for (id, title) in [("t1", "First"), ("t2", "Second")] {
            pass.store
                .upsert_task(&listed(id, title, "2026-09-07T10:00:00Z"))
                .expect("writes");
        }

        let report = pass.run().await;
        assert_eq!(report.pushed, 1, "the second task still went");

        pass.proxy.state.lock().expect("lock").refuse.clear();
        let report = pass.run().await;
        assert_eq!(report.pushed, 1, "and the first went on the next pass");
        assert!(pass.server_link("t1").is_some());
    }

    // ── What only a complete listing can show ───────────────────────────────────────────────

    /// AWTD2-56: a pair whose completion drifted (a botched pass, an item outside every cursor
    /// window since) is repaired from the complete listing when the local task is untouched.
    #[tokio::test]
    async fn awtd2_56_completion_drift_is_repaired_from_the_complete_listing() {
        let stamp = "2026-09-07T10:00:00.000Z";
        let mut done = item("r1", "Buy milk", stamp);
        done["completed"] = json!(true);
        done["completedAt"] = json!("2026-09-06T08:00:00.000Z");
        let pass = Pass::new(Proxy {
            items: vec![done],
            window: Some(Vec::new()),
            links: vec![link_row("t1", "r1", stamp, stamp)],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        let task = pass.task("t1").expect("kept");
        assert!(task.completed, "the drift was repaired");
        assert_eq!(task.completed_at, date::parse("2026-09-06T08:00:00Z"));
    }

    /// AWTD2-56: an item gone from a complete listing was deleted over there (Google models a move
    /// as delete and insert), so its local twin goes — through the service, and tombstoned.
    #[tokio::test]
    async fn awtd2_56_a_twin_absent_from_a_complete_listing_is_deleted_here() {
        let stamp = "2026-09-07T10:00:00.000Z";
        let pass = Pass::new(Proxy {
            items: vec![item("r2", "Keep", stamp)],
            window: Some(Vec::new()),
            links: vec![
                link_row("t1", "r1", stamp, stamp),
                link_row("t2", "r2", stamp, stamp),
            ],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Gone", "2026-09-07T10:00:00Z"))
            .expect("writes");
        pass.store
            .upsert_task(&listed("t2", "Keep", "2026-09-07T10:00:00Z"))
            .expect("writes");

        let report = pass.run().await;

        assert_eq!(report.deleted_locally, 1);
        assert!(pass.task("t1").is_none());
        assert!(pass.task("t2").is_some());
        assert!(
            ledger::tombstoned(&pass.store, PROVIDER_KEY).contains(&"tasklist-1:r1".to_string())
        );
        assert!(
            crate::outbox::journal::all(&pass.store)
                .expect("reads")
                .iter()
                .any(|entry| entry.kind == crate::outbox::kind::DELETE_TASK),
            "the deletion reaches astrid-web"
        );
    }

    /// AWTD2-56: a twin the push has just made is absent from the listing fetched before it, and
    /// that absence must not read as a deletion.
    #[tokio::test]
    async fn awtd2_56_a_twin_made_this_pass_is_not_deleted_by_its_absence() {
        let pass = Pass::new(Proxy {
            window: Some(Vec::new()),
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Buy milk", "2026-09-07T10:00:00Z"))
            .expect("writes");

        let report = pass.run().await;

        assert_eq!(report.pushed, 1);
        assert_eq!(report.deleted_locally, 0);
        assert!(pass.task("t1").is_some());
    }

    /// AWTD2-56: the complete listing is fetched for deletions at most every five minutes when
    /// nothing else in the pass needs it.
    #[tokio::test]
    async fn awtd2_56_the_complete_listing_is_not_fetched_every_pass() {
        let pass = Pass::new(Proxy {
            window: Some(Vec::new()),
            ..Default::default()
        });

        pass.run().await;
        pass.run().await;

        let full = pass
            .proxy
            .sent
            .lock()
            .expect("lock")
            .iter()
            .filter(|request| request.url.contains("full=1"))
            .count();
        assert_eq!(
            full, 1,
            "the second pass, a moment later, did not fetch it again"
        );
    }

    /// AWTD2-56: a truncated listing proves nothing about absence. Nothing is deleted.
    #[tokio::test]
    async fn awtd2_56_a_truncated_listing_deletes_nothing() {
        let stamp = "2026-09-07T10:00:00.000Z";
        let pass = Pass::new(Proxy {
            window: Some(Vec::new()),
            full_truncated: true,
            links: vec![link_row("t1", "r1", stamp, stamp)],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Still here", "2026-09-07T10:00:00Z"))
            .expect("writes");

        let report = pass.run().await;

        assert_eq!(report.deleted_locally, 0);
        assert!(pass.task("t1").is_some());
    }

    /// AWTD2-56: one item this build cannot read makes the listing incomplete, not empty. Read as
    /// empty, every linked task in the list would be deleted for being absent from it.
    #[tokio::test]
    async fn awtd2_56_an_unreadable_listing_deletes_nothing() {
        let stamp = "2026-09-07T10:00:00.000Z";
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Still here", stamp), json!({ "title": "no id" })],
            window: Some(Vec::new()),
            links: vec![link_row("t1", "r1", stamp, stamp)],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Still here", "2026-09-07T10:00:00Z"))
            .expect("writes");
        pass.store
            .upsert_task(&listed("t2", "Unlinked", "2026-09-07T10:00:00Z"))
            .expect("writes");

        let report = pass.run().await;

        assert_eq!(report.deleted_locally, 0);
        assert!(pass.task("t1").is_some());
        assert!(
            !pass.pushes().iter().any(|body| body["title"] == "Unlinked"),
            "and an incomplete listing creates no twin either"
        );
    }

    /// AWTD2-56: a failed listing proves nothing either.
    #[tokio::test]
    async fn awtd2_56_a_failed_listing_deletes_nothing() {
        let stamp = "2026-09-07T10:00:00.000Z";
        let pass = Pass::new(Proxy {
            window: Some(Vec::new()),
            full_fails: true,
            links: vec![link_row("t1", "r1", stamp, stamp)],
            ..Default::default()
        });
        pass.store
            .upsert_task(&listed("t1", "Still here", "2026-09-07T10:00:00Z"))
            .expect("writes");

        pass.run().await;

        assert!(pass.task("t1").is_some());
    }

    /// AWTD2-56: completed history comes in as completed tasks — with Google's completion time —
    /// never as open ones, and once.
    #[tokio::test]
    async fn awtd2_56_completed_history_is_backfilled_as_completed_and_once() {
        let mut done = item("r1", "Filed taxes", "2026-09-01T00:00:00.000Z");
        done["completed"] = json!(true);
        done["completedAt"] = json!("2026-08-31T09:00:00.000Z");
        let pass = Pass::new(Proxy {
            items: vec![done],
            ..Default::default()
        });

        let report = pass.run().await;
        pass.run().await;

        let imported = pass.titled("Filed taxes");
        assert_eq!(imported.len(), 1, "once, not once a pass");
        assert!(imported[0].completed, "completed, not open");
        assert_eq!(
            imported[0].completed_at,
            date::parse("2026-08-31T09:00:00Z")
        );
        assert_eq!(report.backfilled, 1);
    }

    // ── The cursor ──────────────────────────────────────────────────────────────────────────

    /// AWTD2-56: a pulled item that could not be linked on the server (its task has not reached
    /// astrid-web yet) keeps the cursor where it was, so the window is pulled again and the link
    /// written then — instead of the item never being seen again.
    #[tokio::test]
    async fn awtd2_56_the_cursor_waits_for_every_pulled_item_to_be_linked() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Buy milk", "2026-09-07T11:30:00.000Z")],
            ..Default::default()
        });

        pass.run().await;

        assert!(
            !pass
                .sent(Method::Post, "google/tasks")
                .iter()
                .any(|body| body["action"] == "commitCursor"),
            "not committed while the task's create is still in the journal"
        );
    }

    // ── My Tasks runs the same pass ─────────────────────────────────────────────────────────

    /// AWTD2-56: My Tasks adopts by title too, among the unlisted tasks assigned to you.
    #[tokio::test]
    async fn awtd2_56_my_tasks_adopts_the_one_local_task_of_its_title() {
        let pass = Pass::new(Proxy {
            items: vec![item("r1", "Ring the dentist", "2026-09-07T11:30:00.000Z")],
            sync_mode: "all_bidirectional".into(),
            ..Default::default()
        });
        let mut mine = Task::new("t1", "Ring the dentist");
        mine.assignee_id = Some("u1".into());
        mine.updated_at = date::parse("2026-09-07T10:00:00Z");
        pass.store.upsert_task(&mine).expect("writes");

        pass.service.sync_my_tasks(CONTAINER).await.expect("a pass");

        assert_eq!(pass.titled("Ring the dentist").len(), 1);
        assert!(pass.server_link("t1").is_some());
        assert!(pass.pushes().is_empty(), "{:?}", pass.pushes());
    }

    // ── Linking by hand ─────────────────────────────────────────────────────────────────────

    /// AWTD2-56: linking a list by hand takes back an earlier "no" — here and on the account — or
    /// the all-lists modes go on refusing a list somebody has since chosen.
    #[tokio::test]
    async fn awtd2_56_linking_by_hand_clears_the_exclusion() {
        let pass = Pass::new(Proxy {
            excluded: "c1,c2".into(),
            ..Default::default()
        });
        ledger::exclude(&pass.store, PROVIDER_KEY, "c1").expect("excludes");

        pass.service
            .link(Provider::GoogleTasks, "l1", "c1")
            .await
            .expect("links");

        assert!(!ledger::excluded(&pass.store, PROVIDER_KEY).contains(&"c1".to_string()));
        let patched = pass.sent(Method::Patch, "/api/v1/integrations");
        assert_eq!(
            patched.last().expect("the account was told")["metadata"]["excludedTasklists"],
            "c2"
        );
    }
}
