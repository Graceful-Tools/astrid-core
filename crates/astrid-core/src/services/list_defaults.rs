//! What a new task looks like in a list that has said so — moved to [`astrid_rules`].
//!
//! The rule is pure and a client needs it without a cache or a clock, so it now lives in
//! `astrid-rules` beside the rows it reads (`crate::rows::due_picks`) and is answered through
//! `Rule::QuickAddDefaults` as well as through `createTask` (task `75a27f7f`).
//!
//! Kept at this path because `services::list_defaults::apply` is how every write door here names
//! it — the same re-export `lib.rs` does for every other rules module.

pub use astrid_rules::list_defaults::{apply, Given};
