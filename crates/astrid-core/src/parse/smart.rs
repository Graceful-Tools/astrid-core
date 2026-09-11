//! What the quick-add box reads out of a sentence: "Call mum tomorrow urgent #health".
//!
//! Ported from `parseTaskInput` in `astrid-web/lib/task-manager-utils.ts`, with the keyword
//! tables of `lib/i18n/nlp-keywords.ts` carried in `contracts/fixtures/smart.json` rather than
//! retyped — twelve languages of "tomorrow" is not something to copy by hand — and locked by the
//! same fixture, which records what the web's parser answers for a set of inputs under a pinned
//! clock. This closes CONTRACTS.md D11; what still differs is written there (D12, D13).
//!
//! The pipeline, in the web's order, each step taking its words out of the title:
//!
//! 1. `#tags` file the task ([`super::quick_add::extract_lists`]).
//! 2. "weekly monday and wednesday" — a custom weekly repeat, due on the first of those days.
//! 3. "daily", "monthly", "every year" — a simple repeat.
//! 4. "tomorrow", "next week", "this week", a weekday — a due date. Only when step 2 gave none.
//! 5. "urgent", "high priority", "low priority" — a priority. **Low is nothing**, faithfully:
//!    the web reads `0 || undefined`, so "low priority" strips the words and sets no priority.
//!
//! The matching is the web's regular expressions, re-spelled: a keyword must sit on `\b` word
//! boundaries (ASCII word characters, as JavaScript's `\b` sees them), the earliest match in
//! the text wins, and among the table's words the first listed wins at that position. A table
//! with CJK words uses whitespace or the ends of the text as its boundaries instead, and takes
//! the whitespace before the word with it.

use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Deserialize;

use crate::model::TaskList;

const FIXTURE: &str = include_str!("../../../../contracts/fixtures/smart.json");

/// One locale's words.
#[derive(Debug, Clone, Deserialize)]
pub struct Keywords {
    pub dates: HashMap<String, Vec<String>>,
    pub priorities: HashMap<String, Vec<String>>,
    pub repeating: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct Tables {
    keywords: HashMap<String, Keywords>,
}

fn tables() -> &'static HashMap<String, Keywords> {
    static TABLES: OnceLock<HashMap<String, Keywords>> = OnceLock::new();
    TABLES.get_or_init(|| {
        serde_json::from_str::<Tables>(FIXTURE)
            .expect("contracts/fixtures/smart.json carries the keyword tables")
            .keywords
    })
}

impl Keywords {
    /// The words for a locale — `en-US`, `de`, `zh-CN` — the way the web's `getNLPKeywords`
    /// picks them: the tag as given, then its language alone, then English.
    pub fn for_locale(locale: &str) -> &'static Keywords {
        let tables = tables();
        let wanted = locale.to_lowercase();
        if let Some(found) = tables.iter().find(|(key, _)| key.to_lowercase() == wanted) {
            return found.1;
        }
        let language = wanted.split('-').next().unwrap_or("");
        if let Some(found) = tables
            .iter()
            .find(|(key, _)| key.to_lowercase() == language)
        {
            return found.1;
        }
        tables.get("en").expect("the tables carry English")
    }

    fn words<'a>(&self, group: &'a HashMap<String, Vec<String>>, names: &[&str]) -> Vec<&'a str> {
        names
            .iter()
            .flat_map(|name| group.get(*name).into_iter().flatten())
            .map(String::as_str)
            .collect()
    }
}

const DAY_NAMES: [(&str, Weekday); 7] = [
    ("monday", Weekday::Mon),
    ("tuesday", Weekday::Tue),
    ("wednesday", Weekday::Wed),
    ("thursday", Weekday::Thu),
    ("friday", Weekday::Fri),
    ("saturday", Weekday::Sat),
    ("sunday", Weekday::Sun),
];

/// What the sentence said. Only what was said: list defaults are somebody else's business.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// The sentence with its words taken out — or the sentence as typed, when nothing was left.
    pub title: String,
    /// The lists the `#tags` named, in order.
    pub list_ids: Vec<String>,
    /// The calendar day a date word meant. See CONTRACTS.md D12 for why a day and not an instant.
    pub due_day: Option<NaiveDate>,
    /// 1–3. Never 0: see the module note.
    pub priority: Option<i64>,
    /// `daily` | `weekly` | `monthly` | `yearly` | `custom`.
    pub repeating: Option<String>,
    /// For `custom`: the English day names, in the order they were typed.
    pub weekdays: Vec<String>,
}

