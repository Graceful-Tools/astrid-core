//! How much of a task's title a row shows.
//!
//! Design register row **DS19** (`design/README.md`), decided by Jon on 2026-11-10: a row title
//! *wraps without limit by default*, and an appearance setting on every client — "Task titles" —
//! chooses wrap, two lines or one line.
//!
//! ## The choice belongs to the machine
//!
//! Like [`crate::theme`], and for the same reason: it answers "how much of this list do I want on
//! this screen", and a 13-inch laptop and a 32-inch desktop are different questions. So it is kept
//! in the cache (see [`KEY`]) rather than on the account, and it never reaches the Outbox.
//!
//! ## Why wrap is the default, when no client did it
//!
//! Every client truncated: the Mac and Windows stopped at two lines, with no way to see the rest.
//! A title is the task — a row that ends in an ellipsis is a row you have to open to read, and
//! most titles that overflow do so by a word or two. Truncation is now the thing you ask for.
//!
//! ## What this module does not decide
//!
//! How a shell draws it. WinUI counts lines on a `TextBlock`, SwiftUI takes a `lineLimit`, the web
//! sets `-webkit-line-clamp`; a core that returned a CSS value would be fighting three platforms
//! at once. [`TitleDisplay::max_lines`] gives the one number they all need, and `None` means "do
//! not limit it" rather than a sentinel each client would have to know.

use serde::{Deserialize, Serialize};

/// Where the chosen setting is kept. The cache, so it belongs to this installation.
pub const KEY: &str = "rows.titleDisplay";

/// How much of a row's title is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TitleDisplay {
    /// As many lines as the title needs. The default (DS19).
    #[default]
    Wrap,
    /// Two lines, then an ellipsis. What the Mac and Windows did before there was a choice.
    TwoLines,
    /// One line, then an ellipsis. The densest list.
    OneLine,
}

impl TitleDisplay {
    /// The name this travels under, and the one each client stores.
    pub fn wire(self) -> &'static str {
        match self {
            TitleDisplay::Wrap => "wrap",
            TitleDisplay::TwoLines => "twoLines",
            TitleDisplay::OneLine => "oneLine",
        }
    }

    /// Read a stored name.
    ///
    /// Anything unrecognised is the default rather than an error, exactly as
    /// [`crate::theme::Theme::parse`] does: a value written by a newer build — or by a client that
    /// grows a fourth option — must not leave somebody with a list that will not draw.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "twolines" => TitleDisplay::TwoLines,
            "oneline" => TitleDisplay::OneLine,
            _ => TitleDisplay::Wrap,
        }
    }

    /// Every option, in the order a picker should offer them.
    ///
    /// The default first, then the two truncations from loosest to tightest — so the list reads as
    /// one dial being turned down rather than three unrelated choices.
    pub fn all() -> [TitleDisplay; 3] {
        [
            TitleDisplay::Wrap,
            TitleDisplay::TwoLines,
            TitleDisplay::OneLine,
        ]
    }

    /// How many lines a row may use, or `None` for as many as the title needs.
    ///
    /// The one number every shell wants, so that none of them has to match on the names.
    pub fn max_lines(self) -> Option<u32> {
        match self {
            TitleDisplay::Wrap => None,
            TitleDisplay::TwoLines => Some(2),
            TitleDisplay::OneLine => Some(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DS19's default, and the part that changes behaviour: every client truncated before this.
    /// Somebody who has never opened settings sees the whole title.
    #[test]
    fn the_default_wraps_without_limit() {
        assert_eq!(TitleDisplay::default(), TitleDisplay::Wrap);
        assert_eq!(TitleDisplay::parse(""), TitleDisplay::Wrap);
        assert_eq!(TitleDisplay::all()[0], TitleDisplay::Wrap);
        assert_eq!(TitleDisplay::Wrap.max_lines(), None);
    }

    /// The names are the shared vocabulary — the setting is one setting across the clients, so a
    /// machine that moves between builds reads back what it chose.
    #[test]
    fn the_names_are_the_ones_every_client_stores() {
        assert_eq!(TitleDisplay::Wrap.wire(), "wrap");
        assert_eq!(TitleDisplay::TwoLines.wire(), "twoLines");
        assert_eq!(TitleDisplay::OneLine.wire(), "oneLine");

        for display in TitleDisplay::all() {
            assert_eq!(
                TitleDisplay::parse(display.wire()),
                display,
                "{} round-trips",
                display.wire()
            );
        }
    }

    /// A value from a build that knows an option this one does not must not leave a blank list.
    #[test]
    fn an_option_this_build_does_not_know_is_the_default() {
        assert_eq!(TitleDisplay::parse("threeLines"), TitleDisplay::Wrap);
        assert_eq!(TitleDisplay::parse("  TWOLINES  "), TitleDisplay::TwoLines);
    }

    /// The picker reads as one dial turned down, not three unrelated choices.
    #[test]
    fn the_options_run_from_loosest_to_tightest() {
        assert_eq!(
            TitleDisplay::all(),
            [
                TitleDisplay::Wrap,
                TitleDisplay::TwoLines,
                TitleDisplay::OneLine
            ]
        );
        assert_eq!(TitleDisplay::TwoLines.max_lines(), Some(2));
        assert_eq!(TitleDisplay::OneLine.max_lines(), Some(1));
    }

    /// The shells are handed a number, so none of them matches on the names to draw a row.
    #[test]
    fn a_limit_is_a_count_and_never_a_sentinel() {
        for display in TitleDisplay::all() {
            assert_ne!(
                display.max_lines(),
                Some(0),
                "{} must not say zero lines",
                display.wire()
            );
        }
    }

    /// The setting is this installation's, like the theme, so it is kept in the cache and never
    /// travels. The key names the row, not the account.
    #[test]
    fn the_key_is_this_machines() {
        assert_eq!(KEY, "rows.titleDisplay");
        assert_ne!(KEY, crate::theme::KEY, "a key of its own, beside the theme");
    }
}
