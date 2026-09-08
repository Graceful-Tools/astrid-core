//! The control points. Everything the shell asks for goes through one of these.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/`. Rule 1 of `docs/ASTRID.md` §0 — all backend
//! writes go through a service — is not about layering for its own sake. A service is where the
//! three things that must happen together happen together:
//!
//! 1. the cache is updated optimistically, so the UI answers immediately;
//! 2. the write is journalled to the Outbox, so it survives being offline, being killed, and being
//!    retried;
//! 3. the rules that go with the write are applied — repeat rollover, permission checks, list
//!    membership.
//!
//! Anything that calls the API client directly gets the first without the second, or the second
//! without the third. Every "it looked like it saved" bug in the sibling repos is one of those two
//! shapes.

pub mod account;
pub mod attachment;
pub mod auth;
pub mod chat;
pub mod comment;
pub mod list;
pub mod search;
pub mod task;
pub mod timer;

pub use account::AccountService;
pub use attachment::AttachmentService;
pub use auth::AuthService;
pub use chat::ChatService;
pub use comment::CommentService;
pub use list::{ListChanges, ListService};
pub use task::{TaskChanges, TaskDraft, TaskService};

use std::sync::Arc;

use crate::api::ApiClient;
use crate::platform::Clock;
use crate::store::Store;

/// What every service needs, held once.
///
/// Cheap to clone (three `Arc`s) so services can be constructed per call site rather than passed
/// around as a bundle — which keeps a service from quietly acquiring state of its own.
#[derive(Clone)]
pub struct Context {
    pub client: Arc<ApiClient>,
    pub store: Arc<Store>,
    pub clock: Arc<dyn Clock>,
}

impl Context {
    pub fn new(client: Arc<ApiClient>, store: Arc<Store>, clock: Arc<dyn Clock>) -> Self {
        Context {
            client,
            store,
            clock,
        }
    }

    pub fn tasks(&self) -> TaskService {
        TaskService::new(self.clone())
    }

    pub fn lists(&self) -> ListService {
        ListService::new(self.clone())
    }

    pub fn comments(&self) -> CommentService {
        CommentService::new(self.clone())
    }

    /// Files on tasks. Needs the cache directory, which is the only service that does — it is
    /// the one that puts something on disk beside the database.
    pub fn attachments(&self, cache_dir: impl AsRef<std::path::Path>) -> AttachmentService {
        AttachmentService::new(self.clone(), cache_dir)
    }

    pub fn chat(&self) -> ChatService {
        ChatService::new(self.clone())
    }

    pub fn account(&self) -> AccountService {
        AccountService::new(self.clone())
    }

    /// Sign-in. Unlike the others this one holds state — the flow between opening the browser and
    /// the callback arriving — so the app keeps ONE of these rather than making one per call.
    pub fn auth(&self) -> AuthService {
        AuthService::new(self.clone())
    }
}

/// What a service can fail with.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Api(#[from] crate::api::ApiError),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
    /// The thing being acted on is not in the cache and could not be fetched.
    #[error("no {kind} with id {id}")]
    NotFound { kind: &'static str, id: String },
    /// A file on this machine could not be read or written. Distinct from a store failure: the
    /// database is fine, and what failed is somebody's disk, their permissions, or a path.
    #[error("could not use a local file: {0}")]
    LocalFile(String),
}

pub type Result<T> = std::result::Result<T, ServiceError>;
