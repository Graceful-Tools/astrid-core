//! Astrid client core — the layer the Windows shell renders and nothing more.
//!
//! This crate is the Windows counterpart of the Swift service layer in
//! `astrid-ios/Astrid App/Core/`, which the Mac app shares verbatim. The Mac app adds
//! no business logic; neither may the WinUI shell. Anything that decides something about
//! tasks, lists, sync, permissions or user-facing copy belongs here.
//!
//! Rules that govern this crate live in `docs/ASTRID.md`. The short version:
//!
//! 1. Backend writes go through a service, never the HTTP client directly.
//! 2. A task is completed ONLY through `TaskService::complete_task` — the path that rolls
//!    a repeating task over to its next occurrence.
//! 3. Next-occurrence math lives only in [`repeating`], mirrored from
//!    `astrid-web/types/repeating.ts`.
//! 4. Writes journal through the Outbox: idempotent, retrying, dependency-ordered.
//! 5. No Windows dependency here — platform services arrive as callback traits.

pub mod api;
pub mod auth;
pub mod keyboard;
pub mod model;
pub mod permissions;
pub mod platform;
pub mod repeating;
pub mod store;
