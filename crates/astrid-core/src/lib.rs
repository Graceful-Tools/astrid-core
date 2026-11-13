//! Astrid client core — the layer every shell renders and nothing more.
//!
//! This crate is the cross-platform successor to the Swift service layer in
//! `astrid-ios/Astrid App/Core/`. No shell adds business logic — not the WinUI one, not
//! the Apple ones, not Android's. Anything that decides something about tasks, lists,
//! sync, permissions or user-facing copy belongs here.
//!
//! The rules that govern this crate are numbered in README.md. The short version:
//!
//! 1. Backend writes go through a service, never the HTTP client directly.
//! 2. A task is completed ONLY through `TaskService::complete` (or `complete_as`) — the
//!    path that rolls a repeating task over to its next occurrence.
//! 3. Next-occurrence math lives only in [`repeating`], mirrored from
//!    `astrid-web/types/repeating.ts`.
//! 4. Writes journal through the Outbox: idempotent, retrying, dependency-ordered.
//! 5. No platform dependency here — platform services arrive as callback traits.

pub mod api;
pub mod app;
pub mod auth;
pub mod external;
pub mod outbox;
pub mod paste;
pub mod platform;
pub mod realtime;
pub mod services;
pub mod store;
pub mod sync;

// The pure rules live in `astrid-rules`, which builds for WebAssembly too. They are re-exported
// here at the paths they have always had, so `astrid_core::repeating`, `astrid_core::rules` and
// the rest still name the same items for every client.
pub use astrid_rules::{
    board, board_cards, decisions, editing, filters, identifier, keyboard, manual_order, markdown,
    model, palette, parse, permissions, reminders, repeating, rows, rules, smart_tasks, theme,
    title_display,
};

/// The revision of this core that a client was built against.
///
/// A crash report needs to name the code that crashed, and the version a shell knows is its
/// own: Windows' `astrid_version()` answered astrid-ffi's `0.1.0` while the core it was running
/// came from a pin somebody bumped a fortnight ago (astrid-windows task `ab249b76`). The core is
/// its own repository and the clients take it by revision, so the revision is the only string
/// that identifies what is actually running.
///
/// Resolved by `build.rs`, in order: an `ASTRID_BUILD_REV` in the environment (a release build
/// hands it in), the short `git rev-parse` of this checkout, then `"unknown"`. Never empty — a
/// build with no git and no override still produces a string a report can print.
pub const BUILD_REV: &str = env!("ASTRID_BUILD_REV");

#[cfg(test)]
mod build_rev_tests {
    /// Whatever the build environment was, the constant is a printable identifier.
    ///
    /// A cargo build from a git dependency's checkout, a tarball with no `.git`, and CI with the
    /// revision handed in all have to produce something a crash report can carry — an empty
    /// string in a version field reads as a broken client, not as a missing git.
    #[test]
    fn the_build_revision_is_always_a_printable_string() {
        assert!(!super::BUILD_REV.is_empty());
        assert!(!super::BUILD_REV.contains(char::is_whitespace));
    }
}
