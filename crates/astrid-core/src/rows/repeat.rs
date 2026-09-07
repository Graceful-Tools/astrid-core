//! How a repeat describes itself, and what the editor offers.
//!
//! Ports `astrid-ios/Astrid App/Core/Layout/CustomRepeatSummary.swift` (task 42013da7) and the
//! preset list its picker draws.
//!
//! The Swift builds an English sentence — `"Every 2 weeks on Mon, Wed, from due date"` — by
//! concatenation. That cannot cross this boundary: the core returns no words (rule 10), and a
//! sentence assembled from fragments is also the thing that does not survive translation, where
//! the order of "every 2 weeks" and "on Mondays" is not the English order.
//!
//! So a summary is a **sequence of parts**, each a resource key with the numbers and names it
//! needs. The shell joins them. That keeps the decisions here — which parts a pattern has, in what
//! order, and what "every 1 week" collapses to — and leaves only the words over there.
//!
//! The next-occurrence arithmetic stays in [`crate::repeating`] and is not repeated here. This
//! module reads a pattern; it never advances one.

use serde::Serialize;

use crate::model::{CustomRepeatingPattern, RepeatFromMode, Repeating};

/// One fragment of the sentence: a key, plus whatever it interpolates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryPart {
    /// A resource key. Never a word.
    pub key: &'static str,
    /// The number the key's plural form and its `{0}` refer to, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    /// Names the key interpolates — weekdays as the wire spells them, so the shell translates
    /// them from its own table rather than being handed English.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// An instant, for "until 3 March".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

impl SummaryPart {
    fn key(key: &'static str) -> Self {
        SummaryPart {
            key,
            count: None,
            values: Vec::new(),
            date: None,
        }
    }

    fn counted(key: &'static str, count: i64) -> Self {
        SummaryPart {
            count: Some(count),
            ..Self::key(key)
        }
    }
}

/// One choice in the repeat picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatPreset {
    /// The value to write back, exactly as the wire spells it: `never`, `daily`, …, `custom`.
    pub value: &'static str,
    pub title_key: &'static str,
    pub is_selected: bool,
}

/// The presets, in the order the picker shows them. "Never" first: clearing a repeat is a choice
/// like any other rather than an escape hatch, the same decision the date picks made.
const PRESETS: [(&str, &str); 6] = [
    ("never", "repeat.never"),
    ("daily", "repeat.daily"),
    ("weekly", "repeat.weekly"),
    ("monthly", "repeat.monthly"),
    ("yearly", "repeat.yearly"),
    ("custom", "repeat.custom"),
];

pub fn presets(selected: Option<Repeating>) -> Vec<RepeatPreset> {
    let selected = wire_value(selected.unwrap_or(Repeating::Never));
    PRESETS
        .iter()
        .map(|(value, title_key)| RepeatPreset {
            value,
            title_key,
            is_selected: *value == selected,
        })
        .collect()
}

fn wire_value(repeating: Repeating) -> &'static str {
    match repeating {
        Repeating::Never => "never",
        Repeating::Daily => "daily",
        Repeating::Weekly => "weekly",
        Repeating::Monthly => "monthly",
        Repeating::Yearly => "yearly",
        Repeating::Custom => "custom",
    }
}

/// Describe a task's repeat.
///
/// A simple preset describes itself in one part. A custom pattern is the interesting case: "Custom"
/// says nothing, and the detail screen has to word it exactly as the picker does or the same repeat
/// reads two ways on one screen — which is why this is one function rather than two.
///
/// An empty result means the task does not repeat.
pub fn summary(
    repeating: Option<Repeating>,
    pattern: Option<&CustomRepeatingPattern>,
    repeat_from: Option<RepeatFromMode>,
) -> Vec<SummaryPart> {
    let repeating = repeating.unwrap_or(Repeating::Never);
    let mut parts = match repeating {
        Repeating::Never => return Vec::new(),
        Repeating::Daily => vec![SummaryPart::key("repeat.daily")],
        Repeating::Weekly => vec![SummaryPart::key("repeat.weekly")],
        Repeating::Monthly => vec![SummaryPart::key("repeat.monthly")],
        Repeating::Yearly => vec![SummaryPart::key("repeat.yearly")],
        Repeating::Custom => custom_parts(pattern.unwrap_or(&CustomRepeatingPattern::default())),
    };

    // Only when it is the unusual one. Repeating from the completion date is the default the app
    // creates, and saying so on every row is noise that stops being read.
    if repeat_from == Some(RepeatFromMode::DueDate) {
        parts.push(SummaryPart::key("repeat.from_due_date"));
    }
    parts
}