/// Read the sentence, with `today` as the reader's calendar day.
pub fn parse(input: &str, lists: &[TaskList], keywords: &Keywords, today: NaiveDate) -> Parsed {
    let mut parsed = Parsed::default();
    let (mut title, list_ids) = super::quick_add::extract_lists(input.trim(), lists);
    parsed.list_ids = list_ids;

    // 2. "weekly monday and wednesday".
    let day_words = keywords.words(&keywords.dates, &DAY_NAMES.map(|(name, _)| name));
    let weekly_words = keywords.words(&keywords.repeating, &["weekly", "everyWeek"]);
    if let Some((span, days)) = weekly_with_days(&title, &weekly_words, &day_words) {
        let names: Vec<String> = days
            .iter()
            .filter_map(|day| localized_day(day, keywords))
            .map(|(name, _)| name.to_string())
            .collect();
        if !names.is_empty() {
            parsed.repeating = Some("custom".into());
            let first = DAY_NAMES
                .iter()
                .find(|(name, _)| *name == names[0])
                .map(|(_, day)| *day)
                .expect("a mapped day is one of seven");
            parsed.due_day = Some(next_weekday(today, first));
            parsed.weekdays = names;
            title = remove(&title, span);
        }
    }

    // 3. A simple repeat.
    if parsed.repeating.is_none() {
        let categories = [
            (["daily", "everyDay"], "daily"),
            (["weekly", "everyWeek"], "weekly"),
            (["monthly", "everyMonth"], "monthly"),
            (["yearly", "everyYear"], "yearly"),
        ];
        for (names, value) in categories {
            let words = keywords.words(&keywords.repeating, &names);
            if let Some(span) = find_keyword(&title, &words) {
                parsed.repeating = Some(value.into());
                title = remove(&title, span);
                break;
            }
        }
    }

    // 4. A date word. The weekly pattern's date stands; the word still leaves the title.
    let date_names = [
        "today",
        "tomorrow",
        "nextWeek",
        "thisWeek",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let date_words = keywords.words(&keywords.dates, &date_names);
    if let Some(span) = find_keyword(&title, &date_words) {
        if parsed.due_day.is_none() {
            let word = title[span.0..span.1].trim().to_lowercase();
            let is = |name: &str| {
                keywords
                    .dates
                    .get(name)
                    .is_some_and(|words| words.iter().any(|w| w.to_lowercase() == word))
            };
            parsed.due_day = if is("today") {
                Some(today)
            } else if is("tomorrow") {
                Some(today + Duration::days(1))
            } else if is("nextWeek") {
                Some(today + Duration::days(7))
            } else if is("thisWeek") {
                // Friday of this week — today, when today is Friday.
                let until_friday = (5 + 7 - i64::from(today.weekday().num_days_from_sunday())) % 7;
                Some(today + Duration::days(until_friday))
            } else {
                localized_day(&word, keywords).map(|(_, day)| next_weekday(today, day))
            };
        }
        title = remove(&title, span);
    }

    // 5. A priority.
    let priorities = [("highest", 3), ("high", 2), ("medium", 1), ("low", 0)];
    for (name, value) in priorities {
        let words = keywords.words(&keywords.priorities, &[name]);
        if let Some(span) = find_keyword(&title, &words) {
            // `priority || undefined` on the web: zero is nothing.
            parsed.priority = (value != 0).then_some(value);
            title = remove(&title, span);
            break;
        }
    }

    parsed.title = if title.is_empty() {
        input.trim().to_string()
    } else {
        title
    };
    parsed
}

/// The next `day` strictly after `today` — a week away when today is that day.
fn next_weekday(today: NaiveDate, day: Weekday) -> NaiveDate {
    let target = i64::from(day.num_days_from_sunday());
    let current = i64::from(today.weekday().num_days_from_sunday());
    let mut ahead = target - current;
    if ahead <= 0 {
        ahead += 7;
    }
    today + Duration::days(ahead)
}

/// Which day a word names, in any locale.
fn localized_day(word: &str, keywords: &Keywords) -> Option<(&'static str, Weekday)> {
    let lower = word.to_lowercase();
    DAY_NAMES.iter().copied().find(|(name, _)| {
        keywords
            .dates
            .get(*name)
            .is_some_and(|words| words.iter().any(|w| w.to_lowercase() == lower))
    })
}

/// Take a matched span out and tidy the whitespace, as the web's `.replace(/\s+/g, ' ').trim()`.
fn remove(text: &str, (start, end): (usize, usize)) -> String {
    let joined = format!("{}{}", &text[..start], &text[end..]);
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ─── The web's regular expressions, re-spelled ───────────────────────────────────────────────

/// JavaScript's `\w`.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// JavaScript's `\b` at a byte index: one side a word character, the other not.
fn is_boundary(text: &str, at: usize) -> bool {
    let before = text[..at].chars().next_back().is_some_and(is_word);
    let after = text[at..].chars().next().is_some_and(is_word);
    before != after
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4e00}'..='\u{9fff}' | '\u{3040}'..='\u{309f}' | '\u{30a0}'..='\u{30ff}' | '\u{ac00}'..='\u{d7af}')
}

/// Whether `word` sits at `at` in `text`, case-insensitively. Returns the end.
fn word_at(text: &str, at: usize, word: &str) -> Option<usize> {
    let mut end = at;
    let mut have = text[at..].chars();
    for wanted in word.chars() {
        let got = have.next()?;
        if got.to_lowercase().ne(wanted.to_lowercase()) {
            return None;
        }
        end += got.len_utf8();
    }
    Some(end)
}

/// `buildKeywordRegex(words)` applied once: the earliest position at which any word matches on
/// its boundaries, and at that position the first word in the table. The span includes the
/// leading whitespace when the table is CJK, as the web's pattern consumes it.
fn find_keyword(text: &str, words: &[&str]) -> Option<(usize, usize)> {
    if words.is_empty() {
        return None;
    }
    let cjk = words.iter().any(|w| w.chars().any(is_cjk));
    let mut at = 0;
    while at <= text.len() {
        if !text.is_char_boundary(at) {
            at += 1;
            continue;
        }
        if cjk {
            // `(?:^|\s)(word)(?=\s|$)`: the match starts at the text's start, or on the
            // whitespace before the word, which it takes with it.
            let mut starts = Vec::with_capacity(2);
            if at == 0 {
                starts.push(0);
            }
            if let Some(space) = text[at..].chars().next().filter(|c| c.is_whitespace()) {
                starts.push(at + space.len_utf8());
            }
            for word_start in starts {
                for word in words {
                    if let Some(end) = word_at(text, word_start, word) {
                        let followed = end == text.len()
                            || text[end..].chars().next().is_some_and(char::is_whitespace);
                        if followed {
                            return Some((at, end));
                        }
                    }
                }
            }
        } else if is_boundary(text, at) {
            for word in words {
                if let Some(end) = word_at(text, at, word) {
                    if is_boundary(text, end) {
                        return Some((at, end));
                    }
                }
            }
        }
        at += 1;
    }
    None
}

/// `\b(weekly)\s+(day(?:\s+(?:and|und|et|y|e)\s+day|\s*,\s*day)*)\b`: the span of the whole
/// phrase, and the day words as typed.
fn weekly_with_days<'a>(
    text: &'a str,
    weekly: &[&str],
    days: &[&str],
) -> Option<((usize, usize), Vec<&'a str>)> {
    let mut at = 0;
    while at <= text.len() {
        if !text.is_char_boundary(at) || !is_boundary(text, at) {
            at += 1;
            continue;
        }
        for word in weekly {
            let Some(after_weekly) = word_at(text, at, word) else {
                continue;
            };
            // `\s+`
            let rest = &text[after_weekly..];
            let spaces = rest.chars().take_while(|c| c.is_whitespace()).count();
            if spaces == 0 {
                continue;
            }
            let mut cursor =
                after_weekly + rest.chars().take(spaces).map(char::len_utf8).sum::<usize>();
            let Some(first_end) = day_at(text, cursor, days) else {
                continue;
            };
            let mut found = vec![&text[cursor..first_end]];
            cursor = first_end;
            // The separators, greedily.
            while let Some((next_start, next_end)) = separator_then_day(text, cursor, days) {
                found.push(&text[next_start..next_end]);
                cursor = next_end;
            }
            if is_boundary(text, cursor) {
                return Some(((at, cursor), found));
            }
        }
        at += 1;
    }
    None
}

