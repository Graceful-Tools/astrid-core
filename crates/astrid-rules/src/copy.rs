//! Every string-catalog key this core asks a client to say (task 1bdf84d7).
//!
//! Rule 10 is that nothing here returns English: a due pick comes back as `picker.tomorrow` and the
//! shell says the word. That rule has always had a hole in it, which is that **nothing said what
//! the whole set of keys was**. Four clients each kept their own catalog — web 937 keys, iOS 1,421,
//! Android 848, Windows 736, in four naming conventions — and the only shared record of the core's
//! own keys was a 13-row table in astrid-web's `docs/PRODUCT_CONTRACT.md` §2 and a hand-copied
//! array in astrid-windows' `CorePinFollowsIosTests.cs`. A key added here therefore reached a
//! client as a key drawn on screen: `repeating.daily` where the word "Daily" should be, which
//! compiles, passes every test and looks like a rendering bug.
//!
//! [`keys`] closes that. It is exported the way `command_kinds` is, it is what `strings/` is
//! generated from, and `cargo xtask check-strings` fails when the two disagree.
//!
//! ## It is COMPUTED, never a list
//!
//! Every key below is read back out of the thing that emits it — the same const tables and the same
//! functions the pickers themselves call. A second hand-written list would be a second place to
//! add a key, and the whole failure this closes is a key that exists in one place and not another.
//!
//! Three modules cannot be read that way, because their keys are chosen in `match` arms that also
//! decide what each one interpolates: [`repeat::SUMMARY_KEYS`], [`quick_add::PLACEHOLDER_KEYS`] and
//! [`reminder_picks::NONE_KEY`]. Each of those is a named constant beside the arms, with a test in
//! its own module that drives every arm and fails if the two have drifted. And
//! `tests/strings_catalog.rs` scans this crate's whole source for anything key-shaped that [`keys`]
//! does not name, which is the backstop for a module nobody thought to add here at all.
//!
//! ## What is NOT a copy key
//!
//! A dotted lower-case string is not automatically a word. `theme.mode`, `shell.hotkey` and
//! `rows.titleDisplay` are **preference keys** — the name a setting is stored under, which is a
//! wire value and must never be translated. The scan in `tests/strings_catalog.rs` carries them in
//! a short list with that reason written down, rather than leaving them to be noticed each time.

use std::collections::BTreeSet;

use crate::decisions::{RATING_KEY_PREFIX, RATING_LABEL_KEYS};
use crate::model::TaskList;
use crate::parse::quick_add::PLACEHOLDER_KEYS;
use crate::rows::{due_picks, filter_picks, list_default_picks, reminder_picks, repeat};
use crate::smart_tasks;

