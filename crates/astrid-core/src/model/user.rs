//! People, and the AI agents the server presents as people.
//!
//! Ported from `astrid-ios/Astrid App/Models/User.swift`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::date;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    /// Optional: the public-lists endpoint returns admins without their email addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(
        default,
        with = "date::optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at: Option<DateTime<Utc>>,
    /// `HH:MM`, the time of day a new all-day task defaults to when the user gives it a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_due_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_pending: Option<bool>,
    #[serde(default, rename = "isAIAgent", skip_serializing_if = "Option::is_none")]
    pub is_ai_agent: Option<bool>,
    #[serde(
        default,
        rename = "aiAgentType",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_agent_type: Option<String>,
}

impl User {
    /// Someone we hold only an id for.
    ///
    /// The resolver never answers "nobody" for an id it does not recognise: a minimal record still
    /// renders initials and still resolves a cached photo, where nothing at all left the previously
    /// drawn avatar on screen (task 42013da7).
    pub fn new(id: impl Into<String>) -> Self {
        User {
            id: id.into(),
            email: None,
            name: None,
            image: None,
            created_at: None,
            default_due_time: None,
            is_pending: None,
            is_ai_agent: None,
            ai_agent_type: None,
        }
    }

    /// The name we actually hold for somebody, if we hold one.
    ///
    /// `None` when there is neither a name nor an email — which is a real state, and one only the
    /// shell can word. [`User::display_name`] answers it with English, and English must not cross
    /// the boundary (rule 10), so anything projected for the screen uses this and lets the shell
    /// say "Unknown user" in the reader's own language.
    pub fn known_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .or_else(|| {
                self.email
                    .as_deref()
                    .map(str::trim)
                    .filter(|email| !email.is_empty())
            })
    }

    /// What to show where a name goes. Never empty: a person we hold only an id for still has to
    /// render as something.
    ///
    /// **Not for anything the shell draws** — the fallback is English. Use [`User::known_name`]
    /// there. This exists for logs, for sorting, and for the places inside the core that need a
    /// total function.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .or(self.email.as_deref().filter(|e| !e.trim().is_empty()))
            .unwrap_or("Unknown User")
    }

    /// Avatar fallback. One neutral glyph rather than `??` — two question marks read as an error,
    /// and this is simply someone we have no name for yet (task 42013da7).
    pub fn initials(&self) -> String {
        if let Some(name) = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            let mut words = name.split_whitespace();
            let first = words.next().unwrap_or_default();
            if let Some(second) = words.next() {
                return format!("{}{}", first_char(first), first_char(second)).to_uppercase();
            }
            return name.chars().take(2).collect::<String>().to_uppercase();
        }
        match self
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            Some(email) => email.chars().take(2).collect::<String>().to_uppercase(),
            None => "•".to_string(),
        }
    }

    pub fn is_agent(&self) -> bool {
        self.is_ai_agent.unwrap_or(false)
    }
}

fn first_char(word: &str) -> String {
    word.chars().take(1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(name: Option<&str>, email: Option<&str>) -> User {
        User {
            id: "u1".into(),
            email: email.map(str::to_string),
            name: name.map(str::to_string),
            image: None,
            created_at: None,
            default_due_time: None,
            is_pending: None,
            is_ai_agent: None,
            ai_agent_type: None,
        }
    }

    #[test]
    fn two_word_names_initial_both_words() {
        assert_eq!(user(Some("Ada Lovelace"), None).initials(), "AL");
    }

    #[test]
    fn one_word_names_take_two_letters() {
        assert_eq!(user(Some("Ada"), None).initials(), "AD");
    }

    #[test]
    fn without_a_name_the_email_stands_in() {
        assert_eq!(user(None, Some("ada@example.com")).initials(), "AD");
        assert_eq!(
            user(None, Some("ada@example.com")).display_name(),
            "ada@example.com"
        );
    }

    /// Not "??". See the doc comment on `initials`.
    #[test]
    fn with_neither_it_is_one_neutral_glyph() {
        assert_eq!(user(None, None).initials(), "•");
    }

    /// The server sends `""` for a name that was never set, and an empty avatar is worse than a
    /// missing one because it looks like a rendering fault.
    #[test]
    fn an_empty_name_is_treated_as_no_name() {
        assert_eq!(user(Some("   "), Some("ada@example.com")).initials(), "AD");
        assert_eq!(
            user(Some(""), Some("ada@example.com")).display_name(),
            "ada@example.com"
        );
    }

    /// The `isAIAgent` / `aiAgentType` keys are spelled with a capital AI on the wire. serde's
    /// camelCase rule would ask for `isAiAgent`, which silently reads as "not an agent" — the
    /// agent then renders as an ordinary person with no brand icon.
    #[test]
    fn the_agent_flags_keep_their_wire_spelling() {
        let decoded: User =
            serde_json::from_str(r#"{"id":"u1","isAIAgent":true,"aiAgentType":"claude"}"#)
                .expect("decodes");
        assert!(decoded.is_agent());
        assert_eq!(decoded.ai_agent_type.as_deref(), Some("claude"));
    }
}
