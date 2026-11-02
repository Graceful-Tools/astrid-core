//! Linking a list to a repository.
//!
//! **GitHub is the server's job.** A cron on astrid-web pulls issues and pushes tasks, so a client
//! only configures it: connect the account, link a list to a repository. A list linked from here
//! syncs whether or not this app is running. So there is no pass in this file and there is not
//! meant to be one — compare [`super::google`], which is the whole client side of Google Tasks.
//!
//! What is left is the names: the ones the API knows this provider by, and where it lists the
//! containers. [`super::common`] holds the methods that read them.

use crate::api::endpoints;

/// The name the API knows it by.
///
/// `GITHUB_ISSUES` reads oddly beside [`super::Provider::GitHub`] but is the server's own name for
/// it — the one `/api/v1/integrations` validates against and the one the Apple clients send.
/// Anything else is a 400 on disconnect and a provider that never shows as connected.
pub(super) const WIRE: &str = "GITHUB_ISSUES";

/// The name this crate's own records key it by.
pub(super) const SLUG: &str = "github";

/// Where the repositories this account can link are listed.
pub(super) const CONTAINERS_PATH: &str = endpoints::GITHUB_SYNC_REPOS;

/// And the key they arrive under.
pub(super) const CONTAINERS_KEY: &str = "repos";
