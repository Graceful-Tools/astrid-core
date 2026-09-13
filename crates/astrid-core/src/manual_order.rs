//! A hand-arranged order for a list, reconciled with what the list actually holds.
//!
//! The server owns the rule (`astrid-web/lib/list-manual-order.ts`, `sanitizeManualOrder`) and
//! applies it to every order it is sent: ids of tasks not in the list are dropped, a task named
//! twice keeps its first mention, and every task the caller did not name is appended in creation
//! order. So the stored order always describes exactly the tasks in the list, whichever client
//! sent it and however stale its view was.
//!
//! The same rule runs here first, for the offline redraw: a reorder made on a train has to be on
//! screen at once and survive a refresh, which means the cache holds the whole order, not just
//! the rows that were dragged. What the server answers with wins when it arrives (task 7883f710).

/// The stored order for `requested`, given the list's tasks in creation order.
///
/// Mirrors `sanitizeManualOrder`: unknown ids dropped, duplicates collapsed to the first mention,
/// unmentioned tasks appended in the order given.
pub fn reconcile(requested: &[String], in_list_by_creation: &[String]) -> Vec<String> {
    let mut order: Vec<String> = Vec::with_capacity(in_list_by_creation.len());
    for id in requested {
        if in_list_by_creation.contains(id) && !order.contains(id) {
            order.push(id.clone());
        }
    }
    for id in in_list_by_creation {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    order
}

/// The whole list's order after the rows on screen have been arranged.
///
/// The shell only ever shows a window — filtered, and at most a page — so the ids it hands over
/// are the arrangement of what was visible, not of the list. Everything else keeps the place it
/// had: the tasks of the existing arrangement follow in their old order, and anything never
/// arranged comes last, by creation. That is what the server would produce from the same
/// request, which is the point — the screen does not jump when its answer lands.
pub fn arranged(
    displayed: &[String],
    existing: Option<&[String]>,
    in_list_by_creation: &[String],
) -> Vec<String> {
    let mut requested: Vec<String> = displayed.to_vec();
    requested.extend(existing.unwrap_or_default().iter().cloned());
    reconcile(&requested, in_list_by_creation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    // ── reconcile: the server's rule, case for case (astrid-web tests/lib/list-manual-order) ──

    #[test]
    fn keeps_the_requested_order_when_it_names_exactly_the_list() {
        assert_eq!(
            reconcile(&ids(&["c", "a", "b"]), &ids(&["a", "b", "c"])),
            ids(&["c", "a", "b"])
        );
    }

    #[test]
    fn drops_ids_for_tasks_that_are_no_longer_in_the_list() {
        assert_eq!(
            reconcile(&ids(&["gone", "b", "a"]), &ids(&["a", "b"])),
            ids(&["b", "a"])
        );
    }

    #[test]
    fn appends_tasks_the_caller_did_not_mention_in_creation_order() {
        assert_eq!(
            reconcile(&ids(&["c"]), &ids(&["a", "b", "c"])),
            ids(&["c", "a", "b"])
        );
    }

    #[test]
    fn collapses_duplicates_to_the_first_mention() {
        assert_eq!(
            reconcile(&ids(&["b", "a", "b"]), &ids(&["a", "b"])),
            ids(&["b", "a"])
        );
    }

    #[test]
    fn falls_back_to_creation_order_for_an_empty_request() {
        assert_eq!(reconcile(&[], &ids(&["a", "b"])), ids(&["a", "b"]));
    }

    // ── arranged: what a window's drag means for the whole list (task 7883f710) ──

    /// The rows that were on screen lead, in their new order; the rest of the arrangement
    /// follows unchanged; a task never arranged comes last.
    #[test]
    fn a_dragged_window_leads_and_the_rest_keeps_its_place_task_7883f710() {
        let existing = ids(&["a", "b", "c", "d"]);
        let displayed = ids(&["c", "a"]);
        assert_eq!(
            arranged(&displayed, Some(&existing), &ids(&["a", "b", "c", "d", "e"])),
            ids(&["c", "a", "b", "d", "e"])
        );
    }

    #[test]
    fn with_nothing_arranged_yet_the_window_leads_and_creation_order_follows() {
        assert_eq!(
            arranged(&ids(&["b", "a"]), None, &ids(&["a", "b", "c"])),
            ids(&["b", "a", "c"])
        );
    }

    #[test]
    fn a_stale_arrangement_naming_a_task_that_left_does_not_carry_it() {
        let existing = ids(&["a", "left", "b"]);
        assert_eq!(
            arranged(&ids(&["b"]), Some(&existing), &ids(&["a", "b"])),
            ids(&["b", "a"])
        );
    }
}
