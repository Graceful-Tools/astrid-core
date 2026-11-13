//! What a list's *defaults* editor offers: the repeat, when, priority and time choices
//! (task 92830d81).
//!
//! A list carries defaults a new task inherits — `defaultRepeating`, `defaultDueDate`,
//! `defaultPriority`, `defaultDueTime` — and every client drew its own table of what may be
//! chosen for them. Windows' `ListSettingsViewModel` held four (`Repeats`, `Whens`,
//! `PriorityKeys` and an inline range of hours) under a comment saying the core had no command
//! for them; iOS and Android held their own copies. Four tables for one question is four places
//! for a fifth option to be added in three.
//!
//! The canonical source is astrid-web's `components/list-admin/DefaultTaskSettingsSection.tsx`,
//! which spells the repeats and the when options out as inline `SelectItem`s rather than as a
//! table a fixture generator could read. So this is a **core-first** contract
//! (`contracts/README.md`): the rule is canonical here from its first day, and the generator
//! writes down web's values as each case's `expect`.
//!
//! What is a rule here, and why:
//!
//! - **What is offered.** Web's five repeats, its four when options and the four priorities.
//!   Nothing is computed; the point is that one place says it.
//! - **Which hours.** A default time is a quick pick on a native client and a free-form
//!   `TimePicker` on the web, so there is no web list to follow — the working day, 06:00 to
//!   22:00, which is Windows' range and iOS's. A time already stored that is not one of them is
//!   offered too, in order, so a 09:15 set on the web is shown rather than silently rounded to
//!   the nearest hour the picker happens to carry.
//! - **What counts as selected.** Unset is not "nothing selected": web reads
//!   `defaultRepeating || "never"` and `defaultDueDate || "none"` and `defaultPriority || 0`, so
//!   a list that has never been configured shows the one-time, no-date, no-priority choices
//!   ticked. A *stored* value the picker does not offer is different, and selects nothing — a
//!   picker that quietly moved somebody's `custom` repeat to "never" would lose it on the next
//!   save.
//! - **What is not here.** The label for an hour. `06:00` reads as "6 AM" or "06:00" depending on
//!   the reader's clock, and a clock face is a locale's business rather than a rule's, so the
//!   shell formats it. Every other label is a string-catalog key, so no client writes the words.
//!
//! The assignee default is deliberately absent: it is the list's members plus two fixed
//! choices, so it is data a shell already has rather than a table it invented.

use serde::Serialize;

/// The priority choices, lowest first — the key for each of the four levels 0–3.
pub const PRIORITY_KEYS: [&str; 4] = [
    "priority.none",
    "priority.low",
    "priority.medium",
    "priority.high",
];

/// The repeat choices, as web's `SelectItem` values spell them.
pub const REPEATS: [&str; 5] = ["never", "daily", "weekly", "monthly", "yearly"];

/// The when choices: the stored value and its string-catalog key.
pub const DUE_DATES: [(&str, &str); 4] = [
    ("none", "picker.no_due_date"),
    ("today", "picker.today"),
    ("tomorrow", "picker.tomorrow"),
    ("next_week", "picker.next_week"),
];

/// The first and last hour of the quick time picks, inclusive.
pub const FIRST_HOUR: u32 = 6;
pub const LAST_HOUR: u32 = 22;

/// The key for the choice that a task inherits no time of day, only a date.
pub const ALL_DAY_KEY: &str = "defaults.all_day";

/// The key for "once", which is the one repeat whose key is not simply its value.
pub const ONE_TIME_KEY: &str = "repeating.one_time_only";

/// One row of one picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultPick {
    /// What choosing this stores on the list. `None` is a real choice, not an absence: no
    /// priority is `"0"`, but all-day is the *lack* of a time.
    pub value: Option<String>,
    /// The string-catalog key for its label, or `None` when the value is its own label — an
    /// hour, which only the reader's clock can spell.
    pub key: Option<String>,
    pub is_selected: bool,
}

impl DefaultPick {
    fn keyed(value: &str, key: &str, is_selected: bool) -> Self {
        Self {
            value: Some(value.to_string()),
            key: Some(key.to_string()),
            is_selected,
        }
    }
}

/// Every picker the defaults editor draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDefaultPicks {
    pub priority: Vec<DefaultPick>,
    pub repeating: Vec<DefaultPick>,
    pub due_date: Vec<DefaultPick>,
    pub due_time: Vec<DefaultPick>,
}

