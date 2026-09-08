//! Mirroring lists into and out of other people's task systems.
//!
//! Google Tasks and GitHub, which the Apple clients call "external sync". Two providers with two
//! quite different shapes, and knowing which is which is most of understanding this module:
//!
//! - **GitHub is the server's job.** A cron on astrid-web pulls issues and pushes tasks, so a
//!   client only has to *configure* it: connect the account, link a list to a repository. Nothing
//!   here mirrors anything, and a list linked from this app syncs whether or not the app is open.
//! - **Google Tasks is the client's job.** The server holds the tokens and proxies the API, but
//!   the pulling, pushing and reconciling are the client's — "clients poll on foreground/nudge",
//!   as the route says.
//!
//! What lives here is the half that decides. The passes themselves are plumbing; these are the
//! rules that cost somebody their data when they are wrong, and every one of them was extracted on
//! Apple after something went wrong. They are ported with their reasons attached.

pub mod auto_link;
pub mod decisions;
