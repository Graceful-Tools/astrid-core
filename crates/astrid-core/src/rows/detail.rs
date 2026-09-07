//! What the task detail shows, and in what order.
//!
//! Ported from `astrid-ios/Astrid App/Core/Layout/TaskDetailFieldOrder.swift` (task c8a1ff51).
//!
//! The order is a product decision, stated once:
//!
//! > "Order (under task title) — Who, Date, Priority, Lists. Implement same order on all
//! > interfaces for list mode (iOS, Mac, web — board details, mobile etc)."
//!
//! It lives in the core rather than in a view because the point of it is **continuity**: the phone,
//! the Mac, the web and this window are supposed to agree. Both Apple platforms put Priority first
//! and Who second before it was written down — the same wrong order twice, which is what happens
//! when two views each decide for themselves.
//!
//! Project mode is deliberately absent. There, priority and assignee live behind the leading
//! control and are not rows at all (task 729a190e), so there is no order to state.

use crate::model::Priority;

/// A field in the task detail, named so the order can be stated once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailField {
    /// "Who".
    Assignee,
    /// "Date".
    When,
    Priority,
    Lists,
}

impl DetailField {
    /// The stable name the shell dispatches on.
    pub fn name(self) -> &'static str {
        match self {
            DetailField::Assignee => "assignee",
            DetailField::When => "when",
            DetailField::Priority => "priority",
            DetailField::Lists => "lists",
        }
    }
}

/// List mode, top to bottom, under the title.
pub const LIST_MODE: [DetailField; 4] = [
    DetailField::Assignee,
    DetailField::When,
    DetailField::Priority,
    DetailField::Lists,
];

/// The order for a display mode.
pub fn field_order(display_mode: super::DisplayMode) -> &'static [DetailField] {
    match display_mode {
        super::DisplayMode::List => &LIST_MODE,
        // Project mode puts assignee and priority behind the leading control, so the rows that
        // remain are the two that are still rows.
        super::DisplayMode::Project => &[DetailField::When, DetailField::Lists],
    }
}

/// The mark that stands for a priority.
///
/// One definition, because the `!`/`!!`/`!!!` convention was written out in half a dozen places on
/// Apple — the Mac's visuals, the list's sort keys, quick add, the model prompt — and a convention
/// spelled six times is one that drifts.
pub fn priority_glyph(priority: Priority) -> &'static str {
    match priority {
        Priority::None => "○",
        Priority::Low => "!",
        Priority::Medium => "!!",
        Priority::High => "!!!",
    }
}

/// The mark that identifies the priority row itself.
///
/// `!!!`, not a flag (task c8a1ff51). A flag says "some field about importance"; the marks are the
/// vocabulary the app already uses for priority everywhere else, so the row is recognisable
/// without reading its label.
pub const PRIORITY_ROW_ICON: &str = "!!!";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::DisplayMode;

    /// The order is the whole contract. Written as a literal so a change to it is a change
    /// somebody made on purpose, in a commit that can be mirrored to the other clients.
    #[test]
    fn list_mode_is_who_date_priority_lists() {
        let order: Vec<&str> = field_order(DisplayMode::List)
            .iter()
            .map(|field| field.name())
            .collect();
        assert_eq!(order, vec!["assignee", "when", "priority", "lists"]);
    }

    /// Project mode has no assignee or priority row: both live behind the leading control.
    #[test]
    fn project_mode_drops_the_two_fields_that_are_not_rows_there() {
        let order: Vec<&str> = field_order(DisplayMode::Project)
            .iter()
            .map(|field| field.name())
            .collect();
        assert_eq!(order, vec!["when", "lists"]);
    }

    #[test]
    fn the_priority_marks_are_the_ones_used_everywhere_else() {
        assert_eq!(priority_glyph(Priority::None), "○");
        assert_eq!(priority_glyph(Priority::Low), "!");
        assert_eq!(priority_glyph(Priority::Medium), "!!");
        assert_eq!(priority_glyph(Priority::High), "!!!");
        assert_eq!(PRIORITY_ROW_ICON, priority_glyph(Priority::High));
    }
}
