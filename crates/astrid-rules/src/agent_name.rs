//! Whether a custom agent's name can be registered — judged while somebody is typing it.
//!
//! ## Why the rule is here and not in the field
//!
//! `POST /api/v1/custom-agents/register` refuses a bad name with a 400, and the name becomes an
//! address (`<name>.oc@<agent domain>`), so the server's rule is the only one that counts. A
//! client that let somebody finish typing and then relayed a 400 would be a client that asks for
//! a name twice; a client that invented its own rule would eventually accept a name the server
//! refuses, which is worse, because the person has no way to see which of the two is wrong.
//!
//! So this is a transcription of `app/api/v1/custom-agents/register/route.ts`, deliberately
//! verdict-for-verdict:
//!
//! - the name is lowercased and trimmed first, so `  Scout  ` is the perfectly good name `scout`
//!   and uppercase is never itself an error;
//! - then `^[a-z0-9][a-z0-9._-]{0,30}[a-z0-9]$` — two to thirty-two characters, starting and
//!   ending alphanumeric, with dots, hyphens and underscores allowed in between;
//! - then a reserved list, which the server keeps because those addresses mean something else.
//!
//! What it says *about* a refusal is finer-grained than the server's single sentence, which is the
//! whole point of asking as you type: "that character cannot go in a name" is actionable and
//! "Invalid agent name" is not. The **set** of names it accepts is identical, and the tests at the
//! bottom are written against the regex rather than against the prose.
//!
//! No sentence is returned, only which rule was broken — the shells own the words
//! ([`crate::rows::credential`] and the OAuth draft do the same), and a string from here could not
//! be localised.

use serde::Serialize;

/// Why a name cannot be registered. The first rule broken, in the order the server applies them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum NameProblem {
    /// Nothing has been typed yet, or only whitespace.
    Missing,
    /// One character. A name is an address, and the pattern asks for at least two.
    TooShort,
    /// Longer than [`MAX_LENGTH`].
    TooLong,
    /// Does not begin with a letter or a digit.
    StartNotAlphanumeric,
    /// Does not end with a letter or a digit — a trailing dot, hyphen or underscore.
    EndNotAlphanumeric,
    /// A character that may not appear in a name at all. The first such character, so a message
    /// can name it rather than saying the whole name is wrong.
    #[serde(rename_all = "camelCase")]
    DisallowedCharacter { character: String },
    /// A name the server keeps for itself.
    Reserved,
}

/// The longest a name may be, from the server's pattern: one, up to thirty, one.
pub const MAX_LENGTH: usize = 32;

/// The shortest: the pattern's first and last character cannot be the same one.
pub const MIN_LENGTH: usize = 2;

/// Names the server refuses whatever else is true of them (`RESERVED_NAMES`).
pub const RESERVED: [&str; 7] = [
    "admin", "system", "test", "api", "support", "root", "openclaw",
];

/// What a name field knows about what is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NameCheck {
    /// The name as a register would send it: lowercased and trimmed, exactly as the server
    /// normalises it before judging. A screen may show this to explain what will be created.
    pub name: String,
    /// The rule broken, or nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<NameProblem>,
    /// Whether registering it now would be accepted — bar the one thing only the server can know,
    /// which is whether somebody already has this name.
    pub can_register: bool,
    /// Whether the field is effectively empty. Beside the problem rather than folded into it, so
    /// a view can leave an untouched field unmarked without having to interpret `Missing` —
    /// the same arrangement [`crate::rules::Rule::SearchParse`] uses for an empty query.
    pub is_empty: bool,
}

/// Judge one name.
pub fn check(input: &str) -> NameCheck {
    let name = normalize(input);
    let problem = problem_with(&name);
    NameCheck {
        is_empty: name.is_empty(),
        can_register: problem.is_none(),
        problem,
        name,
    }
}

/// Lowercase, then trim — the server's two steps, in its order.
pub fn normalize(input: &str) -> String {
    input.to_lowercase().trim().to_string()
}

/// The first rule an already-normalised name breaks.
fn problem_with(name: &str) -> Option<NameProblem> {
    if name.is_empty() {
        return Some(NameProblem::Missing);
    }
    // Characters, not bytes: a name of two accented letters is two characters long and fails on
    // the character rule below, not on a byte count that would call it long enough.
    let characters: Vec<char> = name.chars().collect();
    if characters.len() < MIN_LENGTH {
        return Some(NameProblem::TooShort);
    }
    if characters.len() > MAX_LENGTH {
        return Some(NameProblem::TooLong);
    }
    // A character the pattern does not allow anywhere beats a position rule: it is the more
    // useful thing to say, and the server refuses the name either way.
    if let Some(character) = characters.iter().find(|it| !is_inner(**it)) {
        return Some(NameProblem::DisallowedCharacter {
            character: character.to_string(),
        });
    }
    if !is_alphanumeric(characters[0]) {
        return Some(NameProblem::StartNotAlphanumeric);
    }
    if !is_alphanumeric(characters[characters.len() - 1]) {
        return Some(NameProblem::EndNotAlphanumeric);
    }
    if RESERVED.contains(&name) {
        return Some(NameProblem::Reserved);
    }
    None
}

/// `[a-z0-9]` — the pattern's own class, which is ASCII and not `char::is_alphanumeric`: the
/// server's regex has no unicode flag, so `é` and `٣` are not letters or digits to it.
fn is_alphanumeric(character: char) -> bool {
    character.is_ascii_lowercase() || character.is_ascii_digit()
}

