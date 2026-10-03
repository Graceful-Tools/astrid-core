//! The global quick-add chord — `Ctrl+Shift+A` unless the person picks another.
//!
//! A Windows convention layered on top of the shared keyboard scheme (see the module note on
//! [`super`]), and the one binding a person may change: it is registered system-wide, so on a
//! machine where another program already holds it the app has to be able to move. The rules for
//! what makes a chord are here rather than in the shell, so a chord the shell would fail to
//! register is refused with a reason before anything is saved.

use std::fmt;

/// What ships. `A` for Astrid, with Ctrl+Shift, which is the shape Windows apps use for a global
/// chord and is free on a default install.
pub const DEFAULT: &str = "Ctrl+Shift+A";

/// Where the choice is kept in the cache. This machine's, like the theme.
pub const KEY: &str = "shell.hotkey";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// An ASCII letter or digit, upper-cased.
    pub key: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordError {
    Empty,
    /// No Ctrl, Alt or Win: a chord that would fire on Shift+A while typing.
    NoModifier,
    /// Two keys, or none, or a key that is not a letter or a digit.
    BadKey(String),
    UnknownToken(String),
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChordError::Empty => write!(f, "type a shortcut such as Ctrl+Shift+A"),
            ChordError::NoModifier => write!(f, "a global shortcut needs Ctrl, Alt or Win"),
            ChordError::BadKey(key) => write!(f, "\"{key}\" is not a letter or a digit"),
            ChordError::UnknownToken(token) => write!(f, "\"{token}\" is not a key"),
        }
    }
}

/// Read `Ctrl+Shift+A`, in any case and any order, with `Control` and `Windows` accepted too.
pub fn parse(text: &str) -> Result<Chord, ChordError> {
    let mut chord = Chord {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
        key: '\0',
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(ChordError::Empty);
    }
    for token in text.split('+').map(str::trim) {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => chord.ctrl = true,
            "alt" => chord.alt = true,
            "shift" => chord.shift = true,
            "win" | "windows" => chord.win = true,
            "" => return Err(ChordError::BadKey(String::new())),
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(key), None) if key.is_ascii_alphanumeric() => {
                        if chord.key != '\0' {
                            return Err(ChordError::BadKey(token.to_string()));
                        }
                        chord.key = key.to_ascii_uppercase();
                    }
                    (Some(_), None) => return Err(ChordError::BadKey(token.to_string())),
                    _ => return Err(ChordError::UnknownToken(token.to_string())),
                }
            }
        }
    }
    if chord.key == '\0' {
        return Err(ChordError::BadKey(String::new()));
    }
    if !(chord.ctrl || chord.alt || chord.win) {
        return Err(ChordError::NoModifier);
    }
    Ok(chord)
}

impl fmt::Display for Chord {
    /// Canonical: `Ctrl+Alt+Shift+Win+A`, in that order, whichever were given.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(self.key.to_string());
        write!(f, "{}", parts.join("+"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_parses_and_prints_as_itself() {
        let chord = parse(DEFAULT).expect("parses");
        assert!(chord.ctrl && chord.shift && !chord.alt && !chord.win);
        assert_eq!(chord.key, 'A');
        assert_eq!(chord.to_string(), DEFAULT);
    }

    #[test]
    fn case_order_and_spelling_do_not_matter() {
        assert_eq!(
            parse(" shift + control + a ").expect("parses").to_string(),
            "Ctrl+Shift+A"
        );
        assert_eq!(
            parse("Windows+Alt+7").expect("parses").to_string(),
            "Alt+Win+7"
        );
    }

    /// Shift+A while typing is a capital A. A global chord has to be one nothing else wants.
    #[test]
    fn a_chord_without_a_real_modifier_is_refused() {
        assert_eq!(parse("Shift+A"), Err(ChordError::NoModifier));
        assert_eq!(parse("A"), Err(ChordError::NoModifier));
    }

    #[test]
    fn a_chord_needs_exactly_one_letter_or_digit() {
        assert_eq!(parse("Ctrl+Shift"), Err(ChordError::BadKey(String::new())));
        assert_eq!(parse("Ctrl+A+B"), Err(ChordError::BadKey("B".into())));
        assert_eq!(parse("Ctrl+F5"), Err(ChordError::UnknownToken("F5".into())));
        assert_eq!(parse("Ctrl+-"), Err(ChordError::BadKey("-".into())));
        assert_eq!(parse("   "), Err(ChordError::Empty));
    }
}
