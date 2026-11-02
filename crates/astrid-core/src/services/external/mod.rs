//! Connecting other people's task systems, and mirroring Google Tasks.
//!
//! Ports the client half of `astrid-ios/Astrid App/Core/Sync/`. The decisions it makes live in
//! [`crate::external`] with their reasons and their tests; this is the pass that uses them.
//!
//! ## Two providers, two shapes — and that is why the files are these sizes
//!
//! **GitHub is the server's job.** A cron on astrid-web pulls issues and pushes tasks, so a client
//! only configures it: connect the account, link a list to a repository. A list linked from here
//! syncs whether or not this app is running. `github.rs` is therefore a handful of names.
//!
//! **Google Tasks is the client's job.** The server holds the tokens and proxies the API, but the
//! pulling and pushing are the client's — "clients poll on foreground/nudge", as the route says.
//! `google.rs` is the pass that does it, and nearly all of this module.
//!
//! `common.rs` is what neither owns: the [`Provider`] enum, an [`ExternalLink`], a [`Container`],
//! and connecting or linking either provider. This file holds only [`ExternalSyncService`] itself
//! and re-exports, so `services::external::ExternalLink` stays the path it has always been.
//!
//! ## Deletions go through a ledger, and the pass is what feeds it
//!
//! The server's link row cascades away with the task, so a deletion has to be captured *at delete
//! time* — and deleting is local, offline and synchronous, with no server to ask. So every pass
//! writes down the links it fetched ([`ledger::remember_links`](crate::external::ledger::remember_links)),
//! a deletion reads them, and the next pass removes the twin and refuses to import the id again.
//! See [`crate::external::ledger`].
//!
//! A remote deletion is acted on when Google says so explicitly (`deleted`), or when a twin is
//! absent from a **complete** listing — never from a cursor page, a truncated listing or a failed
//! one. Deleting local tasks because a page did not mention them is how a dropped request wipes
//! somebody's list; see [`crate::external::decisions::local_deletions`].
//!
//! ## Parity with Apple (AWTD2-56)
//!
//! The pass is `GoogleTasksSyncService.sync(link:)` and `syncMyTasks` from astrid-ios, in the same
//! order and with the same rules: the two watermarks on each server link and last-write-wins,
//! same-title adoption, no twin created without a complete listing, parents before children,
//! completion drift repair, deletion by absence, the completed backfill, and a cursor committed
//! only when every pulled item was dealt with.

mod common;
mod github;
mod google;

use super::Context;

pub use common::{Container, ExternalLink, Provider};
pub use google::{AutoLinkReport, AutoLinkSettings, PassReport};

pub struct ExternalSyncService {
    context: Context,
}

impl ExternalSyncService {
    pub fn new(context: Context) -> Self {
        ExternalSyncService { context }
    }
}
