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
//! insensitive, over the title, the description and the assignee's name — iOS's list search
//! (`TaskListView.applySearchFilter`), followed since 2026-10-03. Not a ranked search, not a fuzzy one. Saying
//! so here means nobody has to read the implementation to find out that "buy milk" will not find
//! "milk, buy".

use chrono::{DateTime, FixedOffset, Utc};

use crate::model::{Task, TaskList, User};
use crate::parse::search::{self as query, SearchQuery};

/// How many characters before a *picker's* search is worth doing — the blocker picker's, which asks
/// the server first and so follows the server's threshold (`server_search`, CONTRACTS D18).
///
/// The search a person types into the app's search box ([`search_tasks`]) has no threshold: iOS
/// searches from the first character (AITD-459, resolved toward iOS 2026-10-03).
pub const MINIMUM_QUERY_LENGTH: usize = 2;

/// What to search within.
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// Only tasks in this list, when set.
    pub list_id: Option<String>,
    /// Whether finished tasks count: `Some(true)` all of them, `Some(false)` none. `None` is iOS's
    /// default for [`search_tasks`] — a list's default completion filter: open tasks, and tasks
    /// completed inside the recently-completed window (AITD-459). The picker paths
    /// ([`search`], [`matches`]) read `None` as all.
    pub include_completed: Option<bool>,
}

impl SearchScope {
    pub fn everywhere() -> Self {
        SearchScope {
            list_id: None,
            include_completed: Some(true),
        }
    }
}

/// The app's search box — iOS's `TaskSearch.results`, which both Apple apps drew, followed
/// exactly (AITD-459; CONTRACTS "Search", resolved toward iOS 2026-10-03):
///
/// - **The query is one phrase, as typed**: lowercased, not trimmed or split, a substring of the
///   title, the description or the assignee's name. Only an empty query finds nothing; one
///   character is a search.
/// - **Top-level tasks only**: a subtask shows under its parent, not as a result of its own.
/// - **Completed work as a list shows it by default** (`scope.include_completed == None`): open
///   tasks and those completed inside the 24-hour window.
/// - **Highest priority first** — a list's `priority` sort: open before done, then priority, then
///   due date; ties keep the cache's order.
///
/// The web's grammar is a superset kept on top: a bare identifier is a direct hit, and a query
/// with filters in it (`is:open milk`) applies them, with the rest of the query as the phrase.
pub fn search_tasks(
    tasks: &[Task],
    query_text: &str,
    scope: &SearchScope,
    context: &SearchContext<'_>,
) -> Vec<Task> {
    if query_text.is_empty() {
        return Vec::new();
    }
    let parsed = query::parse(query_text);
    if let Some(identifier) = &parsed.identifier {
        return tasks
            .iter()
            .filter(|task| {
                task.identifier
                    .as_deref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(identifier))
            })
            .cloned()
            .collect();
    }
    // A plain query is iOS's phrase exactly as typed — spaces, quotes and all. Only a query that
    // used the grammar is read through it.
    let needle = if has_filters(&parsed) {
        parsed.text.to_lowercase()
    } else {
        query_text.to_lowercase()
    };

    let mut found: Vec<Task> = tasks
        .iter()
        .filter(|task| task.parent_task_id.is_none())
        .filter(|task| in_list(task, scope))
        .filter(|task| shows_completed(task, scope, context))
        .filter(|task| structured_match(task, &parsed, context))
        .filter(|task| needle.is_empty() || text_matches(task, &needle, context.users))
        .cloned()
        .collect();
    crate::filters::sort_by_setting(&mut found, Some("priority"), None);
    found
}

fn has_filters(parsed: &SearchQuery) -> bool {
    parsed.assignee.is_some()
        || parsed.due.is_some()
        || parsed.state.is_some()
        || !parsed.list_names.is_empty()
        || !parsed.label_names.is_empty()
        || !parsed.priorities.is_empty()
        || !parsed.statuses.is_empty()
}

/// iOS's `applyCompletionFilterWithWindow(filter: "default", window: nil)` for `None`.
fn shows_completed(task: &Task, scope: &SearchScope, context: &SearchContext<'_>) -> bool {
    if !task.completed {
        return true;
    }
    match scope.include_completed {
        Some(include) => include,
        None => crate::filters::recently_completed::is_recently_completed(
            task.completed_at,
            task.updated_at,
            None,
            context.now,
            context.offset,
        ),
    }
}

