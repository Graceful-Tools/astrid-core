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
#[serde(rename_all = "camelCase")]
pub struct FilterGroup {
    /// The field on the list, as the API spells it — `filterPriority`, `sortBy`, and so on.
    pub field: &'static str,
    pub title_key: &'static str,
    pub picks: Vec<FilterPick>,
}

type Choices = &'static [(&'static str, &'static str)];

const COMPLETION: Choices = &[
    ("default", "filter.completion.recent"),
    ("hide", "filter.completion.hide"),
    ("all", "filter.completion.all"),
];

const PRIORITY: Choices = &[
    ("all", "filter.any"),
    ("3", "priority.high"),
    ("2", "priority.medium"),
    ("1", "priority.low"),
    ("0", "priority.none"),
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

const REPEATING: Choices = &[
    ("all", "filter.any"),
    ("not_repeating", "filter.repeat.never"),
    ("daily", "repeat.daily"),
    ("weekly", "repeat.weekly"),
    ("monthly", "repeat.monthly"),
    ("yearly", "repeat.yearly"),
    ("custom", "repeat.custom"),
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