/// The list's defaults as they stand, which is what decides the ticks.
///
/// Taken as arguments rather than read from a cache so that the editor can ask about a list it
/// has changed but not yet saved, and so every answer is reproducible from the request — the
/// rules door's rule.
#[derive(Debug, Clone, Copy, Default)]
pub struct Chosen<'a> {
    pub priority: Option<i64>,
    pub repeating: Option<&'a str>,
    pub due_date: Option<&'a str>,
    /// `HH:MM`, or `None` for all-day.
    pub due_time: Option<&'a str>,
}

/// Formats an hour of the working day as the `HH:MM` a list stores.
fn hour(hour: u32) -> String {
    format!("{hour:02}:00")
}

/// The rows of all four pickers, with the list's current defaults ticked.
pub fn picks(chosen: Chosen<'_>) -> ListDefaultPicks {
    // `defaultPriority || 0`: a list nobody has configured shows "none" ticked, not a picker
    // with no answer in it.
    let priority = chosen.priority.unwrap_or(0);
    let repeating = chosen.repeating.unwrap_or("never");
    let due_date = chosen.due_date.unwrap_or("none");

    let mut hours: Vec<String> = (FIRST_HOUR..=LAST_HOUR).map(hour).collect();
    // A time stored from somewhere with a free-form picker — the web's — is offered rather than
    // rounded away. In order, so the list reads as a clock whichever hour was added.
    if let Some(stored) = chosen.due_time {
        if !hours.iter().any(|offered| offered == stored) {
            hours.push(stored.to_string());
            hours.sort();
        }
    }

    ListDefaultPicks {
        priority: PRIORITY_KEYS
            .iter()
            .enumerate()
            .map(|(level, key)| {
                DefaultPick::keyed(&level.to_string(), key, level as i64 == priority)
            })
            .collect(),
        repeating: REPEATS
            .iter()
            .map(|value| {
                let key = if *value == "never" {
                    ONE_TIME_KEY.to_string()
                } else {
                    format!("repeating.{value}")
                };
                DefaultPick::keyed(value, &key, *value == repeating)
            })
            .collect(),
        due_date: DUE_DATES
            .iter()
            .map(|(value, key)| DefaultPick::keyed(value, key, *value == due_date))
            .collect(),
        due_time: std::iter::once(DefaultPick {
            value: None,
            key: Some(ALL_DAY_KEY.to_string()),
            is_selected: chosen.due_time.is_none(),
        })
        .chain(hours.into_iter().map(|time| DefaultPick {
            is_selected: chosen.due_time == Some(time.as_str()),
            value: Some(time),
            key: None,
        }))
        .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(picks: &[DefaultPick]) -> Vec<Option<&str>> {
        picks
            .iter()
            .map(|pick| pick.value.as_deref())
            .collect::<Vec<_>>()
    }

    fn selected(picks: &[DefaultPick]) -> Option<Option<&str>> {
        picks
            .iter()
            .find(|pick| pick.is_selected)
            .map(|pick| pick.value.as_deref())
    }

    /// The four tables Windows' `ListSettingsViewModel` held are these, and they are web's
    /// (`DefaultTaskSettingsSection.tsx`) — task 92830d81.
    #[test]
    fn the_pickers_offer_what_web_offers_task_92830d81() {
        let picks = picks(Chosen::default());

        assert_eq!(
            values(&picks.priority),
            vec![Some("0"), Some("1"), Some("2"), Some("3")]
        );
        assert_eq!(
            values(&picks.repeating),
            vec![
                Some("never"),
                Some("daily"),
                Some("weekly"),
                Some("monthly"),
                Some("yearly")
            ]
        );
        assert_eq!(
            values(&picks.due_date),
            vec![
                Some("none"),
                Some("today"),
                Some("tomorrow"),
                Some("next_week")
            ]
        );
        assert_eq!(
            picks.due_time.len(),
            18,
            "all day, then 06:00 to 22:00 inclusive"
        );
        assert_eq!(values(&picks.due_time)[0], None, "all day comes first");
        assert_eq!(values(&picks.due_time)[1], Some("06:00"));
        assert_eq!(values(&picks.due_time)[17], Some("22:00"));
    }

    /// Every label but an hour's is a catalog key, so no client writes the words. "never" is the
    /// one whose key is not its value — the core calls it `repeating.one_time_only` (D49).
    #[test]
    fn every_label_is_a_key_except_an_hour_s() {
        let picks = picks(Chosen::default());

        assert!(picks
            .priority
            .iter()
            .chain(&picks.repeating)
            .chain(&picks.due_date)
            .all(|pick| pick.key.is_some()));
        assert_eq!(picks.repeating[0].key.as_deref(), Some(ONE_TIME_KEY));
        assert_eq!(picks.repeating[1].key.as_deref(), Some("repeating.daily"));
        assert_eq!(picks.due_time[0].key.as_deref(), Some(ALL_DAY_KEY));
        assert!(
            picks.due_time[1..].iter().all(|pick| pick.key.is_none()),
            "an hour reads as 6 AM or 06:00 depending on the reader, so the shell formats it"
        );
    }

    /// A list nobody has configured is not a picker with nothing ticked: web reads
    /// `defaultRepeating || "never"`, `defaultDueDate || "none"` and `defaultPriority || 0`.
    #[test]
    fn unset_defaults_tick_once_no_date_and_no_priority() {
        let picks = picks(Chosen::default());

        assert_eq!(selected(&picks.priority), Some(Some("0")));
        assert_eq!(selected(&picks.repeating), Some(Some("never")));
        assert_eq!(selected(&picks.due_date), Some(Some("none")));
        assert_eq!(selected(&picks.due_time), Some(None), "all day");
    }

    #[test]
    fn the_list_s_own_defaults_are_the_ones_ticked() {
        let picks = picks(Chosen {
            priority: Some(3),
            repeating: Some("weekly"),
            due_date: Some("tomorrow"),
            due_time: Some("09:00"),
        });

        assert_eq!(selected(&picks.priority), Some(Some("3")));
        assert_eq!(selected(&picks.repeating), Some(Some("weekly")));
        assert_eq!(selected(&picks.due_date), Some(Some("tomorrow")));
        assert_eq!(selected(&picks.due_time), Some(Some("09:00")));
    }

    /// A time set where the control is free-form — the web's `TimePicker` — is offered rather
    /// than rounded to an hour the picker happens to carry, and it lands in clock order.
    #[test]
    fn a_stored_time_that_is_not_a_quick_hour_is_offered_in_order() {
        let picks = picks(Chosen {
            due_time: Some("09:15"),
            ..Default::default()
        });

        let values = values(&picks.due_time);
        assert_eq!(values.len(), 19, "the extra time is added, not substituted");
        let at = values.iter().position(|value| *value == Some("09:15"));
        assert_eq!(
            at,
            Some(5),
            "after 09:00 and before 10:00: all day, 06, 07, 08, 09, then 09:15"
        );
        assert_eq!(selected(&picks.due_time), Some(Some("09:15")));
        assert!(
            !picks.due_time[0].is_selected,
            "a stored time is not all day"
        );
    }

    /// An hour already offered is not offered twice.
    #[test]
    fn a_stored_time_that_is_a_quick_hour_is_not_duplicated() {
        let picks = picks(Chosen {
            due_time: Some("14:00"),
            ..Default::default()
        });

        assert_eq!(picks.due_time.len(), 18);
        assert_eq!(
            picks
                .due_time
                .iter()
                .filter(|pick| pick.value.as_deref() == Some("14:00"))
                .count(),
            1
        );
    }

    /// A time outside the working day is still the person's choice, and still shown.
    #[test]
    fn a_stored_time_outside_the_working_day_is_offered_at_its_own_end() {
        let early = picks(Chosen {
            due_time: Some("05:30"),
            ..Default::default()
        });
        assert_eq!(values(&early.due_time)[1], Some("05:30"));

        let late = picks(Chosen {
            due_time: Some("23:45"),
            ..Default::default()
        });
        assert_eq!(values(&late.due_time)[18], Some("23:45"));
    }

    /// Unset and unrecognised are different. A stored value the picker cannot draw selects
    /// nothing, rather than quietly moving the person's choice to the first option — which is
    /// what would lose a `custom` repeat on the next save.
    #[test]
    fn a_stored_value_the_picker_does_not_offer_selects_nothing() {
        let picks = picks(Chosen {
            priority: Some(9),
            repeating: Some("custom"),
            due_date: Some("next_month"),
            ..Default::default()
        });

        assert_eq!(selected(&picks.priority), None);
        assert_eq!(selected(&picks.repeating), None);
        assert_eq!(selected(&picks.due_date), None);
    }

    /// Exactly one row of each picker is ticked, whatever is stored — a picker with two ticks is
    /// a control whose reader cannot tell what the list does.
    #[test]
    fn no_picker_ever_ticks_twice() {
        for chosen in [
            Chosen::default(),
            Chosen {
                priority: Some(2),
                repeating: Some("daily"),
                due_date: Some("today"),
                due_time: Some("07:00"),
            },
            Chosen {
                due_time: Some("09:15"),
                ..Default::default()
            },
        ] {
            let picks = picks(chosen);
            for group in [
                &picks.priority,
                &picks.repeating,
                &picks.due_date,
                &picks.due_time,
            ] {
                assert_eq!(
                    group.iter().filter(|pick| pick.is_selected).count(),
                    1,
                    "{chosen:?}"
                );
            }
        }
    }
}
