//! What a task row shows — decided here, drawn by the shell.
//!
//! Ports `TaskLeadingControl.swift`, `DueDateLabel.swift`, `TaskDisplayMode.swift` and the row
//! assembly the Apple views do, from `astrid-ios/Astrid App/Core/Layout/` and `Models/`.
//!
//! Rule 9 of `docs/ASTRID.md` §0 says the shell decides nothing. A row is where that rule gets
//! tested, because a row is *mostly* presentation — and the parts of it that are not are precisely
//! the parts that went wrong three times on Apple: whether the leading control is a checkbox or a
//! face, whether tapping it completes the task or opens a picker, and which day a date is.
//!
//! ## No user-facing strings
//!
//! Nothing here returns English. A due date comes back as [`DueLabel::Today`] or
//! [`DueLabel::On`], and the shell resolves it against its `.resw` resources. That is rule 10, and
//! it is also what stops the date arithmetic being retyped in XAML — which is how two platforms
//! come to disagree about which day a task is due.

pub mod assignee;
pub mod detail;
pub mod due_picks;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

use crate::filters;
use crate::model::{date, Priority, Task, TaskList, User};

/// Which task-detail design the user has chosen.
///
/// Mirrors `astrid-web/lib/task-display-mode.ts`. **Reads normalise, writes reject** — anything
/// unrecognised resolves to [`DisplayMode::List`] on the way in, because a row written before the
/// column existed reads back null and a build meeting a newer mode must still draw a usable
/// screen; but the server answers 400 to a write carrying anything but the two literals, so
/// [`DisplayMode::wire_value`] only ever emits one of them. A coerced value must never be echoed
/// back.
///
/// Not the `project_mode` feature flag, whose name is close enough to cause a real mistake: that
/// is access — may this person use boards at all — and this is a preference they set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DisplayMode {
    /// The checkbox completes the task. Priority and assignee each get their own row in detail.
    #[default]
    List,
    /// Detail is compact, and the leading control opens a popover rather than completing.
    Project,
}

impl DisplayMode {
    pub fn from_stored(stored: Option<&str>) -> Self {
        match stored.map(|value| value.trim().to_lowercase()).as_deref() {
            Some("project") => DisplayMode::Project,
            _ => DisplayMode::List,
        }
    }

    pub fn wire_value(self) -> &'static str {
        match self {
            DisplayMode::List => "list",
            DisplayMode::Project => "project",
        }
    }

    /// Whether the leading control completes the task, or opens the picker.
    pub fn checkbox_completes_task(self) -> bool {
        self == DisplayMode::List
    }

    /// Whether task detail uses the compact layout.
    pub fn uses_compact_detail(self) -> bool {
        self == DisplayMode::Project
    }
}

/// Where a leading control is being drawn.
///
/// A surface is something the call site knows and the mode never can. Asking only "which mode is
/// this?" is what handed list-row behaviour to board cards, where the click that reads as "pick
/// this one" finished the task instead, with no way back but hunting it down in the Done column
/// (tasks 9be8cb1b, f9d7ed42).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    BoardCard,
    ListRow,
    Detail,
}

/// What the control at the leading edge of a task shows (task 42013da7).
///
/// It answers "whose task is this?". It had two answers — someone else's photo, or a checkbox —
/// and unassigned was folded in with "mine", so a task nobody owns looked exactly like a task you
/// own. Nobody-assigned is its own state and gets its own mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeadingControl {
    /// Yours: the checkbox, which is also how you complete it.
    Checkbox,
    /// Someone else's — or, in project mode, yours: their photo, in a priority-coloured square.
    Avatar(String),
    /// Nobody's yet.
    Unassigned,
}

/// What clicking the leading control does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadingAction {
    Complete,
    OpenPicker,
}

