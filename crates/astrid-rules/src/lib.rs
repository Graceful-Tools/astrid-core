//! Astrid's pure rules — the half of the shared core that waits on nothing.
//!
//! Models in their wire shape, and every rule that must read identically on web, Apple and
//! Windows: repeating rollover, permissions, filters, search and smart parsing, the keyboard
//! scheme, board columns, the editing session, markdown, and the rows a screen draws from. Plus
//! [`rules::run_json`], the synchronous JSON-in/JSON-out door to them.
//!
//! **No I/O, by construction.** Nothing here reads a clock, a disk, the network or the OS's
//! entropy: "now", the person's zone and anything else a rule depends on arrives as an argument.
//! That is what lets this crate build for `wasm32-unknown-unknown`, so the web can run the same
//! rules the apps do (CI builds it for that target). The cache, the Outbox, the API client and
//! sync live in `astrid-core`, which depends on this crate and re-exports every module here at
//! its old path (`astrid_core::repeating`, `astrid_core::rules`, …).

pub mod board;
pub mod board_cards;
pub mod editing;
pub mod envelope;
pub mod filters;
pub mod identifier;
pub mod keyboard;
pub mod manual_order;
pub mod markdown;
pub mod model;
pub mod palette;
pub mod parse;
pub mod permissions;
pub mod reminders;
pub mod repeating;
pub mod rows;
pub mod rules;
pub mod smart_tasks;
pub mod theme;
