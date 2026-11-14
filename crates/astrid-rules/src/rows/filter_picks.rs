//! The filter and sort choices a list can be set to.
//!
//! The rules themselves are [`crate::filters`], ported and tested there. This is the other half:
//! what the sheet offers, in what order, and which choice is on — which nothing had until now,
//! because the Apple sheets build their menus inline and the values are only written down in the
//! `match` arms that read them.
//!
//! **The values are the contract.** They are saved on the list and synced to every client, so a
//! value spelled differently here would be a filter the other clients keep and this one ignores —
//! and the ignoring is silent, because every rule ends in "keep it". Each list below is the exact
//! set [`crate::filters`] matches on.
//!
//! Titles are keys. The shell says the words.

use serde::Serialize;

use crate::model::TaskList;

/// One choice, and whether the list is set to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct FilterPick {
    /// The field this choice writes. Carried on the pick as well as the group because a shell
    /// draws picks one at a time, and a radio button that does not know which group it is in is a
    /// radio button that clears the wrong one.
    pub field: &'static str,
    /// The value to write back, exactly as [`crate::filters`] matches it.
    pub value: &'static str,
    pub title_key: &'static str,
    pub is_selected: bool,
}

/// One filter: the field it writes, and the choices for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct FilterGroup {
    /// The field on the list, as the API spells it — `filterPriority`, `sortBy`, and so on.
    pub field: &'static str,
    pub title_key: &'static str,
    pub picks: Vec<FilterPick>,
}

type Choices = &'static [(&'static str, &'static str)];

/// iOS's choices, in iOS's order (`ListSortFiltersTab`, `MyTasksFilterSheet`; D22).
const COMPLETION: Choices = &[
    ("default", "filter.completion.recent"),
    ("all", "filter.completion.all"),
    ("completed", "filter.completion.completed"),
    ("incomplete", "filter.completion.incomplete"),
];

const PRIORITY: Choices = &[
    ("all", "filter.any"),
    // iOS's words and keys (`Localizable.strings`): Highest / High / Medium / Low (D27).
    ("3", "lists.highest_priority"),
    ("2", "lists.high_priority"),
    ("1", "lists.medium_priority"),
    ("0", "lists.low_priority"),
];

const DUE_DATE: Choices = &[
    ("all", "filter.any"),
    ("overdue", "filter.due.overdue"),
    ("today", "filter.due.today"),
    ("this_week", "filter.due.this_week"),
    ("this_month", "filter.due.this_month"),
    ("no_date", "filter.due.none"),
];

const ASSIGNEE: Choices = &[
    ("all", "filter.any"),
    ("current_user", "filter.assignee.me"),
    ("not_current_user", "filter.assignee.someone_else"),
    ("unassigned", "filter.assignee.nobody"),
];

/// The four cadences and "custom" carry the repeat *picker*'s keys (D49), so the same word is
/// asked for under one spelling whichever sheet is open — this group asked for `repeat.daily`
/// against the picker's `repeating.daily` until D60 was resolved (task `b0b5f3cc`).
///
/// `filter.repeat.never` stays its own key: "Not repeating" is a filter's word for the absence of
/// a repeat, not the picker's "One time only" preset.
const REPEATING: Choices = &[
    ("all", "filter.any"),
    ("not_repeating", "filter.repeat.never"),
    ("daily", "repeating.daily"),
    ("weekly", "repeating.weekly"),
    ("monthly", "repeating.monthly"),
    ("yearly", "repeating.yearly"),
    ("custom", "repeating.custom"),
];

const ASSIGNED_BY: Choices = &[
    ("all", "filter.any"),
    ("current_user", "filter.assigned_by.me"),
    ("not_current_user", "filter.assigned_by.someone_else"),
];

const IN_LISTS: Choices = &[
    ("dont_filter", "filter.any"),
    ("in_list", "filter.lists.in_a_list"),
    ("not_in_list", "filter.lists.not_in_a_list"),
    ("public_lists", "filter.lists.public"),
];

const SORT_BY: Choices = &[
    ("auto", "sort.auto"),
    ("priority", "sort.priority"),
    ("when", "sort.when"),
    ("createdAt", "sort.created"),
    ("manual", "sort.manual"),
];

