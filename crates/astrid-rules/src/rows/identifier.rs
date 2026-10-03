//! Where a task's identifier (`AWTD-1007`) is shown, and where it stays merely reachable.
//!
//! The server is the only minter of these ids, so every client can read `task.identifier` and every
//! client can therefore draw it — which is how Windows came to print it beside every title on every
//! list, including the personal ones where the key names a board the reader has never seen. An id
//! that means nothing to whoever is reading it is noise in the one position that matters most, at
//! the head of the title.
//!
//! The rule is astrid-web's, locked by `contracts/fixtures/task-identifiers.json` → `showRule`, and
//! it separates *shown* from *reachable*: a task keeps its id for links and search forever, so
//! copying it is always offered, while drawing it is not.

use super::Surface;

/// Does this surface draw the id?
///
/// Two conditions and a surface. The task must have an id, and it must sit on a board — see
/// [`super::detail::is_task_in_project`], which is the same membership question the board-state row
/// already asks, so a task cannot show its id in one row and deny having a column in the next.
///
/// Then the surface decides:
///
/// - **Detail** shows it, because detail is where a reader goes to find out what a thing is.
/// - **A board card** shows it muted: on a board the key is shared context, and a card is where
///   someone reads an id out to quote it in a commit or a comment.
/// - **A list row never does.** This is the whole point of the rule. A list mixes tasks from
///   everywhere, so a key drawn there is a fact about somewhere else.
///
/// Note the asymmetry with "reachable": a task moved off every board stops *showing* its id, which
/// is deliberate — the key no longer describes where the task lives — but [`offers_copy_identifier`]
/// still hands it over, because the old id goes on resolving in links and search.
pub fn shows_identifier(surface: Surface, has_identifier: bool, is_in_project: bool) -> bool {
    if !has_identifier || !is_in_project {
        return false;
    }
    match surface {
        Surface::Detail | Surface::BoardCard => true,
        Surface::ListRow => false,
    }
}

/// Is "Copy task id" worth offering?
///
/// Whenever there is an id, on every surface and regardless of where the task now lives. The id is
/// how a person hands this task to a commit message, a chat or another client, and a task that has
/// moved off its board still resolves by it — so withdrawing the command when the id stops being
/// *drawn* would take away the one action the reader came for, at exactly the moment the id is
/// hardest to find by other means.
pub fn offers_copy_identifier(has_identifier: bool) -> bool {
    has_identifier
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case the task was filed for: the id disappears from list rows and nowhere else.
    #[test]
    fn a_board_task_shows_its_id_everywhere_but_a_list_row() {
        assert!(shows_identifier(Surface::Detail, true, true));
        assert!(shows_identifier(Surface::BoardCard, true, true));
        assert!(!shows_identifier(Surface::ListRow, true, true));
    }

    /// Off every board, the id stops being drawn on every surface — including detail.
    #[test]
    fn a_task_with_no_board_draws_its_id_nowhere() {
        for surface in [Surface::Detail, Surface::BoardCard, Surface::ListRow] {
            assert!(!shows_identifier(surface, true, false));
        }
    }

    /// No id, nothing to draw and nothing to copy — the ordinary solo task.
    #[test]
    fn no_identifier_means_no_row_and_no_command() {
        assert!(!shows_identifier(Surface::Detail, false, true));
        assert!(!offers_copy_identifier(false));
    }

    /// Copying survives the move off the board that stops the drawing. This is the pair the
    /// fixture's "moved out of every project" case exists to pin.
    #[test]
    fn copying_outlives_showing() {
        assert!(!shows_identifier(Surface::Detail, true, false));
        assert!(offers_copy_identifier(true));
    }
}
