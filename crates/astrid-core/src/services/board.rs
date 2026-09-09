//! Changing a board's columns (task e5214fba).
//!
//! A board's columns are `Project.customStates`, and the web changes them through four writes —
//! add, rename, reorder, remove — whose rules (`lib/project-custom-states.ts`) are ported into
//! [`crate::board`] and locked by `contracts/fixtures/statuses.json`. This service is where those
//! rules meet the server:
//!
//! 1. The rule runs here first, against the board as the cache has it. A name the web would refuse
//!    is refused before any round trip, with the web's own message.
//! 2. The write goes to the server. Not through the Outbox: a column is shared board configuration
//!    rather than a personal edit, the server derives the role and clears the tasks a removed
//!    column held, and an optimistic column that later failed would have shown a state tasks were
//!    being dragged into. So, like membership, it needs a connection and says so.
//! 3. The cached project takes the rule's result at once — the server computes the same thing,
//!    by contract — and is then re-fetched, so the board redraws without waiting for the next
//!    sync.
//!
//! **The server's half is not there yet.** The web manages statuses at `/api/statuses`, which is
//! not under `/api/v1` and which this client's request guard refuses. The writes here go to the
//! versioned route specified in web task 58994c6c — `/api/v1/projects/{id}/statuses`, the same
//! four verbs, the bodies minus the project id — and until that route is deployed they answer
//! 404, which [`missing_route`] turns into a sentence rather than a code. Nothing here changes
//! when it lands.

use serde_json::json;

use super::{Context, Result, ServiceError};
use crate::api::endpoints;
use crate::board::{self, CustomState, ReorderDirection, StateError, StateWrite};
use crate::model::Project;

/// A 404 from the statuses route is the server not having it yet, and should read that way
/// rather than as "not found" beside a column somebody can see.
fn missing_route(error: crate::api::ApiError) -> ServiceError {
    match error {
        crate::api::ApiError::Http { status: 404, .. } => {
            ServiceError::Api(crate::api::ApiError::Refused(
                "This server does not manage board columns yet (astrid-web task 58994c6c)."
                    .to_string(),
            ))
        }
        other => ServiceError::Api(other),
    }
}

/// What a status write came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusOutcome {
    /// Written, on the server and in the cache.
    Written(CustomState),
    /// Refused by the rule, before any round trip, with the web's message.
    Refused(StateError),
}

pub struct BoardService {
    context: Context,
}

impl BoardService {
    pub fn new(context: Context) -> Self {
        BoardService { context }
    }

    /// The board a list belongs to, as the cache has it.
    fn project_for_list(&self, list_id: &str) -> Result<Project> {
        let list = self
            .context
            .store
            .list(list_id)?
            .ok_or_else(|| ServiceError::NotFound {
                kind: "list",
                id: list_id.to_string(),
            })?;
        let project_id = list.project_id.ok_or_else(|| ServiceError::NotFound {
            kind: "board",
            id: list_id.to_string(),
        })?;
        self.context
            .store
            .projects()?
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or(ServiceError::NotFound {
                kind: "board",
                id: project_id,
            })
    }

    pub async fn add_status(&self, list_id: &str, name: &str) -> Result<StatusOutcome> {
        let project = self.project_for_list(list_id)?;
        let write = match board::add_custom_state(project.custom_states.as_ref(), name) {
            Ok(write) => write,
            Err(refused) => return Ok(StatusOutcome::Refused(refused)),
        };
        let request = self
            .context
            .client
            .post(endpoints::project_statuses(&project.id))
            .value(json!({ "name": name.trim() }));
        self.context
            .client
            .send(request)
            .await
            .map_err(missing_route)?;
        self.take(project, write).await
    }

    pub async fn rename_status(
        &self,
        list_id: &str,
        role: &str,
        name: &str,
    ) -> Result<StatusOutcome> {
        let project = self.project_for_list(list_id)?;
        let write = match board::rename_state(project.custom_states.as_ref(), role, name) {
            Ok(write) => write,
            Err(refused) => return Ok(StatusOutcome::Refused(refused)),
        };
        let request = self
            .context
            .client
            .patch(endpoints::project_statuses(&project.id))
            .value(json!({ "role": role, "name": name.trim() }));
        self.context
            .client
            .send(request)
            .await
            .map_err(missing_route)?;
        self.take(project, write).await
    }

    pub async fn reorder_status(
        &self,
        list_id: &str,
        role: &str,
        direction: ReorderDirection,
    ) -> Result<StatusOutcome> {
        let project = self.project_for_list(list_id)?;
        let write =
            match board::reorder_custom_state(project.custom_states.as_ref(), role, direction) {
                Ok(write) => write,
                Err(refused) => return Ok(StatusOutcome::Refused(refused)),
            };
        let request = self
            .context
            .client
            .put(endpoints::project_statuses(&project.id))
            .value(json!({ "role": role, "direction": direction }));
        self.context
            .client
            .send(request)
            .await
            .map_err(missing_route)?;
        self.take(project, write).await
    }

    pub async fn remove_status(&self, list_id: &str, role: &str) -> Result<StatusOutcome> {
        let project = self.project_for_list(list_id)?;
        let write = match board::remove_custom_state(project.custom_states.as_ref(), role) {
            Ok(write) => write,
            Err(refused) => return Ok(StatusOutcome::Refused(refused)),
        };
        let request = self
            .context
            .client
            .delete(endpoints::project_statuses(&project.id))
            .value(json!({ "role": role }));
        self.context
            .client
            .send(request)
            .await
            .map_err(missing_route)?;
        // The server clears `statusRole` on the tasks that carried the removed column. The cache
        // learns that on the next sync; until then those tasks draw in Inbox, which is where the
        // board puts a role no column has.
        self.take(project, write).await
    }

    /// The written state into the cache, then the server's own copy of the board.
    async fn take(&self, mut project: Project, write: StateWrite) -> Result<StatusOutcome> {
        project.custom_states = Some(board::states_to_json(&write.states));
        self.context.store.upsert_projects(&[project])?;
        // Best effort: the cache already says what the server will say, so a refresh that could
        // not happen changes nothing.
        let _ = self.refresh_projects().await;
        Ok(StatusOutcome::Written(write.state))
    }

    /// Fetch every board the account has and cache it.
    pub async fn refresh_projects(&self) -> Result<usize> {
        let projects = self
            .context
            .client
            .send_collection::<Project>(
                self.context.client.get(endpoints::PROJECTS),
                Some(endpoints::envelope::PROJECTS),
            )
            .await?;
        let items = projects.into_items();
        self.context.store.upsert_projects(&items)?;
        Ok(items.len())
    }
}
