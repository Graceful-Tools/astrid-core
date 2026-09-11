//! Reading what somebody typed.
//!
//! The quick-add control's `#list` autocomplete lives here, ported from
//! `astrid-web/lib/quick-add.ts`. Everything in this module is pure and returns keys rather than
//! sentences, so the shell resolves its own words and the rules stay testable without a window.

pub mod mentions;
pub mod quick_add;
pub mod search;