impl LeadingControl {
    /// Which control to draw.
    ///
    /// `display_mode` is required rather than defaulted (task 132d7b3f). The two modes disagree
    /// about exactly one case — a task assigned to *you* — and a default would let a new call site
    /// pick the old answer silently, which is the bug the parameter exists to prevent.
    ///
    /// In **list** mode your own task is the checkbox, because there the checkbox is how you
    /// complete it. In **project** mode your own task shows your photo, exactly as someone else's
    /// shows theirs: the control opens the quick changer rather than completing, so it was never a
    /// checkbox in the "click to finish" sense, and a board where every card you own is a bare box
    /// and everyone else's is a face makes your own work the only thing you cannot see at a glance.
    pub fn for_task(
        assignee_id: Option<&str>,
        current_user_id: Option<&str>,
        display_mode: DisplayMode,
    ) -> LeadingControl {
        let Some(assignee) = assignee_id.filter(|id| !id.is_empty()) else {
            return LeadingControl::Unassigned;
        };
        if current_user_id == Some(assignee) && !display_mode.uses_compact_detail() {
            return LeadingControl::Checkbox;
        }
        LeadingControl::Avatar(assignee.to_string())
    }

    /// Complete the task, or open the picker?
    ///
    /// A **board card** always opens the picker, in both modes: a board is where a task has a
    /// status, so the control is how you set it, and completing outright from a card is the
    /// trapdoor described on [`Surface`].
    ///
    /// A **list row** completes, because that is what a checkbox means when the task is not on a
    /// board — unless project mode has turned the control into the quick changer everywhere.
    ///
    /// **Detail** adds one condition: only when the control actually *is* a checkbox. Someone
    /// else's photo is not a checkbox, and finishing their task by clicking their face is not what
    /// that click means (task 729a190e).
    pub fn action(&self, surface: Surface, display_mode: DisplayMode) -> LeadingAction {
        match surface {
            Surface::BoardCard => LeadingAction::OpenPicker,
            Surface::ListRow => {
                if display_mode.checkbox_completes_task() {
                    LeadingAction::Complete
                } else {
                    LeadingAction::OpenPicker
                }
            }
            Surface::Detail => {
                if display_mode.checkbox_completes_task() && *self == LeadingControl::Checkbox {
                    LeadingAction::Complete
                } else {
                    LeadingAction::OpenPicker
                }
            }
        }
    }
}

/// What a due-date control says, without saying it in any language.
///
/// The shell turns this into words from its resources. Splitting it here is what keeps the
/// timezone arithmetic in one place: iOS kept it as a private `formatDate` inside a date picker,
/// and porting that control to the Mac would have meant retyping seventy lines of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueLabel {
    None,
    Yesterday,
    Today,
    Tomorrow,
    /// Far enough out to need naming. The shell formats the date — with its weekday, because when
    /// scheduling, what you want to know about "12 August" is whether it is a Wednesday or a
    /// Saturday, and nothing else on the row says so.
    On {
        day: NaiveDate,
        /// The time of day, for a timed task. `None` for an all-day one.
        time: Option<chrono::NaiveTime>,
    },
}

impl DueLabel {
    /// What to show for this due date, as the reader sees it.
    pub fn for_due(
        due: Option<DateTime<Utc>>,
        is_all_day: bool,
        now: DateTime<Utc>,
        offset: FixedOffset,
    ) -> DueLabel {
        let Some(due) = due else {
            return DueLabel::None;
        };
        match day_offset(due, is_all_day, now, offset) {
            0 => DueLabel::Today,
            1 => DueLabel::Tomorrow,
            -1 => DueLabel::Yesterday,
            _ => {
                if is_all_day {
                    DueLabel::On {
                        day: date::all_day_date(due),
                        time: None,
                    }
                } else {
                    let local = due.with_timezone(&offset);
                    DueLabel::On {
                        day: local.date_naive(),
                        time: Some(local.time()),
                    }
                }
            }
        }
    }
}

