//! Finding a task.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/SearchService.swift`, with its central
//! discovery preserved and its structure simplified.
//!
//! **There is no server search endpoint.** The Apple service branches on connectivity and then
//! does the same thing either way: it matches over the tasks it already has. The branch is
//! vestigial — the "online" path reads the same cached array the "offline" one reads through Core
//! Data. So this is one path, over the cache, and it is faster and works on a train for the same
//! reason.
//!
//! That also makes the matching rules honest about what they are: a substring match, case
//! insensitive, over the title and the description. Not a ranked search, not a fuzzy one. Saying
//! so here means nobody has to read the implementation to find out that "buy milk" will not find
//! "milk, buy".

use crate::model::Task;

/// How many characters before searching is worth doing.
///
/// One character matches most of an account and tells the reader nothing; two is where the results
/// start to mean something. Matches the Apple client so a person moving between them does not have
/// to learn a different threshold.
pub const MINIMUM_QUERY_LENGTH: usize = 2;

/// What to search within.
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// Only tasks in this list, when set.
    pub list_id: Option<String>,
    /// Whether finished tasks count. On by default: "what did I call that thing I did last week?"
    /// is one of the questions search exists to answer.
    pub include_completed: bool,
}

impl SearchScope {
    pub fn everywhere() -> Self {
        SearchScope {
            list_id: None,
            include_completed: true,
        }
    }
}

/// Match `query` against `tasks`.
///
/// A query shorter than [`MINIMUM_QUERY_LENGTH`] matches nothing — not everything. Returning the
/// whole account for a single keystroke is a list that flashes its entire contents on the way to
/// the answer.
pub fn matches(tasks: &[Task], query: &str, scope: &SearchScope) -> Vec<Task> {
    let needle = query.trim().to_lowercase();
    if needle.chars().count() < MINIMUM_QUERY_LENGTH {
        return Vec::new();
    }

    let mut found: Vec<Task> = tasks
        .iter()
        .filter(|task| {
            if !scope.include_completed && task.completed {
                return false;
            }
            if let Some(list_id) = &scope.list_id {
                if !task.effective_list_ids().iter().any(|id| id == list_id) {
                    return false;
                }
            }
            task.title.to_lowercase().contains(&needle)
                || task.description.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();

    // A title match before a description match, then open before done, then most recently touched.
    // Nobody types a search expecting the completed one from March at the top.
    found.sort_by(|a, b| {
        let title_match = |task: &Task| task.title.to_lowercase().contains(&needle);
        title_match(b)
            .cmp(&title_match(a))
            .then_with(|| a.completed.cmp(&b.completed))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn task(id: &str, title: &str) -> Task {
        Task::new(id, title)
    }

    #[test]
    fn it_matches_the_title_and_the_description_without_regard_to_case() {
        let mut noted = task("t2", "Trip");
        noted.description = "book the FLIGHTS".into();
        let tasks = vec![task("t1", "Book Flights"), noted, task("t3", "Buy milk")];

        let results = matches(&tasks, "flights", &SearchScope::everywhere());
        let found: Vec<&str> = results.iter().map(|task| task.id.as_str()).collect();
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0], "t1",
            "a title match comes before a description match"
        );
    }

    /// One character matches most of an account and says nothing. Returning everything for it is a
    /// list that flashes its whole contents on the way to the answer.
    #[test]
    fn a_query_too_short_to_mean_anything_matches_nothing() {
        let tasks = vec![task("t1", "Buy milk")];
        assert!(matches(&tasks, "b", &SearchScope::everywhere()).is_empty());
        assert!(matches(&tasks, " ", &SearchScope::everywhere()).is_empty());
        assert!(matches(&tasks, "", &SearchScope::everywhere()).is_empty());
        assert_eq!(matches(&tasks, "bu", &SearchScope::everywhere()).len(), 1);
    }

    /// "What did I call that thing I did last week?" is one of the questions search exists for, so
    /// finished tasks are included — but they sort after the open ones.
    #[test]
    fn finished_tasks_are_found_but_sort_after_the_open_ones() {
        let mut done = task("done", "Buy milk");
        done.completed = true;
        let tasks = vec![done, task("open", "Buy milk again")];

        let results = matches(&tasks, "buy", &SearchScope::everywhere());
        let found: Vec<&str> = results.iter().map(|task| task.id.as_str()).collect();
        assert_eq!(found, vec!["open", "done"]);
    }

    #[test]
    fn finished_tasks_can_be_left_out() {
        let mut done = task("done", "Buy milk");
        done.completed = true;
        let scope = SearchScope {
            list_id: None,
            include_completed: false,
        };
        assert!(matches(&[done], "buy", &scope).is_empty());
    }

    #[test]
    fn a_search_can_be_confined_to_one_list() {
        let mut home = task("home", "Buy milk");
        home.list_ids = Some(vec!["l1".into()]);
        let mut work = task("work", "Buy milk for the office");
        work.list_ids = Some(vec!["l2".into()]);

        let scope = SearchScope {
            list_id: Some("l1".into()),
            include_completed: true,
        };
        let found = matches(&[home, work], "buy", &scope);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "home");
    }

    #[test]
    fn the_most_recently_touched_comes_first_among_equals() {
        let mut older = task("older", "Buy milk");
        older.updated_at = date::parse("2026-09-01T12:00:00Z");
        let mut newer = task("newer", "Buy milk");
        newer.updated_at = date::parse("2026-09-07T12:00:00Z");

        let results = matches(&[older, newer], "buy", &SearchScope::everywhere());
        let found: Vec<&str> = results.iter().map(|task| task.id.as_str()).collect();
        assert_eq!(found, vec!["newer", "older"]);
    }
}
