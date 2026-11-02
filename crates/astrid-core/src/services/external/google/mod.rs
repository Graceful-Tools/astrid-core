//! Mirroring Google Tasks: the pass, and the auto-linking that decides what it runs over.
//!
//! **Google Tasks is the client's job.** The server holds the tokens and proxies the API, but the
//! pulling and pushing are the client's — "clients poll on foreground/nudge", as the route says.
//! So nearly all of [`super`] is this file, while [`super::github`] is a handful of names.
//!
//! Ports `GoogleTasksSyncService.sync(link:)` and `syncMyTasks` from astrid-ios. The decisions the
//! pass makes live in [`crate::external`] with their reasons and their tests; this is the pass that
//! uses them. [`super`] has the module's overview, the ledger's part in it, and the parity notes.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ExternalLink, ExternalSyncService, Provider};
use crate::api::endpoints;
use crate::external::auto_link::{self, SyncMode};
use crate::external::decisions::{self, PullOutcome};
use crate::external::ledger;
use crate::model::{date, Task};
use crate::services::Result;

/// The name the API knows it by.
pub(super) const WIRE: &str = "GOOGLE_TASKS";

/// The name this crate's own records key it by — the API's slug, and the ledger's own key.
pub(super) const SLUG: &str = "google";

/// Where the task lists this account can link are listed.
pub(super) const CONTAINERS_PATH: &str = endpoints::GOOGLE_TASKLISTS;

/// And the key they arrive under.
pub(super) const CONTAINERS_KEY: &str = "tasklists";

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

/// The name the ledger files Google's deletions under — [`SLUG`], read as a ledger key.
const PROVIDER_KEY: &str = SLUG;

/// And the one it files list-to-container links under, so deleting a list can be remembered the
/// same way deleting a task is. Separate from the tasks' own store: the ids mean different things.
pub(super) const LIST_PROVIDER_KEY: &str = "google.lists";

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

impl ExternalSyncService {
    /// Take the tombstones out of Google's integration metadata.
    ///
    /// They arrive as one comma-separated string, which is how the server stores its metadata, and
    /// they go into their own store — never this device's — so a large merge cannot evict a local
    /// deletion. See [`crate::external::ledger`].
    pub(super) fn merge_server_tombstones(&self, answer: &serde_json::Value) -> Result<()> {
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

    /// Best effort: the link is made either way, and a setting that could not be written is put
    /// right by the next manual link.
    pub(super) async fn clear_exclusion(&self, container_id: &str) {
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

    /// "New List": make the list on Google and bind it to an Astrid list, in one call (task
    /// 6db679c3).
    ///
    /// Two writes, so the failure that matters is *made on Google, not bound here* — and a blind
    /// retry would leave a second list of the same name on somebody's account every time they
    /// pressed the button. So an unlinked container already called this is adopted instead, which
    /// is exactly what [`auto_link::astrid_to_google`] does for the all-lists modes; a container
    /// somebody else's list is already bound to is not an orphan and is left alone. A retry after
    /// a failed bind therefore finishes the first attempt rather than duplicating it.
    pub async fn create_and_link_google_list(
        &self,
        list_id: &str,
        name: &str,
    ) -> Result<serde_json::Value> {
        let (containers, _) = self.containers(Provider::GoogleTasks).await?;
        let links = self.links(Provider::GoogleTasks).await?;
        let orphan = containers.into_iter().find(|container| {
            container.name == name
                && !links
                    .iter()
                    .any(|link| link.remote_container_id == container.id)
        });
        let (container_id, created) = match orphan {
            Some(container) => (container.id, false),
            None => (self.create_container(name).await?, true),
        };
        let answer = self
            .link(Provider::GoogleTasks, list_id, &container_id)
            .await?;
        Ok(json!({
            "containerId": container_id,
            "containerName": name,
            "created": created,
            "link": answer.get("link").cloned().unwrap_or(serde_json::Value::Null),
        }))
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
            scope
                .candidates(store)?
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
        let mut held: Vec<Task> = scope
            .candidates(&self.context.store)?
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
        // `description_text`, not the field: the bridge has always sent `""` for a task with no
        // description, and `json!(None)` would send `null` instead — a different value to Google.
        body.insert("notes".into(), json!(task.description_text()));
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
            scope
                .candidates(store)?
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
        if task.description_text() != notes {
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

    /// The local tasks this pass could be mirroring — read bounded where the placement allows.
    ///
    /// [`Scope::holds`] still decides; this only narrows what has to be decoded to ask it, which
    /// was the whole account on every pass, per linked list, every five minutes (task `71ddacb8`).
    ///
    /// **In a list, membership answers it exactly**: `holds` wants `list_ids` to contain the list,
    /// and membership is written from `effective_list_ids`, which *is* `list_ids` whenever that is
    /// set. **My Tasks still reads the account**, and deliberately: it wants the tasks with no list
    /// at all, and a task carrying `lists` but no `list_ids` has membership rows while
    /// [`is_my_task`] still counts it unlisted — so a query on "has no membership" would quietly
    /// stop mirroring a task this pass owns. Bounding that one needs the two notions of "in a
    /// list" reconciled first, which is more than a read change.
    fn candidates(&self, store: &crate::store::Store) -> crate::store::Result<Vec<Task>> {
        match &self.placement {
            Placement::InList(list_id) => store.tasks_in_list(list_id),
            Placement::MyTasks(_) => store.tasks(),
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
        && remote.notes.as_deref().unwrap_or_default() == task.description_text()
        && remote.completed == task.completed
        && remote.due_date.as_deref().and_then(date::parse) == due.and_then(date::parse)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod awtd2_56;