fn day_at(text: &str, at: usize, days: &[&str]) -> Option<usize> {
    days.iter().find_map(|day| word_at(text, at, day))
}

/// `\s+(?:and|und|et|y|e)\s+day` or `\s*,\s*day`, starting at `at`.
fn separator_then_day(text: &str, at: usize, days: &[&str]) -> Option<(usize, usize)> {
    let rest = &text[at..];
    let leading: usize = rest
        .chars()
        .take_while(|c| c.is_whitespace())
        .map(char::len_utf8)
        .sum();
    let after_space = at + leading;
    // `\s*,\s*day`
    if text[after_space..].starts_with(',') {
        let after_comma = after_space + 1;
        let more: usize = text[after_comma..]
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(char::len_utf8)
            .sum();
        let start = after_comma + more;
        return day_at(text, start, days).map(|end| (start, end));
    }
    // `\s+(and|und|et|y|e)\s+day`
    if leading == 0 {
        return None;
    }
    for joiner in ["and", "und", "et", "y", "e"] {
        let Some(after_joiner) = word_at(text, after_space, joiner) else {
            continue;
        };
        let more: usize = text[after_joiner..]
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(char::len_utf8)
            .sum();
        if more == 0 {
            continue;
        }
        let start = after_joiner + more;
        if let Some(end) = day_at(text, start, days) {
            return Some((start, end));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> &'static Keywords {
        Keywords::for_locale("en-US")
    }

    fn wednesday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 9).expect("a date")
    }

    #[test]
    fn a_sentence_gives_up_its_date_priority_and_lists() {
        let lists = [TaskList::new("h", "Health")];
        let parsed = parse("Ship it tomorrow urgent #health", &lists, en(), wednesday());
        assert_eq!(parsed.title, "Ship it");
        assert_eq!(parsed.list_ids, vec!["h".to_string()]);
        assert_eq!(parsed.due_day, NaiveDate::from_ymd_opt(2026, 9, 10));
        assert_eq!(parsed.priority, Some(3));
        assert_eq!(parsed.repeating, None);
    }

    #[test]
    fn weekly_with_days_is_a_custom_repeat_due_on_the_first_day() {
        let parsed = parse("Standup weekly mon and wed", &[], en(), wednesday());
        assert_eq!(parsed.title, "Standup");
        assert_eq!(parsed.repeating.as_deref(), Some("custom"));
        assert_eq!(parsed.weekdays, vec!["monday", "wednesday"]);
        assert_eq!(
            parsed.due_day,
            NaiveDate::from_ymd_opt(2026, 9, 14),
            "next Monday"
        );
    }

    /// The web reads `0 || undefined`: "low priority" leaves the title and sets nothing.
    #[test]
    fn low_priority_is_no_priority_faithfully() {
        let parsed = parse("Tidy the desk low priority", &[], en(), wednesday());
        assert_eq!(parsed.title, "Tidy the desk");
        assert_eq!(parsed.priority, None);
    }

    #[test]
    fn a_word_inside_a_word_is_not_a_keyword() {
        let parsed = parse("Tomorrowland tickets", &[], en(), wednesday());
        assert_eq!(parsed.title, "Tomorrowland tickets");
        assert_eq!(parsed.due_day, None);
    }

    #[test]
    fn a_sentence_that_was_all_keywords_keeps_its_words() {
        let parsed = parse("today", &[], en(), wednesday());
        assert_eq!(parsed.title, "today");
        assert_eq!(parsed.due_day, Some(wednesday()));
    }

    #[test]
    fn a_locale_falls_back_to_its_language_and_then_to_english() {
        assert!(Keywords::for_locale("de-AT").dates["tomorrow"].contains(&"morgen".to_string()));
        assert!(Keywords::for_locale("xx-YY").dates["tomorrow"].contains(&"tomorrow".to_string()));
    }
}