/// Whole days from the reader's today to this due date.
///
/// Not private: a quick-pick row needs it to decide which option carries the tick, and comparing
/// dates by hand there is how the tick lands on the wrong row for anybody west of UTC.
///
/// An all-day date is compared as a calendar date — its UTC day against the reader's local day —
/// and a timed one as an instant in the reader's zone. See [`crate::model::date`] for why.
pub fn day_offset(
    due: DateTime<Utc>,
    is_all_day: bool,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> i64 {
    let today = filters::local_day(now, offset);
    let due_day = if is_all_day {
        date::all_day_date(due)
    } else {
        filters::local_day(due, offset)
    };
    (due_day - today).num_days()
}

/// Everything one row of a list needs, resolved once.
///
/// Assembled here rather than in the shell so the same task cannot be depicted one way in a list,
/// another in a board card and a third in quick add — which is exactly what happened before
/// `TaskLeadingControl` existed.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub completed: bool,
    pub priority: Priority,
    pub due: DueLabel,
    pub is_overdue: bool,
    pub leading: LeadingControl,
    pub action: LeadingAction,
    /// How far to indent, from the subtask chain.
    pub depth: usize,
    /// True when this row still exists only on this device.
    pub is_pending: bool,
    pub is_private: bool,
    pub is_repeating: bool,
    pub has_description: bool,
    pub comment_count: usize,
    pub attachment_count: usize,
    pub subtask_count: usize,
    /// The lists to draw as chips, in the order they were given.
    pub list_chips: Vec<ListChip>,
    /// The assignee, when the row has one to draw.
    pub assignee: Option<User>,
    /// The board column this task sits in, when it has one.
    pub status_role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListChip {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// What the row builder needs to resolve a task against.
pub struct RowContext<'a> {
    pub current_user_id: Option<&'a str>,
    pub display_mode: DisplayMode,
    pub surface: Surface,
    pub now: DateTime<Utc>,
    pub offset: FixedOffset,
    /// Every list, for the chips. Only the ones a task is in are read.
    pub lists: &'a [TaskList],
    /// Everyone known, for the assignee. A row whose assignee is not here draws an avatar with no
    /// name, which is correct — we have an id and nothing else yet.
    pub users: &'a [User],
    /// Subtask depths, from [`crate::filters::subtasks::depth_of`].
    pub depths: &'a std::collections::HashMap<String, usize>,
    /// How many children each task has, for the count badge.
    pub subtask_counts: &'a std::collections::HashMap<String, usize>,
}

impl TaskRow {
    pub fn build(task: &Task, context: &RowContext<'_>) -> TaskRow {
        let assignee_id = task
            .assignee_id
            .as_deref()
            .or(task.assignee.as_ref().map(|user| user.id.as_str()));
        let leading =
            LeadingControl::for_task(assignee_id, context.current_user_id, context.display_mode);
        let action = leading.action(context.surface, context.display_mode);

        let list_ids = task.effective_list_ids();
        let list_chips = list_ids
            .iter()
            .filter_map(|id| context.lists.iter().find(|list| &list.id == id))
            // A board column is a state, not a place. Drawing it as a chip beside "Home" and
            // "Work" tells the reader a task is filed somewhere it is not.
            .filter(|list| list.is_domain_list())
            .map(|list| ListChip {
                id: list.id.clone(),
                name: list.name.clone(),
                color: list.display_color().to_string(),
            })
            .collect();

        TaskRow {
            id: task.id.clone(),
            title: task.title.clone(),
            completed: task.completed,
            priority: task.priority,
            due: DueLabel::for_due(
                task.due_date_time,
                task.is_all_day,
                context.now,
                context.offset,
            ),
            is_overdue: filters::is_overdue(task, context.now, context.offset),
            leading,
            action,
            depth: context.depths.get(&task.id).copied().unwrap_or(0),
            is_pending: crate::model::is_temp_id(&task.id),
            is_private: task.is_private,
            is_repeating: task.is_repeating(),
            has_description: !task.description.trim().is_empty(),
            comment_count: task.comments.as_ref().map(Vec::len).unwrap_or(0),
            attachment_count: task.all_secure_files().len(),
            subtask_count: context.subtask_counts.get(&task.id).copied().unwrap_or(0),
            list_chips,
            assignee: assignee_id.and_then(|id| {
                task.assignee
                    .clone()
                    .or_else(|| context.users.iter().find(|user| user.id == id).cloned())
            }),
            status_role: task.status_role.clone(),
        }
    }

    /// Build every row for a list of tasks, in the order given.
    pub fn build_all(tasks: &[Task], context: &RowContext<'_>) -> Vec<TaskRow> {
        tasks
            .iter()
            .map(|task| TaskRow::build(task, context))
            .collect()
    }
}

/// Count each task's children, for the badge on a collapsed parent.
pub fn subtask_counts(tasks: &[Task]) -> std::collections::HashMap<String, usize> {
    let mut counts = std::collections::HashMap::new();
    for task in tasks {
        if let Some(parent) = &task.parent_task_id {
            *counts.entry(parent.clone()).or_insert(0) += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn at(instant: &str) -> DateTime<Utc> {
        date::parse(instant).expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("UTC")
    }

    fn context<'a>(
        lists: &'a [TaskList],
        users: &'a [User],
        depths: &'a HashMap<String, usize>,
        counts: &'a HashMap<String, usize>,
    ) -> RowContext<'a> {
        RowContext {
            current_user_id: Some("me"),
            display_mode: DisplayMode::List,
            surface: Surface::ListRow,
            now: at("2026-09-07T12:00:00Z"),
            offset: utc(),
            lists,
            users,
            depths,
            subtask_counts: counts,
        }
    }

    // ── Display mode ─────────────────────────────────────────────────────────────────────────

    /// Reads normalise, writes reject. A row written before the column existed reads back null,
    /// and the server answers 400 to anything but the two literals — so a coerced value must never
    /// be echoed back.
    #[test]
    fn an_unknown_display_mode_reads_as_list_and_is_never_written_back() {
        for stored in [None, Some(""), Some("  "), Some("somethingLater")] {
            assert_eq!(DisplayMode::from_stored(stored), DisplayMode::List);
        }
        assert_eq!(
            DisplayMode::from_stored(Some("PROJECT")),
            DisplayMode::Project
        );
        assert_eq!(
            DisplayMode::from_stored(Some("somethingLater")).wire_value(),
            "list"
        );
    }

    // ── The leading control ──────────────────────────────────────────────────────────────────

    /// Unassigned is its own state. Folding it in with "mine" made a task nobody owns look exactly
    /// like a task you own (task 42013da7).
    #[test]
    fn nobodys_task_looks_like_nobodys() {
        assert_eq!(
            LeadingControl::for_task(None, Some("me"), DisplayMode::List),
            LeadingControl::Unassigned
        );
        assert_eq!(
            LeadingControl::for_task(Some(""), Some("me"), DisplayMode::List),
            LeadingControl::Unassigned
        );
    }

    #[test]
    fn someone_elses_task_shows_their_face_in_either_mode() {
        for mode in [DisplayMode::List, DisplayMode::Project] {
            assert_eq!(
                LeadingControl::for_task(Some("them"), Some("me"), mode),
                LeadingControl::Avatar("them".into())
            );
        }
    }

    /// The one case the two modes disagree about, and the reason the mode is a required argument.
    #[test]
    fn your_own_task_is_a_checkbox_in_list_mode_and_your_face_in_project_mode() {
        assert_eq!(
            LeadingControl::for_task(Some("me"), Some("me"), DisplayMode::List),
            LeadingControl::Checkbox
        );
        assert_eq!(
            LeadingControl::for_task(Some("me"), Some("me"), DisplayMode::Project),
            LeadingControl::Avatar("me".into())
        );
    }

    /// The trapdoor tasks 9be8cb1b and f9d7ed42 closed: on a board, the click that reads as "pick
    /// this one" must not finish the task.
    #[test]
    fn a_board_card_never_completes_from_its_leading_control() {
        for mode in [DisplayMode::List, DisplayMode::Project] {
            assert_eq!(
                LeadingControl::Checkbox.action(Surface::BoardCard, mode),
                LeadingAction::OpenPicker
            );
        }
    }

    #[test]
    fn a_list_row_completes_unless_project_mode_has_taken_that_over() {
        assert_eq!(
            LeadingControl::Checkbox.action(Surface::ListRow, DisplayMode::List),
            LeadingAction::Complete
        );
        assert_eq!(
            LeadingControl::Checkbox.action(Surface::ListRow, DisplayMode::Project),
            LeadingAction::OpenPicker
        );
    }

    /// The asymmetry between a list row and detail, stated so it cannot be "tidied up": a list row
    /// completes whoever the task belongs to, and detail refuses unless the control really is a
    /// checkbox. Making them agree in either direction breaks one of the two behaviours the Apple
    /// tasks (729a190e, 132d7b3f) settled.
    #[test]
    fn a_list_row_completes_someone_elses_task_where_detail_would_not() {
        let theirs = LeadingControl::Avatar("them".into());
        assert_eq!(
            theirs.action(Surface::ListRow, DisplayMode::List),
            LeadingAction::Complete
        );
        assert_eq!(
            theirs.action(Surface::Detail, DisplayMode::List),
            LeadingAction::OpenPicker
        );
    }

    /// Finishing somebody else's task by clicking their face is not what that click means
    /// (task 729a190e).
    #[test]
    fn detail_completes_only_when_the_control_really_is_a_checkbox() {
        assert_eq!(
            LeadingControl::Checkbox.action(Surface::Detail, DisplayMode::List),
            LeadingAction::Complete
        );
        assert_eq!(
            LeadingControl::Avatar("them".into()).action(Surface::Detail, DisplayMode::List),
            LeadingAction::OpenPicker
        );
        assert_eq!(
            LeadingControl::Unassigned.action(Surface::Detail, DisplayMode::List),
            LeadingAction::OpenPicker
        );
    }

    // ── Due labels ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_named_days_are_named() {
        let now = at("2026-09-07T12:00:00Z");
        let day = |offset: i64| {
            Some(date::all_day_instant(
                NaiveDate::from_ymd_opt(2026, 9, 7).expect("a real day") + chrono::Days::new(0)
                    - chrono::Duration::days(-offset),
            ))
        };
        assert_eq!(DueLabel::for_due(day(0), true, now, utc()), DueLabel::Today);
        assert_eq!(
            DueLabel::for_due(day(1), true, now, utc()),
            DueLabel::Tomorrow
        );
        assert_eq!(
            DueLabel::for_due(day(-1), true, now, utc()),
            DueLabel::Yesterday
        );
        assert_eq!(DueLabel::for_due(None, true, now, utc()), DueLabel::None);
    }

    /// An all-day date is a calendar date. Read locally, a 25 December task prints as the 24th for
    /// everyone west of UTC.
    #[test]
    fn an_all_day_date_keeps_its_own_day_wherever_it_is_read() {
        let california = FixedOffset::east_opt(-7 * 3600).expect("an offset");
        let christmas =
            date::all_day_instant(NaiveDate::from_ymd_opt(2026, 12, 25).expect("a real day"));
        let label = DueLabel::for_due(
            Some(christmas),
            true,
            at("2026-12-01T12:00:00Z"),
            california,
        );
        assert_eq!(
            label,
            DueLabel::On {
                day: NaiveDate::from_ymd_opt(2026, 12, 25).expect("a real day"),
                time: None
            }
        );
    }

    /// A timed task belongs to the day the reader sees it on. 23:00 on the 7th in California is
    /// the 8th in UTC, and calling it "tomorrow" would be wrong for the person reading it.
    #[test]
    fn a_timed_date_belongs_to_the_readers_day() {
        let california = FixedOffset::east_opt(-7 * 3600).expect("an offset");
        // 20:00 on the 7th in California is 03:00 on the 8th in UTC.
        let tonight = at("2026-09-08T03:00:00Z");
        // 12:00 on the 7th in California.
        let now = at("2026-09-07T19:00:00Z");
        assert_eq!(
            DueLabel::for_due(Some(tonight), false, now, california),
            DueLabel::Today
        );
    }

    #[test]
    fn the_day_offset_is_available_for_a_quick_pick_tick() {
        let now = at("2026-09-07T12:00:00Z");
        let in_three_days =
            date::all_day_instant(NaiveDate::from_ymd_opt(2026, 9, 10).expect("a real day"));
        assert_eq!(day_offset(in_three_days, true, now, utc()), 3);
    }

    // ── The row ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_row_resolves_its_chips_assignee_and_badges() {
        let lists = vec![
            TaskList::new("l1", "Home"),
            serde_json::from_value(serde_json::json!({
                "id": "s1", "name": "Doing", "listType": "status"
            }))
            .expect("decodes"),
        ];
        let users: Vec<User> =
            vec![
                serde_json::from_value(serde_json::json!({ "id": "them", "name": "Ada" }))
                    .expect("decodes"),
            ];
        let depths = HashMap::from([("t1".to_string(), 2)]);
        let counts = HashMap::from([("t1".to_string(), 3)]);

        let mut task = Task::new("t1", "Buy milk");
        task.description = "two litres".into();
        task.assignee_id = Some("them".into());
        task.list_ids = Some(vec!["l1".into(), "s1".into()]);
        task.priority = Priority::High;

        let row = TaskRow::build(&task, &context(&lists, &users, &depths, &counts));
        assert_eq!(row.title, "Buy milk");
        assert_eq!(row.depth, 2);
        assert_eq!(row.subtask_count, 3);
        assert!(row.has_description);
        assert_eq!(row.leading, LeadingControl::Avatar("them".into()));
        // A list row completes in list mode whoever the task belongs to — the row control is the
        // checkbox affordance even when it is drawn as a face. Only DETAIL adds the condition
        // that the control must really be a checkbox.
        assert_eq!(row.action, LeadingAction::Complete);
        assert_eq!(
            row.assignee
                .expect("resolved from the known users")
                .display_name(),
            "Ada"
        );
        // A board column is a state, not a place: it is not a chip.
        assert_eq!(
            row.list_chips
                .iter()
                .map(|chip| chip.id.as_str())
                .collect::<Vec<_>>(),
            vec!["l1"]
        );
        assert_eq!(row.list_chips[0].color, "#3b82f6");
    }

    /// A row for a task that has not reached the server yet says so, so the shell can mark it.
    #[test]
    fn a_row_for_an_unsent_task_is_pending() {
        let lists = Vec::new();
        let users = Vec::new();
        let depths = HashMap::new();
        let counts = HashMap::new();
        let row = TaskRow::build(
            &Task::new("temp_abc", "Buy milk"),
            &context(&lists, &users, &depths, &counts),
        );
        assert!(row.is_pending);
    }

    /// An assignee we have only an id for still draws a control. `User::initials` has the
    /// fallback; the row does not invent a name.
    #[test]
    fn an_assignee_nobody_has_resolved_yet_still_draws_a_control() {
        let lists = Vec::new();
        let users = Vec::new();
        let depths = HashMap::new();
        let counts = HashMap::new();
        let mut task = Task::new("t1", "Buy milk");
        task.assignee_id = Some("stranger".into());

        let row = TaskRow::build(&task, &context(&lists, &users, &depths, &counts));
        assert_eq!(row.leading, LeadingControl::Avatar("stranger".into()));
        assert!(row.assignee.is_none());
    }

    #[test]
    fn subtask_counts_are_counted_once_per_parent() {
        let mut a = Task::new("a", "a");
        a.parent_task_id = Some("p".into());
        let mut b = Task::new("b", "b");
        b.parent_task_id = Some("p".into());
        let counts = subtask_counts(&[Task::new("p", "p"), a, b]);
        assert_eq!(counts.get("p"), Some(&2));
    }
}