fn group(
    field: &'static str,
    title_key: &'static str,
    choices: Choices,
    current: Option<&str>,
    fallback: &'static str,
) -> FilterGroup {
    // An unset filter is the list's default rather than nothing selected: a sheet with no choice
    // marked reads as broken, and "all" is what an unset filter means everywhere in `filters`.
    let current = current
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);
    FilterGroup {
        field,
        title_key,
        picks: choices
            .iter()
            .map(|(value, title_key)| FilterPick {
                field,
                value,
                title_key,
                is_selected: *value == current,
            })
            .collect(),
    }
}

/// Every filter a list carries, with the current setting marked.
pub fn groups(list: &TaskList) -> Vec<FilterGroup> {
    vec![
        group(
            "filterCompletion",
            "filter.completion",
            COMPLETION,
            list.filter_completion.as_deref(),
            "default",
        ),
        group(
            "filterPriority",
            "filter.priority",
            PRIORITY,
            list.filter_priority.as_deref(),
            "all",
        ),
        group(
            "filterDueDate",
            "filter.due",
            DUE_DATE,
            list.filter_due_date.as_deref(),
            "all",
        ),
        group(
            "filterAssignee",
            "filter.assignee",
            ASSIGNEE,
            list.filter_assignee.as_deref(),
            "all",
        ),
        group(
            "filterRepeating",
            "filter.repeat",
            REPEATING,
            list.filter_repeating.as_deref(),
            "all",
        ),
        group(
            "filterAssignedBy",
            "filter.assigned_by",
            ASSIGNED_BY,
            list.filter_assigned_by.as_deref(),
            "all",
        ),
        group(
            "filterInLists",
            "filter.lists",
            IN_LISTS,
            list.filter_in_lists.as_deref(),
            "dont_filter",
        ),
        group("sortBy", "sort", SORT_BY, list.sort_by.as_deref(), "auto"),
    ]
}