fn custom_parts(pattern: &CustomRepeatingPattern) -> Vec<SummaryPart> {
    let interval = pattern.interval.unwrap_or(1);
    let unit = pattern.unit.as_deref().unwrap_or("days");
    let mut parts = vec![SummaryPart::counted(
        match unit {
            "weeks" => "repeat.every_n_weeks",
            "months" => "repeat.every_n_months",
            "years" => "repeat.every_n_years",
            // A unit written by a client this build has not met is still a repeat, and calling it
            // days is what the Swift does. Silence here would read as "does not repeat".
            _ => "repeat.every_n_days",
        },
        interval,
    )];

    match unit {
        "weeks" => {
            if let Some(weekdays) = pattern.weekdays.as_ref().filter(|days| !days.is_empty()) {
                parts.push(SummaryPart {
                    values: weekdays.clone(),
                    ..SummaryPart::key("repeat.on_weekdays")
                });
            }
        }
        "months" => match pattern.month_repeat_type.as_deref() {
            Some("same_date") => {
                if let Some(day) = pattern.month_day {
                    parts.push(SummaryPart::counted("repeat.on_day_of_month", day));
                }
            }
            Some("same_weekday") => {
                if let Some(weekday) = &pattern.month_weekday {
                    parts.push(SummaryPart {
                        count: Some(weekday.week_of_month),
                        values: vec![weekday.weekday.clone()],
                        ..SummaryPart::key("repeat.on_nth_weekday")
                    });
                }
            }
            _ => {}
        },
        "years" => {
            if let (Some(month), Some(day)) = (pattern.month, pattern.day) {
                parts.push(SummaryPart {
                    count: Some(day),
                    values: vec![month.to_string()],
                    ..SummaryPart::key("repeat.on_month_and_day")
                });
            }
        }
        _ => {}
    }

    match pattern.end_condition.as_deref() {
        Some("after_occurrences") => {
            if let Some(count) = pattern.end_after_occurrences {
                parts.push(SummaryPart::counted("repeat.ends_after", count));
            }
        }
        Some("until_date") => {
            if let Some(date) = pattern.end_until_date {
                parts.push(SummaryPart {
                    date: Some(date.to_rfc3339()),
                    ..SummaryPart::key("repeat.ends_on")
                });
            }
        }
        _ => {}
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{date, MonthWeekday};

    fn keys(parts: &[SummaryPart]) -> Vec<&str> {
        parts.iter().map(|part| part.key).collect()
    }

    #[test]
    fn a_task_that_does_not_repeat_has_nothing_to_say() {
        assert!(summary(None, None, None).is_empty());
        assert!(summary(Some(Repeating::Never), None, None).is_empty());
    }

    #[test]
    fn a_preset_describes_itself_in_one_part() {
        assert_eq!(
            keys(&summary(Some(Repeating::Weekly), None, None)),
            vec!["repeat.weekly"]
        );
    }

    /// Repeating from the completion date is what the app creates, so only the unusual one is
    /// worth a fragment — saying it on every row is noise that stops being read.
    #[test]
    fn only_repeating_from_the_due_date_is_worth_saying() {
        assert_eq!(
            keys(&summary(
                Some(Repeating::Daily),
                None,
                Some(RepeatFromMode::CompletionDate)
            )),
            vec!["repeat.daily"]
        );
        assert_eq!(
            keys(&summary(
                Some(Repeating::Daily),
                None,
                Some(RepeatFromMode::DueDate)
            )),
            vec!["repeat.daily", "repeat.from_due_date"]
        );
    }

    /// "Every 2 weeks on Monday, Wednesday" — the parts, in that order, with the weekdays as the
    /// wire spells them so the shell can translate them itself.
    #[test]
    fn a_weekly_custom_pattern_names_its_days() {
        let pattern = CustomRepeatingPattern {
            unit: Some("weeks".into()),
            interval: Some(2),
            weekdays: Some(vec!["monday".into(), "wednesday".into()]),
            ..Default::default()
        };
        let parts = summary(Some(Repeating::Custom), Some(&pattern), None);
        assert_eq!(
            keys(&parts),
            vec!["repeat.every_n_weeks", "repeat.on_weekdays"]
        );
        assert_eq!(parts[0].count, Some(2));
        assert_eq!(parts[1].values, vec!["monday", "wednesday"]);
    }

    #[test]
    fn a_monthly_pattern_says_which_day_or_which_weekday() {
        let by_date = CustomRepeatingPattern {
            unit: Some("months".into()),
            month_repeat_type: Some("same_date".into()),
            month_day: Some(15),
            ..Default::default()
        };
        let parts = summary(Some(Repeating::Custom), Some(&by_date), None);
        assert_eq!(
            keys(&parts),
            vec!["repeat.every_n_months", "repeat.on_day_of_month"]
        );
        assert_eq!(parts[1].count, Some(15));

        let by_weekday = CustomRepeatingPattern {
            unit: Some("months".into()),
            month_repeat_type: Some("same_weekday".into()),
            month_weekday: Some(MonthWeekday {
                weekday: "tuesday".into(),
                week_of_month: 3,
            }),
            ..Default::default()
        };
        let parts = summary(Some(Repeating::Custom), Some(&by_weekday), None);
        assert_eq!(parts[1].key, "repeat.on_nth_weekday");
        assert_eq!(parts[1].count, Some(3));
        assert_eq!(parts[1].values, vec!["tuesday"]);
    }

    #[test]
    fn a_pattern_that_ends_says_how() {
        let after = CustomRepeatingPattern {
            unit: Some("days".into()),
            end_condition: Some("after_occurrences".into()),
            end_after_occurrences: Some(10),
            ..Default::default()
        };
        let parts = summary(Some(Repeating::Custom), Some(&after), None);
        assert_eq!(parts.last().expect("a part").key, "repeat.ends_after");
        assert_eq!(parts.last().expect("a part").count, Some(10));

        let until = CustomRepeatingPattern {
            unit: Some("days".into()),
            end_condition: Some("until_date".into()),
            end_until_date: date::parse("2027-03-03T00:00:00Z"),
            ..Default::default()
        };
        let parts = summary(Some(Repeating::Custom), Some(&until), None);
        assert_eq!(parts.last().expect("a part").key, "repeat.ends_on");
        assert!(parts.last().expect("a part").date.is_some());
    }

    /// A pattern written by a client this build has not met is still a repeat. Saying nothing
    /// would read on screen as "does not repeat", which is a different task.
    #[test]
    fn an_unknown_unit_still_describes_itself() {
        let odd = CustomRepeatingPattern {
            unit: Some("fortnights".into()),
            interval: Some(3),
            ..Default::default()
        };
        let parts = summary(Some(Repeating::Custom), Some(&odd), None);
        assert_eq!(keys(&parts), vec!["repeat.every_n_days"]);
        assert_eq!(parts[0].count, Some(3));
    }

    /// A custom repeat with no pattern stored at all — an older client wrote the preset without
    /// one — still describes itself rather than coming back empty.
    #[test]
    fn a_custom_repeat_with_no_pattern_still_says_something() {
        let parts = summary(Some(Repeating::Custom), None, None);
        assert_eq!(keys(&parts), vec!["repeat.every_n_days"]);
        assert_eq!(parts[0].count, Some(1));
    }

    #[test]
    fn the_presets_lead_with_never_and_mark_the_current_one() {
        let offered = presets(Some(Repeating::Weekly));
        assert_eq!(offered[0].value, "never");
        assert_eq!(offered.len(), 6);
        let selected: Vec<&str> = offered
            .iter()
            .filter(|preset| preset.is_selected)
            .map(|preset| preset.value)
            .collect();
        assert_eq!(selected, vec!["weekly"]);
    }

    /// A task with no repeat at all shows "Never" as the current choice, not nothing.
    #[test]
    fn no_repeat_selects_never() {
        let offered = presets(None);
        assert!(offered[0].is_selected);
    }
}