fn in_list(task: &Task, scope: &SearchScope) -> bool {
    scope
        .list_id
        .as_ref()
        .is_none_or(|list_id| task.effective_list_ids().iter().any(|id| id == list_id))
}

/// What the structured half of a query is resolved against.
pub struct SearchContext<'a> {
    pub lists: &'a [TaskList],
    pub users: &'a [User],
    pub current_user_id: Option<&'a str>,
    pub now: DateTime<Utc>,
    pub offset: FixedOffset,
}

/// Match `query` against `tasks` — the text as a substring, the rest as the web's search grammar
/// (`assignee:me priority:high due:week is:open status:ready list:Work label:bug AST-142`).
///
/// A bare identifier is a direct hit and nothing else is consulted. Free text shorter than
/// [`MINIMUM_QUERY_LENGTH`] with no filter beside it matches nothing — not everything. Returning
/// the whole account for a single keystroke is a list that flashes its entire contents on the way
/// to the answer.
pub fn search(
    tasks: &[Task],
    query_text: &str,
    scope: &SearchScope,
    context: &SearchContext<'_>,
) -> Vec<Task> {
    let parsed = query::parse(query_text);
    if parsed.is_empty() {
        return Vec::new();
    }
    if let Some(identifier) = &parsed.identifier {
        return tasks
            .iter()
            .filter(|task| {
                task.identifier
                    .as_deref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(identifier))
            })
            .cloned()
            .collect();
    }
    let needle = parsed.text.to_lowercase();
    if !has_filters(&parsed) && needle.chars().count() < MINIMUM_QUERY_LENGTH {
        return Vec::new();
    }

    let mut found: Vec<Task> = tasks
        .iter()
        .filter(|task| in_scope(task, scope))
        .filter(|task| structured_match(task, &parsed, context))
        .filter(|task| needle.is_empty() || text_matches(task, &needle, context.users))
        .cloned()
        .collect();

    sort(&mut found, &needle);
    found
}

/// The free-text match: the title, the description, or the assignee's name — iOS's list search
/// (`TaskListView.applySearchFilter`). The name is the task's own `assignee`, or the cached person
/// its `assigneeId` names when the row carries only the id. Name or email, as iOS's `displayName`
/// reads, but never its "Unknown User" placeholder: nobody searching "unknown" means that.
fn text_matches(task: &Task, needle: &str, users: &[User]) -> bool {
    if task.title.to_lowercase().contains(needle)
        || task.description.to_lowercase().contains(needle)
    {
        return true;
    }
    let assignee = task.assignee.as_ref().or_else(|| {
        let id = task.assignee_id.as_deref()?;
        users.iter().find(|user| user.id == id)
    });
    assignee.is_some_and(|user| {
        user.name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .or(user.email.as_deref())
            .is_some_and(|name| name.to_lowercase().contains(needle))
    })
}

/// Match `query` against `tasks` as plain text. What [`search`] does without a context — for the
/// callers that have no lists or users to hand, and for the tests that came before the grammar.
pub fn matches(tasks: &[Task], query_text: &str, scope: &SearchScope) -> Vec<Task> {
    let needle = query_text.trim().to_lowercase();
    if needle.chars().count() < MINIMUM_QUERY_LENGTH {
        return Vec::new();
    }
    let mut found: Vec<Task> = tasks
        .iter()
        .filter(|task| in_scope(task, scope))
        .filter(|task| text_matches(task, &needle, &[]))
        .cloned()
        .collect();
    sort(&mut found, &needle);
    found
}

fn in_scope(task: &Task, scope: &SearchScope) -> bool {
    if scope.include_completed == Some(false) && task.completed {
        return false;
    }
    in_list(task, scope)
}