/// Whether any filter is narrowing what the list shows.
///
/// What the button says: a list quietly hiding half its tasks because of a setting somebody made
/// last month is the complaint this answers.
pub fn is_filtered(list: &TaskList) -> bool {
    let set = |value: Option<&str>, default: &str| {
        value
            .filter(|value| !value.is_empty())
            .is_some_and(|value| value != default)
    };
    set(list.filter_completion.as_deref(), "default")
        || set(list.filter_priority.as_deref(), "all")
        || set(list.filter_due_date.as_deref(), "all")
        || set(list.filter_assignee.as_deref(), "all")
        || set(list.filter_repeating.as_deref(), "all")
        || set(list.filter_assigned_by.as_deref(), "all")
        || set(list.filter_in_lists.as_deref(), "dont_filter")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> TaskList {
        TaskList::new("l1", "Work")
    }

    fn picked(groups: &[FilterGroup], field: &str) -> String {
        groups
            .iter()
            .find(|group| group.field == field)
            .expect("a group")
            .picks
            .iter()
            .filter(|pick| pick.is_selected)
            .map(|pick| pick.value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// A sheet with no choice marked reads as broken. An unset filter is the default, which is what
    /// an unset filter means everywhere in `filters`.
    #[test]
    fn an_unset_filter_shows_its_default_as_the_choice() {
        let groups = groups(&list());
        assert_eq!(picked(&groups, "filterPriority"), "all");
        assert_eq!(picked(&groups, "filterCompletion"), "default");
        assert_eq!(picked(&groups, "filterInLists"), "dont_filter");
        assert_eq!(picked(&groups, "sortBy"), "auto");
    }

    #[test]
    fn the_saved_setting_is_the_marked_one() {
        let mut list = list();
        list.filter_due_date = Some("today".into());
        list.sort_by = Some("priority".into());
        let groups = groups(&list);
        assert_eq!(picked(&groups, "filterDueDate"), "today");
        assert_eq!(picked(&groups, "sortBy"), "priority");
    }

    /// A value this build has never met leaves nothing marked rather than marking the wrong thing.
    /// The list still shows everything, because every rule in `filters` ends in "keep it".
    #[test]
    fn a_value_from_a_newer_build_marks_nothing() {
        let mut list = list();
        list.filter_due_date = Some("next_quarter".into());
        assert_eq!(picked(&groups(&list), "filterDueDate"), "");
    }

    /// The filter's four cadences and its "custom" are the picker's words, so they are the
    /// picker's keys (D49, D60).
    ///
    /// This group asked for `repeat.daily` while the picker asked for `repeating.daily` — the same
    /// five words under two spellings, depending on which sheet was open. It is asserted against
    /// [`crate::rows::repeat::presets`] rather than against a second written-out list, because a
    /// second list is what let the two drift apart in the first place.
    ///
    /// `not_repeating` is not in it: "Not repeating" is a filter's own word, not a sixth preset.
    #[test]
    fn the_repeat_filter_asks_for_the_pickers_keys_b0b5f3cc() {
        let presets = crate::rows::repeat::presets(None);
        let preset_key = |value: &str| {
            presets
                .iter()
                .find(|preset| preset.value == value)
                .unwrap_or_else(|| panic!("the picker offers {value}"))
                .title_key
                .to_string()
        };

        let repeating = groups(&list())
            .into_iter()
            .find(|group| group.field == "filterRepeating")
            .expect("a repeat group");

        for value in ["daily", "weekly", "monthly", "yearly", "custom"] {
            let pick = repeating
                .picks
                .iter()
                .find(|pick| pick.value == value)
                .unwrap_or_else(|| panic!("the filter offers {value}"));
            assert_eq!(
                pick.title_key,
                preset_key(value),
                "the repeat filter and the repeat picker must ask for one key per word"
            );
        }
    }

    /// The values are the contract: they are saved on the list and read by every client, so this
    /// pins the exact set `crate::filters` matches on.
    #[test]
    fn the_values_are_the_ones_the_rules_match_on() {
        let groups = groups(&list());
        let values = |field: &str| {
            groups
                .iter()
                .find(|group| group.field == field)
                .expect("a group")
                .picks
                .iter()
                .map(|pick| pick.value)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            values("filterDueDate"),
            vec![
                "all",
                "overdue",
                "today",
                "this_week",
                "this_month",
                "no_date"
            ]
        );
        assert_eq!(
            values("filterAssignee"),
            vec!["all", "current_user", "not_current_user", "unassigned"]
        );
        assert_eq!(
            values("filterInLists"),
            vec!["dont_filter", "in_list", "not_in_list", "public_lists"]
        );
    }

    /// D22, resolved toward iOS: the completion picker offers what iOS's list and My Tasks
    /// sheets offer, in their order (`ListSortFiltersTab`, `MyTasksFilterSheet`).
    #[test]
    fn d22_completion_picker_offers_ios_choices() {
        let groups = groups(&list());
        let completion = groups
            .iter()
            .find(|group| group.field == "filterCompletion")
            .expect("a group");
        let values: Vec<&str> = completion.picks.iter().map(|pick| pick.value).collect();
        assert_eq!(values, vec!["default", "all", "completed", "incomplete"]);
    }

    /// D27, resolved toward iOS: 3/2/1/0 read Highest / High / Medium / Low, under the keys iOS's
    /// filter sheet resolves (`lists.highest_priority` and friends in `Localizable.strings`).
    #[test]
    fn d27_priority_labels_are_ios_words() {
        let groups = groups(&list());
        let priority = groups
            .iter()
            .find(|group| group.field == "filterPriority")
            .expect("a group");
        let picks: Vec<(&str, &str)> = priority
            .picks
            .iter()
            .map(|pick| (pick.value, pick.title_key))
            .collect();
        assert_eq!(
            picks,
            vec![
                ("all", "filter.any"),
                ("3", "lists.highest_priority"),
                ("2", "lists.high_priority"),
                ("1", "lists.medium_priority"),
                ("0", "lists.low_priority"),
            ]
        );
    }

    /// What the button says: a list quietly hiding half its tasks is the complaint this answers.
    #[test]
    fn a_list_says_whether_anything_is_narrowing_it() {
        assert!(!is_filtered(&list()));

        let mut narrowed = list();
        narrowed.filter_priority = Some("3".into());
        assert!(is_filtered(&narrowed));

        // Explicitly set to the default is not narrowing anything.
        let mut plain = list();
        plain.filter_priority = Some("all".into());
        plain.filter_in_lists = Some("dont_filter".into());
        assert!(!is_filtered(&plain));

        // Sorting is not filtering: a list sorted by priority still shows everything.
        let mut sorted = list();
        sorted.sort_by = Some("priority".into());
        assert!(!is_filtered(&sorted));
    }
}
