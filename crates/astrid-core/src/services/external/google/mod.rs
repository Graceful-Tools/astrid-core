//! Google Tasks: the names every build knows, and — behind `sync-google` — the pass that uses them.
//!
//! **Google Tasks is the client's job.** The server holds the tokens and proxies the API, but the
//! pulling and pushing are the client's — "clients poll on foreground/nudge", as the route says.
//! So [`pass`] is nearly all of [`super`], while [`super::github`] is a handful of names: a cron on
//! astrid-web does GitHub, and there is no client pass to write or to compile out.
//!
//! ## Why the names are out here and the pass is in there (task `b92569a8`)
//!
//! A partner build turns Google off in configuration, and until this split it still compiled the
//! whole pass in. `sync-google` is how it stops — on by default, so every consumer that says
//! nothing keeps mirroring.
//!
//! What it must *not* take with it is [`Provider`](super::Provider). That enum is a model type: the
//! server answers `/api/v1/integrations` with whatever the account has connected regardless of what
//! this binary can mirror, and an enum that loses a variant per feature combination is an enum
//! every shell's bindings gate has to read a matrix of. So the four names below stay compiled in
//! every build, [`super::common`] goes on reading them as the table it describes itself as, and
//! connect / disconnect / link / unlink keep working. Only the pass is gated.

use crate::api::endpoints;

#[cfg(feature = "sync-google")]
mod pass;

#[cfg(feature = "sync-google")]
pub use pass::{AutoLinkReport, AutoLinkSettings, PassReport};

/// The name the API knows it by.
pub(super) const WIRE: &str = "GOOGLE_TASKS";

/// The name this crate's own records key it by — the API's slug, and the ledger's own key.
pub(super) const SLUG: &str = "google";

/// Where the task lists this account can link are listed.
pub(super) const CONTAINERS_PATH: &str = endpoints::GOOGLE_TASKLISTS;

/// And the key they arrive under.
pub(super) const CONTAINERS_KEY: &str = "tasklists";

/// The name the ledger files list-to-container links under, so deleting a list can be remembered
/// the same way deleting a task is. Separate from the tasks' own store: the ids mean different
/// things.
///
/// Out here with the names rather than in [`pass`] because [`super::common`]'s `links()` writes it
/// on every listing, and that runs in a build with no pass — the links are still the server's, and
/// a device that may not mirror them can still be asked to show them.
pub(super) const LIST_PROVIDER_KEY: &str = "google.lists";