/// A title match before a description match, then open before done, then most recently touched.
/// Nobody types a search expecting the completed one from March at the top.
fn sort(found: &mut [Task], needle: &str) {
    found.sort_by(|a, b| {
        let title_match =
            |task: &Task| !needle.is_empty() && task.title.to_lowercase().contains(needle);
        title_match(b)
            .cmp(&title_match(a))
            .then_with(|| a.completed.cmp(&b.completed))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
}

/// The structured half: every filter present must hold.
fn structured_match(task: &Task, parsed: &SearchQuery, context: &SearchContext<'_>) -> bool {
    if let Some(assignee) = parsed.assignee.as_deref() {
        let matches = if assignee == "me" {
            task.assignee_id.as_deref() == context.current_user_id
                && context.current_user_id.is_some()
        } else {
            let handle = assignee.to_lowercase();
            task.assignee_id.as_deref().is_some_and(|id| {
                id.eq_ignore_ascii_case(&handle)
                    || context.users.iter().any(|user| {
                        user.id == id
                            && (user
                                .name
                                .as_deref()
                                .is_some_and(|n| n.to_lowercase().contains(&handle))
                                || user
                                    .email
                                    .as_deref()
                                    .is_some_and(|e| e.to_lowercase() == handle))
                    })
            })
        };
        if !matches {
            return false;
        }
    }
    if !parsed.priorities.is_empty() {
        let wanted: Vec<i64> = parsed
            .priorities
            .iter()
            .map(|word| query::priority_to_number(word))
            .collect();
        if !wanted.contains(&task.priority.as_i64()) {
            return false;
        }
    }
    if let Some(due) = parsed.due.as_deref() {
        // The web's search words map onto the list filter's values, so "due this week" means
        // the same thing here as it does in a list's filter sheet.
        let filter = match due {
            "today" => "today",
            "overdue" => "overdue",
            "week" => "this_week",
            "month" => "this_month",
            _ => "no_date",
        };
        if !crate::filters::matches_due_date(task, Some(filter), context.now, context.offset) {
            return false;
        }
    }
    if let Some(state) = parsed.state.as_deref() {
        let holds = match state {
            "open" => !task.completed,
            "done" => task.completed && task.closed_reason.is_none(),
            _ => task.closed_reason.is_some(),
        };
        if !holds {
            return false;
        }
    }
    if !parsed.statuses.is_empty() {
        let holds = parsed.statuses.iter().any(|status| match status.as_str() {
            "none" => task.status_role.is_none(),
            wanted => task
                .status_role
                .as_deref()
                .is_some_and(|role| role.eq_ignore_ascii_case(wanted)),
        });
        if !holds {
            return false;
        }
    }
    let list_ids = task.effective_list_ids();
    let in_named = |names: &[String], label: bool| {
        names.iter().all(|name| {
            list_ids.iter().any(|id| {
                context.lists.iter().any(|list| {
                    &list.id == id
                        && list.is_label_list() == label
                        && list.name.eq_ignore_ascii_case(name)
                })
            })
        })
    };
    if !in_named(&parsed.list_names, false) {
        return false;
    }
    if !in_named(&parsed.label_names, true) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{date, Priority};

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
            include_completed: Some(false),
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
            include_completed: Some(true),
        };
        let found = matches(&[home, work], "buy", &scope);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "home");
    }

    /// iOS's list search (`TaskListView.applySearchFilter`) matches the title, the description
    /// and the assignee's name. Followed here, for plain text and for the grammar's free text.
    #[test]
    fn search_matches_the_assignee_name_as_ios_does() {
        let mut held = task("held", "Water the plants");
        held.assignee_id = Some("u2".into());
        let mut priya = User::new("u2");
        priya.name = Some("Priya Raman".into());
        held.assignee = Some(priya);
        let tasks = vec![held, task("other", "Buy milk")];

        let found = matches(&tasks, "priya", &SearchScope::everywhere());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "held");

        let context = SearchContext {
            lists: &[],
            users: &[],
            current_user_id: None,
            now: date::parse("2026-09-07T12:00:00Z").expect("an instant"),
            offset: FixedOffset::east_opt(0).expect("UTC"),
        };
        let found = search(&tasks, "raman", &SearchScope::everywhere(), &context);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "held");
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

    // ── AITD-459: `search_tasks` answers as iOS's `TaskSearch.results` does ──────────────────

    fn context_at(now: &str) -> SearchContext<'static> {
        SearchContext {
            lists: &[],
            users: &[],
            current_user_id: None,
            now: date::parse(now).expect("an instant"),
            offset: FixedOffset::east_opt(0).expect("UTC"),
        }
    }

    fn ids(found: &[Task]) -> Vec<&str> {
        found.iter().map(|task| task.id.as_str()).collect()
    }

    const NOW: &str = "2026-10-03T12:00:00Z";

    /// iOS searches from the first character; only an empty query finds nothing.
    #[test]
    fn aitd459_one_character_is_a_search() {
        let tasks = vec![task("t1", "Buy milk"), task("t2", "Call Ann")];
        let context = context_at(NOW);
        let scope = SearchScope::default();
        assert_eq!(
            ids(&search_tasks(&tasks, "b", &scope, &context)),
            vec!["t1"]
        );
        assert!(search_tasks(&tasks, "", &scope, &context).is_empty());
    }

    /// The query is one phrase, as typed: not each word anywhere, and not trimmed.
    #[test]
    fn aitd459_the_query_is_one_phrase_as_typed() {
        let tasks = vec![task("1", "Buy milk and bread"), task("2", "Buy milk")];
        let context = context_at(NOW);
        let scope = SearchScope::default();
        assert!(search_tasks(&tasks, "buy bread", &scope, &context).is_empty());
        assert_eq!(
            ids(&search_tasks(&tasks, "milk and", &scope, &context)),
            vec!["1"]
        );
        assert_eq!(
            ids(&search_tasks(&tasks, "milk ", &scope, &context)),
            vec!["1"]
        );
        assert_eq!(
            ids(&search_tasks(&tasks, "MILK", &scope, &context)).len(),
            2
        );
    }

    /// Completed work is hidden as a list hides it by default: open tasks, and tasks completed
    /// inside the recently-completed window (24 hours).
    #[test]
    fn aitd459_completed_work_follows_the_default_window() {
        let mut long_done = task("long", "milk");
        long_done.completed = true;
        long_done.completed_at = date::parse("2026-09-03T12:00:00Z");
        let mut just_done = task("just", "milk");
        just_done.completed = true;
        just_done.completed_at = date::parse("2026-10-03T09:00:00Z");
        let tasks = vec![long_done, just_done, task("open", "milk")];
        let context = context_at(NOW);

        let found = search_tasks(&tasks, "milk", &SearchScope::default(), &context);
        assert_eq!(ids(&found), vec!["open", "just"]);

        let everything = SearchScope {
            include_completed: Some(true),
            ..SearchScope::default()
        };
        assert_eq!(search_tasks(&tasks, "milk", &everything, &context).len(), 3);
        let open_only = SearchScope {
            include_completed: Some(false),
            ..SearchScope::default()
        };
        assert_eq!(
            ids(&search_tasks(&tasks, "milk", &open_only, &context)),
            vec!["open"]
        );
    }

    /// Top-level tasks only: a subtask shows under its parent, not as a result of its own.
    #[test]
    fn aitd459_subtasks_are_not_results() {
        let mut sub = task("sub", "milk");
        sub.parent_task_id = Some("parent".into());
        let tasks = vec![sub, task("top", "milk")];
        let found = search_tasks(&tasks, "milk", &SearchScope::default(), &context_at(NOW));
        assert_eq!(ids(&found), vec!["top"]);
    }

    /// Highest priority first (a list's "priority" sort), open before done, then due date —
    /// not title-match-first or newest-first.
    #[test]
    fn aitd459_results_sort_by_priority() {
        let mut low = task("low", "milk");
        low.priority = Priority::Low;
        let mut high = task("high", "notes about it");
        high.description = "milk".into();
        high.priority = Priority::High;
        high.updated_at = date::parse("2026-01-01T00:00:00Z");
        let mut done = task("done", "milk");
        done.priority = Priority::High;
        done.completed = true;
        done.completed_at = date::parse("2026-10-03T11:00:00Z");
        let tasks = vec![done, low, high];
        let found = search_tasks(&tasks, "milk", &SearchScope::default(), &context_at(NOW));
        assert_eq!(ids(&found), vec!["high", "low", "done"]);
    }

    /// The grammar stays a superset: filters still apply beside the phrase.
    #[test]
    fn aitd459_the_grammar_still_filters() {
        let mut done = task("done", "milk");
        done.completed = true;
        done.completed_at = date::parse("2026-10-03T11:00:00Z");
        let tasks = vec![done, task("open", "milk")];
        let found = search_tasks(
            &tasks,
            "is:open milk",
            &SearchScope::default(),
            &context_at(NOW),
        );
        assert_eq!(ids(&found), vec!["open"]);
    }
}