/// `[a-z0-9._-]` — what may appear anywhere in a name.
fn is_inner(character: char) -> bool {
    is_alphanumeric(character) || matches!(character, '.' | '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server's pattern, so every case below is checked against the rule it transcribes
    /// rather than against this file's opinion of it (task 959b24b5).
    fn server_would_accept(input: &str) -> bool {
        let name = normalize(input);
        let characters: Vec<char> = name.chars().collect();
        let matches_pattern = (MIN_LENGTH..=MAX_LENGTH).contains(&characters.len())
            && is_alphanumeric(characters[0])
            && is_alphanumeric(characters[characters.len() - 1])
            && characters[1..characters.len().saturating_sub(1)]
                .iter()
                .all(|it| is_inner(*it));
        matches_pattern && !RESERVED.contains(&name.as_str())
    }

    #[track_caller]
    fn assert_agrees_with_server(input: &str) {
        assert_eq!(
            check(input).can_register,
            !normalize(input).is_empty() && server_would_accept(input),
            "disagreed with the server about {input:?}"
        );
    }

    #[test]
    fn a_plain_name_can_be_registered() {
        let check = check("scout");
        assert!(check.can_register);
        assert_eq!(check.problem, None);
        assert!(!check.is_empty);
        assert_eq!(check.name, "scout");
    }

    /// Uppercase is not an error — the server lowercases before it judges — and neither is
    /// surrounding whitespace. A field that refused `Scout` would be refusing a good name.
    #[test]
    fn case_and_surrounding_space_are_normalised_rather_than_refused() {
        let check = check("  Scout  ");
        assert!(check.can_register);
        assert_eq!(
            check.name, "scout",
            "what a register would actually send, so a screen can say so"
        );
    }

    /// An untouched field is empty, not wrong. The flag is what lets a view say nothing yet.
    #[test]
    fn an_empty_field_is_reported_as_empty() {
        for input in ["", "   ", "\t\n"] {
            let check = check(input);
            assert!(check.is_empty, "{input:?}");
            assert_eq!(check.problem, Some(NameProblem::Missing));
            assert!(!check.can_register);
        }
    }

    #[test]
    fn the_length_rules_are_the_patterns() {
        assert_eq!(check("a").problem, Some(NameProblem::TooShort));
        assert!(check("ab").can_register, "two characters is the minimum");

        let longest = "a".repeat(MAX_LENGTH);
        assert!(check(&longest).can_register, "thirty-two is allowed");
        assert_eq!(
            check(&"a".repeat(MAX_LENGTH + 1)).problem,
            Some(NameProblem::TooLong)
        );
    }

    #[test]
    fn dots_hyphens_and_underscores_belong_inside_a_name_only() {
        assert!(check("build-bot_2.0").can_register);
        assert_eq!(
            check(".scout").problem,
            Some(NameProblem::StartNotAlphanumeric)
        );
        assert_eq!(
            check("scout-").problem,
            Some(NameProblem::EndNotAlphanumeric)
        );
        assert_eq!(check("_").problem, Some(NameProblem::TooShort));
    }

    /// Naming the character is the reason to ask as you type at all: "Invalid agent name" does
    /// not tell anybody that it was the space.
    #[test]
    fn a_character_a_name_may_not_contain_is_named() {
        assert_eq!(
            check("my scout").problem,
            Some(NameProblem::DisallowedCharacter {
                character: " ".to_string()
            })
        );
        assert_eq!(
            check("scout@astrid").problem,
            Some(NameProblem::DisallowedCharacter {
                character: "@".to_string()
            })
        );
    }

    /// The regex has no unicode flag, so an accented letter is not a letter to the server. A
    /// client that accepted it would be a client whose registrations fail at the last step.
    #[test]
    fn a_letter_the_servers_pattern_does_not_know_is_refused() {
        assert_eq!(
            check("café").problem,
            Some(NameProblem::DisallowedCharacter {
                character: "é".to_string()
            })
        );
        assert_agrees_with_server("café");
    }

    #[test]
    fn a_reserved_name_is_refused_even_though_it_is_well_formed() {
        for name in RESERVED {
            assert_eq!(check(name).problem, Some(NameProblem::Reserved), "{name}");
        }
        // Including through normalisation: `Admin` is `admin`.
        assert_eq!(check(" Admin ").problem, Some(NameProblem::Reserved));
        // But a reserved name as a prefix is a name like any other.
        assert!(check("admin-helper").can_register);
    }

    /// The contract that matters: this and the server accept exactly the same names. The reasons
    /// may be finer here; the verdict may not differ.
    #[test]
    fn the_verdict_is_the_servers_verdict() {
        for input in [
            "scout",
            "Scout",
            "  scout  ",
            "a",
            "ab",
            "a-b",
            "a..b",
            "a_b-c.d",
            "-scout",
            "scout.",
            "scout bot",
            "SCOUT_BOT",
            "admin",
            "openclaw",
            "admin2",
            "café",
            "1",
            "12",
            "0_0",
            &"a".repeat(MAX_LENGTH),
            &"a".repeat(MAX_LENGTH + 1),
        ] {
            assert_agrees_with_server(input);
        }
    }

    /// The wire shape a shell reads: no problem key at all when there is nothing wrong, and a
    /// tagged one when there is, so a `switch` on `kind` is exhaustive.
    #[test]
    fn the_answer_leaves_out_a_problem_it_does_not_have() {
        let good = serde_json::to_value(check("scout")).unwrap();
        assert!(good.get("problem").is_none());
        assert_eq!(good["canRegister"], serde_json::json!(true));
        assert_eq!(good["name"], serde_json::json!("scout"));

        let bad = serde_json::to_value(check("my scout")).unwrap();
        assert_eq!(
            bad["problem"]["kind"],
            serde_json::json!("disallowedCharacter")
        );
        assert_eq!(bad["problem"]["character"], serde_json::json!(" "));
        assert_eq!(bad["canRegister"], serde_json::json!(false));
    }
}
