//! Which look the app wears.
//!
//! Ports `ThemeMode` from `astrid-ios/Astrid App/Models/ThemeMode.swift`, which the Mac stores in
//! `@AppStorage("themeMode")` and reads in its root view, its sidebar and its settings pane.
//!
//! ## Four modes, and why one of them is not an appearance
//!
//! `light`, `dark` and `auto` are the three every app has. **Ocean is the brand look**: a light
//! appearance with a cyan surface behind the chrome. It is the default, which is the part worth
//! knowing — somebody who has never opened settings is looking at Ocean, not at "light", and a
//! client that quietly started in light mode would not look like Astrid.
//!
//! ## The choice belongs to the machine
//!
//! Not to the account, deliberately, and the Mac does the same. A theme answers "what does this
//! screen look like in this room" — a laptop in the evening and a desktop under an office light
//! are different questions, and syncing the answer gets one of them wrong.
//!
//! ## What this module does not decide
//!
//! The colours. WinUI themes through resource dictionaries, and a core that shipped hex values for
//! the shell to paste into brushes would be fighting the platform for no gain. The mode is the
//! contract — the same four names on every client — and each shell renders it.

use serde::{Deserialize, Serialize};

/// Where the chosen mode is kept. The cache, so it belongs to this installation.
pub const KEY: &str = "theme.mode";

/// The look the app wears.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// The brand look: light, with a cyan surface. The default.
    #[default]
    Ocean,
    Light,
    Dark,
    /// Whatever the system is set to.
    Auto,
}

impl Theme {
    /// The name this travels under, and the one the Apple clients store.
    pub fn wire(self) -> &'static str {
        match self {
            Theme::Ocean => "ocean",
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::Auto => "auto",
        }
    }

    /// Read a stored name.
    ///
    /// Anything unrecognised is the default rather than an error. A value written by a newer build
    /// — or by a client that grows a fifth theme — must not leave somebody with an app that will
    /// not draw.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            "auto" => Theme::Auto,
            _ => Theme::Ocean,
        }
    }

    /// Every mode, in the order a picker should offer them.
    ///
    /// The default first, then light and dark, then the one that defers to the system — which
    /// reads last because it is the answer for somebody who does not want to choose.
    pub fn all() -> [Theme; 4] {
        [Theme::Ocean, Theme::Light, Theme::Dark, Theme::Auto]
    }

    /// Whether this mode draws light or dark, or leaves it to the system.
    ///
    /// Ocean is a *light* appearance with a different surface behind it, so a shell asking "which
    /// appearance is this" gets `Some(false)` rather than a fourth answer it has to special-case.
    pub fn is_dark(self) -> Option<bool> {
        match self {
            Theme::Ocean | Theme::Light => Some(false),
            Theme::Dark => Some(true),
            Theme::Auto => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Somebody who has never opened settings is looking at Ocean. A client that started in light
    /// would not look like Astrid.
    #[test]
    fn the_default_is_the_brand_look() {
        assert_eq!(Theme::default(), Theme::Ocean);
        assert_eq!(Theme::parse(""), Theme::Ocean);
        assert_eq!(Theme::all()[0], Theme::Ocean);
    }

    /// The names are shared with the Apple clients, which store the same four strings.
    #[test]
    fn the_names_are_the_ones_every_client_stores() {
        assert_eq!(Theme::Ocean.wire(), "ocean");
        assert_eq!(Theme::Light.wire(), "light");
        assert_eq!(Theme::Dark.wire(), "dark");
        assert_eq!(Theme::Auto.wire(), "auto");

        for theme in Theme::all() {
            assert_eq!(
                Theme::parse(theme.wire()),
                theme,
                "{} round-trips",
                theme.wire()
            );
        }
    }

    /// A value from a build that knows a theme this one does not must not leave a blank window.
    #[test]
    fn a_theme_this_build_does_not_know_is_the_default() {
        assert_eq!(Theme::parse("solarized"), Theme::Ocean);
        assert_eq!(Theme::parse("  DARK  "), Theme::Dark);
    }

    /// Ocean is a light appearance with a different surface, not a third kind of light.
    #[test]
    fn ocean_draws_light() {
        assert_eq!(Theme::Ocean.is_dark(), Some(false));
        assert_eq!(Theme::Light.is_dark(), Some(false));
        assert_eq!(Theme::Dark.is_dark(), Some(true));
        assert_eq!(Theme::Auto.is_dark(), None, "the system decides");
    }
}