/// Every key this core can hand a shell, sorted and without repeats.
///
/// Sorted because it is a contract that gets diffed: a generator whose output reordered itself
/// with the hash seed would show every locale as changed on every run.
///
/// A key appearing in two places is one key. `repeating.daily` is a repeat preset and a list
/// default, `picker.today` is a due pick and a list default, and `filter.any` is the first row of
/// six different filters — the catalog holds one string for each.
pub fn keys() -> Vec<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();

    // The filter sheet. `groups` takes the list only to mark the current choice, so any list
    // yields every key; a synthetic one keeps this free of a second table of what is offered.
    for group in filter_picks::groups(&TaskList::new("", "")) {
        keys.insert(group.title_key.to_string());
        keys.extend(group.picks.iter().map(|pick| pick.title_key.to_string()));
    }

    // The due picker's quick dates and times.
    keys.extend(
        due_picks::DATE_OPTIONS
            .iter()
            .map(|option| option.title_key.to_string()),
    );
    keys.extend(
        due_picks::TIME_OPTIONS
            .iter()
            .map(|option| option.title_key.to_string()),
    );

    // The reminder offsets. Not `options()`: it drops any offset that has already passed, so the
    // keys it returns depend on the clock, and a catalog that depended on the clock would be
    // short a string for half the day.
    keys.insert(reminder_picks::NONE_KEY.to_string());
    keys.extend(
        reminder_picks::OFFSETS
            .iter()
            .map(|(key, _)| (*key).to_string()),
    );

    // The repeat picker, and the sentence the detail screen writes from a custom pattern.
    keys.extend(
        repeat::presets(None)
            .iter()
            .map(|preset| preset.title_key.to_string()),
    );
    keys.extend(repeat::SUMMARY_KEYS.iter().map(|key| (*key).to_string()));

    // A list's defaults editor: four pickers, whose hours are not keys — only a reader's clock can
    // spell an hour, so those rows carry no key at all.
    let defaults = list_default_picks::picks(list_default_picks::Chosen::default());
    for picks in [
        &defaults.priority,
        &defaults.repeating,
        &defaults.due_date,
        &defaults.due_time,
    ] {
        keys.extend(picks.iter().filter_map(|pick| pick.key.clone()));
    }

    // The settings combos, each of which builds its keys from the values it offers.
    for choices in [
        smart_tasks::offset_choices(),
        smart_tasks::time_choices(),
        smart_tasks::subtask_choices(),
        smart_tasks::layout_choices(),
    ] {
        for choice in choices {
            keys.insert(choice.title_key);
            keys.extend(choice.description_key);
        }
    }

    // The quick-add prompt.
    keys.extend(PLACEHOLDER_KEYS.iter().map(|key| (*key).to_string()));

    // The trade-off grid's rating words, whose keys are leaves of a namespace the shells share.
    keys.extend(
        RATING_LABEL_KEYS
            .iter()
            .map(|leaf| format!("{RATING_KEY_PREFIX}.{leaf}")),
    );

    keys.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::keys;

    /// The shape of the thing: sorted, unique, non-empty, and every entry a key rather than a word.
    ///
    /// The last clause is the one worth asserting. A key is lower case, dotted and has no spaces;
    /// English has spaces and capitals. A module that started returning its `label` instead of its
    /// `label_key` would otherwise put a sentence into the catalog as if it were a key, and the
    /// generated output would look plausible in every one of the four formats.
    #[test]
    fn the_keys_are_keys_1bdf84d7() {
        let keys = keys();
        assert!(keys.len() > 80, "got {} keys", keys.len());

        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(keys, sorted, "the keys should be sorted and unique");

        for key in &keys {
            assert!(
                !key.is_empty() && !key.contains(' ') && !key.ends_with('.'),
                "{key} does not look like a key"
            );
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_'),
                "{key} holds something no resource format can carry"
            );
        }

        // Every key is dotted but one. The sort group's title is the bare `sort` while its picks
        // are `sort.auto`, `sort.priority` and the rest, where every other group's title matches
        // its picks' prefix (`filter.due` over `filter.due.today`). That is the shape that makes
        // a nested catalog format impossible — see `strings/README.md` — and it is pinned here
        // rather than tidied, because astrid-windows' shipped `Strings.cs` looks it up under that
        // name and a pinned client cannot be fixed by editing this repository.
        let dotless: Vec<&String> = keys.iter().filter(|key| !key.contains('.')).collect();
        assert_eq!(dotless, vec!["sort"], "a new dotless key needs a reason");
    }

    /// The keys the clients already hand-copy are in here, by name. These are the ones a client
    /// would silently draw as a key: astrid-windows' `CorePinFollowsIosTests.cs` asserts the first
    /// two groups against its pinned core, and this is the other end of that assertion.
    #[test]
    fn the_keys_the_clients_copy_are_named_1bdf84d7() {
        let keys = keys();
        for expected in [
            "repeating.one_time_only",
            "repeating.custom",
            "filter.completion.recent",
            "filter.any",
            "lists.highest_priority",
            "sort",
            "sort.manual",
            "picker.no_due_date",
            "picker.night",
            "reminder.none",
            "reminder.week_before",
            "repeat.on_last_weekday",
            "repeat.from_due_date",
            "defaults.all_day",
            "priority.none",
            "smart.offset.none",
            "smart.layout.project_desc",
            "smart.time.17_00",
            "tasks.addTaskToList",
            "tasks.tradeoffRatings.veryGood",
        ] {
            assert!(
                keys.iter().any(|key| key == expected),
                "{expected} is emitted by this core but copy::keys() does not name it"
            );
        }
    }
}
