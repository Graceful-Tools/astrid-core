//! The stateless door: the pure contracts, answered synchronously.
//!
//! `astrid_core::app::App::run_json` is the door to a running client — a cache, an Outbox, a network —
//! and it is async because most of what goes through it waits on something. Some rules wait on
//! nothing: what completing a task does, how a description renders, who may edit a list. A view
//! asks those while it draws, and a shell that has to await a future to draw a row is a shell
//! that draws it late.
//!
//! So those rules have a second door, with the same shape as the first — one function, JSON in,
//! JSON out, the same [`Response`] envelope — and no state at all. Nothing here reads a cache or a
//! clock: whatever a rule depends on ("now", the device's offset from UTC) arrives in the request,
//! which is what makes every answer reproducible from the request alone.
//!
//! Inputs are the **API wire shape** (`/api/v1`), the one every client already speaks and this
//! crate's models already decode leniently. A shell that keeps its own models hands them over as
//! it would to the server; nothing is mirrored field by field across the boundary.

use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::envelope::{Failure, Response};
use crate::model::{date, Task};

/// One question for a pure rule.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Rule {
    /// What marking `task` completed (or not) at `now` does to it — see
    /// [`crate::repeating::completion`]. `task` is the task as the person sees it.
    Completion {
        task: Box<Task>,
        completed: bool,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The person's zone, by IANA name (`America/Los_Angeles`).
        time_zone: String,
    },
    /// The calculator on its own: where a series goes next from these fields — see
    /// [`crate::repeating::next_occurrence`], which this answers exactly, all-day handling
    /// included.
    NextOccurrence {
        /// `daily`, `weekly`, `monthly`, `yearly` or `custom`, as the wire spells it.
        repeating: String,
        /// The custom pattern, or a simple pattern's end condition, as the wire stores it.
        #[serde(default)]
        pattern: Option<serde_json::Value>,
        #[cfg_attr(feature = "schema", schemars(with = "Option<date::Timestamp>"))]
        #[serde(default, with = "date::optional")]
        current_due_date: Option<DateTime<Utc>>,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        completion: DateTime<Utc>,
        /// `DUE_DATE` or `COMPLETION_DATE`; the wire default is the latter.
        #[serde(default)]
        repeat_from: Option<String>,
        #[serde(default)]
        occurrence_count: i64,
        /// The zone whose calendar the steps are taken on, by IANA name. UTC when absent — the
        /// web's own answer.
        #[serde(default)]
        time_zone: Option<String>,
        /// An all-day series steps in UTC whatever the zone: its dates are UTC midnights.
        #[serde(default)]
        is_all_day: bool,
    },
    /// Everything one person may do with one list (and with a task in it by `task_creator_id`) —
    /// see [`crate::permissions`]. `list` is the list's wire shape; nobody signed in has no access.
    ListAccess {
        list: crate::permissions::ListAccess,
        #[serde(default)]
        user_id: Option<String>,
        #[serde(default)]
        task_creator_id: Option<String>,
    },
    /// What a quick-add line says — its title, `#lists`, date word, repeat and priority — see
    /// [`crate::parse::smart`]. `today` is the person's calendar day; a date word answers as a
    /// calendar day too (`dueDay`), which a shell stores the way it stores any all-day date.
    SmartParse {
        text: String,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        /// The app's language (`en`, `pt-BR`, …): its keyword tables. English when unknown.
        #[serde(default)]
        locale: Option<String>,
        today: chrono::NaiveDate,
    },
    /// What a search box asks for — `assignee:me priority:high overdue rollover` as a structured
    /// filter plus the free text — see [`crate::parse::search`]. Answers the parse in the web's
    /// field names with `isEmpty` beside it: whether the query asks for anything at all.
    SearchParse { query: String },
    /// A description, comment or chat message as blocks to draw — see [`crate::markdown`].
    /// With `identifiers` (the reader's project keys), task ids in the prose link as well —
    /// [`crate::markdown::render_with_identifiers`]; without it nothing links, as on the web.
    /// `origin` is the reader's server, which a relative link (`/lists/…`, an id's `/t/…`)
    /// resolves against (AWTD-1630) — see [`UNNAMED_ORIGIN`] for a request that leaves it out.
    ///
    /// Gone without the `markdown` feature, which is how the browser build is made: the web has
    /// its own renderer, and this rule is for the clients that do not (task 4d05306d). A request
    /// for it then fails to decode, which is the same answer the door gives any rule it has no
    /// variant for — a build that cannot do it says so rather than answering emptily.
    #[cfg(feature = "markdown")]
    RenderMarkdown {
        text: String,
        #[serde(default)]
        identifiers: Option<Identifiers>,
        #[serde(default)]
        origin: Option<String>,
    },
    /// The words for a repeat being edited, before it is saved — the custom editor's Preview
    /// (iOS CustomRepeatingPatternEditor, AATD-50). Same parts as a saved task's `repeatSummary`.
    RepeatSummary {
        #[serde(default)]
        repeating: Option<crate::model::Repeating>,
        #[serde(default)]
        repeating_data: Option<crate::model::CustomRepeatingPattern>,
        #[serde(default)]
        repeat_from: Option<crate::model::RepeatFromMode>,
    },
    /// Whether a custom agent could be registered under the name somebody is typing — see
    /// [`crate::agent_name`]. Through this door rather than the async one because it is asked on
    /// every keystroke, and because the answer depends on nothing but the name.
    CustomAgentName { name: String },
    /// Every colour that follows from one brand accent, or why the accent cannot be used — see
    /// [`crate::brand_colors`]. Through this door because the answer depends on nothing but the
    /// accent, and because a partner deployment needs it at runtime as well as at build time.
    BrandColors { accent: String },
    /// A project's board as iOS draws it: its columns in order, each with its cards in order —
    /// see [`crate::board_cards`]. `custom_states` is the project's `customStates` as the wire
    /// carries it; `board_list_id` the list whose manual order and Done window the board follows
    /// (none: store order, the default window).
    Board {
        project_id: String,
        #[serde(default)]
        custom_states: Option<serde_json::Value>,
        #[serde(default)]
        tasks: Vec<Task>,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        #[serde(default)]
        board_list_id: Option<String>,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The person's offset from UTC now, in minutes: where their day starts for Done's window.
        #[serde(default)]
        utc_offset_minutes: i32,
    },
    /// Which lists a project's multi-list board draws as rows, in order, and which lists could be
    /// added — see [`crate::board_rows`] (AWTD-1676). `saved_list_ids` is what
    /// `GET /api/v1/projects/{id}/board-rows` answered: absent or `null` for nothing saved (the
    /// project's own lists), `[]` for a board emptied on purpose.
    BoardRows {
        project_id: String,
        #[serde(default)]
        saved_list_ids: Option<Vec<String>>,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
    },
    /// What a list's "Project" control offers: whether it may leave its project, which projects
    /// it may move into, and which projects a new list may be made in — see
    /// [`crate::list_project_moves`] (astrid-ios AITD-680, D68). `projects` is every project the
    /// person can see; `list` is absent when only asking where a list may be created.
    ListProjectMoves {
        user_id: String,
        #[serde(default)]
        list: Option<Box<crate::model::TaskList>>,
        #[serde(default)]
        projects: Vec<crate::model::project::Project>,
    },
    /// One row of the multi-list board: `row_list_id`'s cards under the project's columns, in
    /// [`Rule::Board`]'s shape — see [`crate::board_rows::row_columns`] (AWTD-1676, D67).
    /// `custom_states` is the PROJECT's; the row list's manual order and Done window arrange it.
    BoardRow {
        row_list_id: String,
        #[serde(default)]
        custom_states: Option<serde_json::Value>,
        #[serde(default)]
        tasks: Vec<Task>,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The person's offset from UTC now, in minutes: where their day starts for Done's window.
        #[serde(default)]
        utc_offset_minutes: i32,
    },
    /// What dropping `task` on the column `column` (an id from [`Rule::Board`]) writes — see
    /// [`crate::board::resolve_move`]. A drop on Done answers `completed: true`; a shell completes
    /// through [`Rule::Completion`] so a repeating task rolls forward.
    BoardMove {
        task: Box<Task>,
        column: String,
        #[serde(default)]
        custom_states: Option<serde_json::Value>,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
    },
    /// R1/R2 of [`crate::waiting`]: whether a write that may move `task` into, within or out of
    /// Waiting is allowed, and what it stores — see [`crate::waiting::resolve_status_write`].
    /// `task` and `blockers` are as they are before the write; `change` is the write. Refused with
    /// `{kind: "refused", code: "WAITING_NEEDS_CONDITION"}` when it enters Waiting with no condition.
    WaitingWrite {
        task: Box<Task>,
        #[serde(default)]
        blockers: Vec<crate::waiting::Blocker>,
        #[serde(default)]
        change: crate::waiting::StatusChange,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
    },
    /// R3: does a Waiting task leave by itself — [`crate::waiting::wake`]. Answers
    /// `{ "action": "stay" | "wakeToReady" | "resolveToInbox" }`.
    WaitingWake {
        task: Box<Task>,
        #[serde(default)]
        blockers: Vec<crate::waiting::Blocker>,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
    },
    /// R3 + R5: the lane a task belongs in now, and the write that puts it there —
    /// [`crate::waiting::settle`]. What the server's scheduled job runs per task.
    WaitingSettle {
        task: Box<Task>,
        #[serde(default)]
        blockers: Vec<crate::waiting::Blocker>,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
    },
    /// R4: what handing a task back to a person writes — [`crate::waiting::hand_back`].
    HandBack { assignee_id: String },
    /// R7: the optional preset columns this board can still add — [`crate::board::available_presets`].
    BoardPresets {
        #[serde(default)]
        custom_states: Option<serde_json::Value>,
    },
    /// Is this comment a Decision, and what does it ask — [`crate::decisions::parse`]. `null` back
    /// means it is not one, and the shell draws the markdown it already has.
    Decision { text: String },
    /// Is this table a trade-off grid, and what does each cell rate —
    /// [`crate::decisions::read_grid`]. `rows` is each body row's cell text, first column included.
    TradeoffGrid {
        #[serde(default)]
        rows: Vec<Vec<String>>,
    },
    /// The rating key a shell draws beside a grid, worst to best —
    /// [`crate::decisions::rating_key`].
    RatingKey,
    /// What a list's defaults editor offers for repeat, when, priority and time, with the list's
    /// own defaults ticked — [`crate::rows::list_default_picks`] (task 92830d81).
    ///
    /// The fields are the list's `default*` columns as the wire spells them; all four are
    /// optional, because an unconfigured list has none of them and that is itself an answer.
    ListDefaultPicks {
        #[serde(default)]
        priority: Option<i64>,
        #[serde(default)]
        repeating: Option<String>,
        #[serde(default)]
        due_date: Option<String>,
        /// `HH:MM`, or absent for all-day.
        #[serde(default)]
        due_time: Option<String>,
    },
    /// Read `AWTD-1007` into its parts, or `null` when it is not an identifier —
    /// [`crate::identifier::parse_identifier`]. iOS's `Board/TaskIdentifiers.swift` (task
    /// `75a27f7f`), which a search box calls as the person types, so it has to be synchronous.
    ///
    /// `null` is an answer, not a failure: a title is a normal thing to type into that box.
    IdentifierParse { value: String },
    /// Write an identifier out from parts somebody else holds —
    /// [`crate::identifier::format_identifier`]. The server is the only minter; this derives
    /// nothing.
    IdentifierFormat { key: String, sequence: u64 },
    /// What a dragged or indented row's drop means — [`crate::rows::nesting::outcome`]. iOS's
    /// `Filters/DragNesting.swift` and `SubtaskPromotion.swift` (task `75a27f7f`), called from a
    /// drag-hover zone while the drag is in flight.
    ///
    /// `tasks` is the rows as drawn, and must include `dragged_id` — the whole answer turns on
    /// the dragged row's own parent, so a missing one is refused rather than answered `none`.
    Nesting {
        zone: NestingZone,
        dragged_id: String,
        #[serde(default)]
        tasks: Vec<Task>,
    },
    /// Whether the caret sits inside an `@`, `#` or `!` the person is still typing, and what they
    /// have typed after it — [`crate::parse::mentions::find_trigger`]. iOS's
    /// `AutocompleteSupport.swift` (task `75a27f7f`), asked on every keystroke in the comment box.
    ///
    /// `caret` is in **UTF-16 units**, which is what a TextBox's caret and the web's `substring`
    /// both count in. `null` back means no popup — the usual answer, not a failure.
    MentionTrigger { text: String, caret: usize },
    /// What picking a row from that popup writes into the box —
    /// [`crate::parse::mentions::insert`]. The server resolves `@[Name](id)` and nothing else, so
    /// the shell asks for the text rather than assembling the reference itself.
    ///
    /// With no trigger at the caret the text comes back untouched.
    MentionInsert {
        text: String,
        caret: usize,
        /// `mention`, `list` or `task`.
        trigger_kind: crate::parse::mentions::TriggerKind,
        label: String,
        id: String,
    },
    /// Whether this reader can only copy `task` rather than edit it — [`crate::rows::copy_only`].
    /// iOS's `Tasks/TaskCapabilities.swift` (task `75a27f7f`), asked as a row draws its leading
    /// control. `lists` is the lists the task is on, as the wire carries them.
    CopyOnly {
        task: Box<Task>,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        #[serde(default)]
        user_id: Option<String>,
    },
    /// What of a comment is drawn at all — [`crate::rows::comment`]'s `shows_text`, `is_empty` and
    /// `renders_inline`. iOS's `CommentVisibility.swift` (task `75a27f7f`), asked as a bubble
    /// draws.
    ///
    /// `isSystem` too (AITD-667): [`crate::model::Comment::is_system`], but never while
    /// `offline`. A cached comment can come back without its `authorId`, so offline "no author"
    /// proves nothing, and hiding a person's own words from their own thread is the worse mistake.
    CommentVisibility {
        comment: Box<crate::model::Comment>,
        #[serde(default)]
        offline: bool,
    },
    /// The controls a task's lists let an editor offer, and what deleting it is called —
    /// [`crate::capabilities::for_lists`]. iOS's `TaskCapabilities.swift` (AITD-667), asked as an
    /// editor draws. `list_ids` is the task's lists; `lists` is every list the shell has loaded,
    /// and only those the task is on are read.
    Capabilities {
        #[serde(default)]
        list_ids: Vec<String>,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
    },
    /// Which of `titles` match `query`, best first, as indices — [`crate::palette::rank`]. The
    /// Mac's `FuzzyMatch` (AITD-667), asked on every keystroke in the ⌘K palette, which keeps its
    /// own commands; the App door's `palette` is the cache-backed whole search.
    PaletteRank {
        query: String,
        #[serde(default)]
        titles: Vec<String>,
    },
    /// What a bare key press does right now, or `null` — [`crate::keyboard::action_for`]. The
    /// Mac's `Keyboard/*` (task `75a27f7f`).
    ///
    /// Suppression is part of the answer, not the caller's job: a key pressed into a text field or
    /// under a modal is text, and an unbound key is `null` too. A shell that decided that itself
    /// would be a second copy of the guard.
    Shortcut {
        /// The canonical key, an alias, or a Windows event name (`ArrowDown`).
        key: String,
        #[serde(default)]
        has_selection: bool,
        #[serde(default)]
        is_text_field_focused: bool,
        #[serde(default)]
        is_modal_presented: bool,
    },
    /// What a new task in `list` starts as, before anything is typed —
    /// [`crate::list_defaults::apply`], the same rule `createTask` fills a draft in with. iOS's
    /// `Tasks/NewTaskDefaults.swift` (task `75a27f7f`), asked as the quick-add control draws.
    ///
    /// `list` is the list as the wire carries it, because this door has no cache to find one in.
    /// **Absent** is My Tasks and the other synthesised views: `ListService::my_tasks` is never
    /// cached, so there is no row there to read defaults from, and the creator is still the
    /// fallback rather than nobody.
    ///
    /// `priority` and `assignee_id` are what the person has already picked, and sending one is
    /// what makes that field said — a default never overrides a choice. Nothing here parses the
    /// title: a control that changed as somebody typed "urgent" would fight the choice they made
    /// in it.
    QuickAddDefaults {
        #[serde(default)]
        list: Option<Box<crate::model::TaskList>>,
        /// The signed-in user's id, the fallback assignee. Absent before anybody is signed in.
        #[serde(default)]
        user_id: Option<String>,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The person's offset from UTC now, in minutes: which calendar day "tomorrow" is.
        #[serde(default)]
        utc_offset_minutes: i32,
        #[serde(default)]
        priority: Option<i64>,
        /// A user id, or `unassigned` — the same spelling a list's own default uses.
        #[serde(default)]
        assignee_id: Option<String>,
    },
    /// Which of these tasks a reminder puts in front of somebody, in what order —
    /// [`crate::rows::triage::deck`]. iOS's `Tasks/ReminderTriage.swift` (task `75a27f7f`), asked
    /// as a notification is tapped.
    ///
    /// Answers **ids**, not rows: a row is assembled from the lists, the users and the subtask
    /// counts, none of which this door has — the App door's `reminderTriageDeck` is where a
    /// drawn row comes from. The caller already holds the tasks it sent, so ids are enough to
    /// order them.
    ///
    /// The App door also answers `assistant`. That is `users.find(…)` over the cache, a lookup
    /// rather than a rule, so it does not belong here.
    ReminderTriage {
        #[serde(default)]
        tasks: Vec<Task>,
        /// The signed-in user's id. The deck is **what is due for me**, so without one there is
        /// nobody to triage for and the answer is empty rather than everybody's.
        #[serde(default)]
        user_id: Option<String>,
        /// The task the notification fired for, if it fired for one.
        #[serde(default)]
        reminded_task_id: Option<String>,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The person's offset from UTC now, in minutes: which calendar day "today" is.
        #[serde(default)]
        utc_offset_minutes: i32,
    },
    /// Every row a list draws - [`crate::rows::assemble`]. Filter, sort, splice, window, project,
    /// in that order, as one answer (task `c2109ff8`).
    ///
    /// The order is the rule. Four contracts run in sequence and a client that ran them in its
    /// own order is four chances to show a different list from web, which is why this is one
    /// request rather than four. The tasks come in because this door holds nothing: a client that
    /// has its own cache hands over the candidates, and a real list hands over its members while
    /// a virtual one hands over everything the account holds.
    Rows {
        /// The list's saved settings - filters, sort, manual order, `showSubtasks`. Ignored when
        /// `myTasks` is set, whose settings belong to the account, and so optional: a My Tasks
        /// request has no list to send.
        #[serde(default)]
        list: Option<Box<crate::model::TaskList>>,
        /// Present when this is My Tasks, whose scope ("assigned to me") is not a list filter.
        #[serde(default)]
        my_tasks: Option<crate::filters::my_tasks::Preferences>,
        #[serde(default)]
        tasks: Vec<Task>,
        /// Every cached subtask, wherever it is filed. Also what the subtask-count badge counts,
        /// so a caller that wants counts sends them even with nothing indented.
        #[serde(default)]
        subtasks: Vec<Task>,
        /// Every list, for the chips. Only the ones a task is in are read.
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        /// Everyone the rows might name, for the assignee.
        #[serde(default)]
        users: Vec<crate::model::User>,
        #[serde(default)]
        current_user_id: Option<String>,
        /// The account's half of the subtask rule - `insideParentOnly` and the rest.
        #[serde(default)]
        subtask_display: Option<String>,
        /// `list` or `project`; anything else reads as `list`.
        #[serde(default)]
        display_mode: Option<String>,
        /// `listRow`, `boardCard` or `detail`; anything else reads as `listRow`.
        #[serde(default)]
        surface: Option<String>,
        /// Where the window starts.
        #[serde(default)]
        offset: usize,
        /// How many rows; all of them when absent.
        #[serde(default)]
        limit: Option<usize>,
        /// Ids and counts only, no rows - what a caller that already holds the tasks wants.
        #[serde(default)]
        ids_only: bool,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The person's offset from UTC now, in minutes.
        #[serde(default)]
        utc_offset_minutes: i32,
    },
    /// What a hand-arranged order becomes once the list it names has moved on -
    /// [`crate::manual_order::reconcile`]. Asked on every drop, by a client that holds the list.
    ManualOrder {
        /// The order as the person left it.
        #[serde(default)]
        requested: Vec<String>,
        /// Everything in the list now, oldest first.
        #[serde(default)]
        in_list_by_creation: Vec<String>,
    },
    /// When a snoozed reminder comes back - [`crate::reminders::snooze_until`].
    ReminderSnooze {
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        minutes: i64,
    },
    /// The whole command palette for a query - [`crate::palette::search`]: commands, lists and
    /// tasks, best first. [`Rule::PaletteRank`] is the scorer alone, for a palette that keeps its
    /// own rows.
    Palette {
        query: String,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        #[serde(default)]
        tasks: Vec<Task>,
    },
    /// Every task reference in a text that the reader could follow -
    /// [`crate::identifier::find_links`].
    IdentifierLinks {
        text: String,
        #[serde(default)]
        context: crate::identifier::LinkContext,
    },
    /// Which CLDR category a number selects - [`crate::plurals::category`]. The row a plural key's
    /// translation is read from (task `bcc8a11e`).
    ///
    /// The catalog holds the words and this picks the row, which is the split `strings/README.md`
    /// describes. A client that links Rust calls the function; one that reaches the core through
    /// a JSON door had no way to ask, so the WinUI shell could not adopt the catalog's plural keys
    /// and kept `count == 1` arms in C# instead. A rule rather than a command, deliberately: it
    /// depends on nothing but its arguments, and a command would be unreachable from
    /// `astrid-rules-wasm`, which exposes `run_json` alone.
    ///
    /// `form` is not inferable from the number. English's **cardinal** 3 is `other` ("3 weeks")
    /// while its **ordinal** 3 is `few` ("3rd") - genuinely different CLDR rules, and the arm that
    /// chose the key already knows which question it is asking.
    PluralCategory {
        /// A tag from `strings/locales.json` (`en`, `de`, `zh-CN`). The language subtag decides,
        /// so `de-DE` and `pt-BR` are answered rather than falling through to `other`.
        locale: String,
        form: crate::plurals::CountForm,
        count: i64,
    },
    /// Where a task callout opens from its row, and where its arrow points -
    /// [`crate::rows::callout::place`] (task `011080d4`).
    ///
    /// A multi-column screen -- a board, a list beside a detail pane, a tablet in landscape --
    /// has to pick a side and aim an arrow before it can draw one. Both are arithmetic, and every
    /// such client repeats them. All coordinates are logical pixels in the window's space, y
    /// increasing downward; the caller has already divided by its scale factor.
    ///
    /// The callout's SIZE is the caller's: how big a detail pane wants to be is a design decision
    /// per platform, and a rule that returned one would be choosing the layout rather than
    /// placing it.
    CalloutPlacement {
        /// The row the callout belongs to.
        anchor: crate::rows::callout::Rect,
        /// What the callout has to fit inside - the window, or the pane it is confined to.
        viewport: crate::rows::callout::Rect,
        width: f64,
        height: f64,
    },
    /// What a task's detail draws: its field order, and whether the board-state and blockers rows
    /// appear - [`crate::rows::detail`] (AWTD2-216; iOS's `TaskDetailFieldOrder`,
    /// `TaskDetailProjectStateRow.isVisible`, tasks c8a1ff51, AITD-327/332/555).
    ///
    /// Asked on its own because the Apple detail views do not read the App door's `taskDetail`.
    DetailLayout {
        /// `list` or `project`; anything else reads as `list`.
        #[serde(default)]
        display_mode: Option<String>,
        #[serde(default)]
        is_in_project: bool,
        #[serde(default)]
        is_read_only: bool,
        #[serde(default)]
        has_blockers: bool,
    },
    /// What the leading control's picker offers, and whose task this is -
    /// [`crate::rows::LeadingControl::picker_shows_project_state`] and
    /// [`crate::rows::LeadingControl::is_someone_elses_task`] (AWTD2-216; tasks AITD-375, 42013da7).
    LeadingPicker {
        /// `list` or `project`; anything else reads as `list`.
        #[serde(default)]
        display_mode: Option<String>,
        /// `listRow`, `boardCard` or `detail`; anything else reads as `listRow`.
        #[serde(default)]
        surface: Option<String>,
        #[serde(default)]
        assignee_id: Option<String>,
        #[serde(default)]
        current_user_id: Option<String>,
    },
    /// The lists a row draws as chips - [`crate::rows::list_chips`] (AWTD2-216; iOS's
    /// `chipListsForTaskRow`). `hidden_list_ids` are the lists the surrounding view already shows.
    ListChips {
        #[serde(default)]
        list_ids: Vec<String>,
        #[serde(default)]
        lists: Vec<crate::model::TaskList>,
        #[serde(default)]
        hidden_list_ids: Vec<String>,
    },
    /// Whole days from the reader's today to `date` - [`crate::rows::day_offset`] (AWTD2-216; iOS's
    /// `DueDateLabel.dayOffset`). An all-day date is compared as a calendar date, a timed one as
    /// an instant in the reader's zone.
    DayOffset {
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        date: DateTime<Utc>,
        #[serde(default)]
        is_all_day: bool,
        #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
        #[serde(with = "date::required")]
        now: DateTime<Utc>,
        /// The person's offset from UTC now, in minutes.
        #[serde(default)]
        utc_offset_minutes: i32,
    },
    /// Which look the app wears, from what was stored - [`crate::theme::Theme::parse`].
    ///
    /// A read normalises: anything a newer build wrote reads as the default rather than as an
    /// error, because a value this build has not heard of must not leave somebody with an app
    /// that will not draw.
    Theme {
        #[serde(default)]
        stored: Option<String>,
    },
}

/// Where a drop happened, as the wire spells it: `{"on": "<id>"}`, `"line"`, `"outdent"` or
/// `{"indent": ["<id>", …]}`. The shell's half of [`crate::rows::nesting::Zone`], which carries
/// no `Deserialize` of its own because nothing else sends it.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum NestingZone {
    /// On a row: become its subtask.
    On(String),
    /// On the insertion line between rows.
    Line,
    /// In the leading gutter: up one level.
    Outdent,
    /// The keyboard's indent, among these rows as drawn.
    Indent(Vec<String>),
}

impl From<NestingZone> for crate::rows::nesting::Zone {
    fn from(zone: NestingZone) -> Self {
        match zone {
            NestingZone::On(id) => Self::OnRow(id),
            NestingZone::Line => Self::Line,
            NestingZone::Outdent => Self::Outdent,
            NestingZone::Indent(rows) => Self::Indent(rows),
        }
    }
}

/// [`Rule::QuickAddDefaults`]'s answer: the fields of the draft the list's defaults just filled
/// in, which are exactly the fields `createTask` would have sent.
///
/// Every one is present, including the empty answers, so a shell reads one shape — a control that
/// had to tell "no default" from "field omitted" would be deciding something.
///
/// The App door's `quickAddDefaults` answers two more things, `leading` and `options`, and neither
/// belongs here: `options` is built from every loaded user and list, which is the cache, and
/// `leading` is `dispatch::row_json`'s wire shape rather than this crate's.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuickAddDefaultsAnswer {
    priority: i64,
    assignee_id: Option<String>,
    #[serde(with = "date::optional")]
    due_date_time: Option<DateTime<Utc>>,
    is_all_day: bool,
    repeating: Option<crate::model::Repeating>,
    is_private: bool,
}

/// [`Rule::ReminderTriage`]'s answer: the deck, in the order it is dealt, as ids into the tasks
/// the caller sent.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReminderTriageAnswer {
    task_ids: Vec<String>,
}

/// [`Rule::Nesting`]'s answer: what the drop means, and the parent it nests under when it makes
/// one. `parent_id` is always present so a shell can read one shape rather than two.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NestingAnswer {
    outcome: &'static str,
    parent_id: Option<String>,
}

impl From<crate::rows::nesting::Outcome> for NestingAnswer {
    fn from(outcome: crate::rows::nesting::Outcome) -> Self {
        Self {
            outcome: outcome.name(),
            parent_id: match outcome {
                crate::rows::nesting::Outcome::MakeSubtask { parent_id } => Some(parent_id),
                _ => None,
            },
        }
    }
}

/// What [`Rule::RenderMarkdown`] resolves a relative link against when the request names no
/// `origin`: astrid.cc, which is what every such link meant before the origin was an argument, so
/// a shell that has not started passing its server yet draws what it always drew (AWTD-1630).
///
/// Built with `require-base-url` (white-label binaries, astrid-web AWTD-1627) there is no Astrid
/// server in the binary to fall back to, and the link stays relative: the shell's own origin is the
/// only one it could mean, and a shell that wants it absolute passes it.
#[cfg(not(feature = "require-base-url"))]
pub const UNNAMED_ORIGIN: &str = "https://astrid.cc";
/// See the definition without `require-base-url`: here a relative link stays relative.
#[cfg(feature = "require-base-url")]
pub const UNNAMED_ORIGIN: &str = "";

/// What a reader can see, for [`Rule::RenderMarkdown`]: [`crate::identifier::LinkContext`] in the
/// web's names (`lib/task-identifier-links.ts`, `IdentifierLinkContext`).
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Identifiers {
    #[serde(default)]
    pub project_key: Option<String>,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub hidden: Vec<String>,
}

/// The answer to [`Rule::SearchParse`]: the parse, and whether it asks for anything.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParse {
    #[serde(flatten)]
    pub query: crate::parse::search::SearchQuery,
    pub is_empty: bool,
}

/// Answer one rule.
pub fn run(rule: Rule) -> Response {
    match rule {
        Rule::Completion {
            task,
            completed,
            now,
            time_zone,
        } => match zone(&time_zone) {
            Ok(zone) => Response::ok(crate::repeating::completion(&task, completed, now, zone)),
            Err(failure) => Response::failed(failure),
        },
        Rule::NextOccurrence {
            repeating,
            pattern,
            current_due_date,
            completion,
            repeat_from,
            occurrence_count,
            time_zone,
            is_all_day,
        } => {
            let zone = match zone(time_zone.as_deref().unwrap_or("UTC")) {
                Ok(zone) => zone,
                Err(failure) => return Response::failed(failure),
            };
            // The same fields a task carries, read the way a task is read — so this rule and
            // `Completion` cannot interpret a pattern differently.
            let task = serde_json::from_value::<Task>(serde_json::json!({
                "id": "rule",
                "repeating": repeating,
                "repeatingData": pattern,
                "dueDateTime": current_due_date.map(date::format),
                "isAllDay": is_all_day,
                "repeatFrom": repeat_from,
                "occurrenceCount": occurrence_count,
            }));
            match task {
                Ok(task) => {
                    Response::ok(crate::repeating::next_occurrence(&task, completion, zone))
                }
                Err(error) => Response::failed(Failure::bad_request(error.to_string())),
            }
        }
        Rule::ListAccess {
            list,
            user_id,
            task_creator_id,
        } => Response::ok(match user_id {
            Some(user) => crate::permissions::Access::of(&user, task_creator_id.as_deref(), &list),
            None => crate::permissions::Access::NONE,
        }),
        Rule::SmartParse {
            text,
            lists,
            locale,
            today,
        } => Response::ok(crate::parse::smart::parse(
            &text,
            &lists,
            crate::parse::smart::Keywords::for_locale(locale.as_deref().unwrap_or("en")),
            today,
        )),
        Rule::SearchParse { query } => {
            let query = crate::parse::search::parse(&query);
            let is_empty = query.is_empty();
            Response::ok(SearchParse { query, is_empty })
        }
        Rule::RepeatSummary {
            repeating,
            repeating_data,
            repeat_from,
        } => Response::ok(crate::rows::repeat::summary(
            repeating,
            repeating_data.as_ref(),
            repeat_from,
        )),
        #[cfg(feature = "markdown")]
        Rule::RenderMarkdown {
            text,
            identifiers,
            origin,
        } => {
            let origin = origin.unwrap_or_else(|| UNNAMED_ORIGIN.to_string());
            Response::ok(match identifiers {
                Some(Identifiers {
                    project_key,
                    keys,
                    hidden,
                }) => crate::markdown::render_with_identifiers(
                    &text,
                    &crate::identifier::LinkContext {
                        project_key,
                        keys,
                        hidden,
                        origin,
                    },
                ),
                None => crate::markdown::render(&text, &origin),
            })
        }
        Rule::CustomAgentName { name } => Response::ok(crate::agent_name::check(&name)),
        // A refused accent is a bad request, not an error: the caller sent a colour that cannot
        // carry a label, and the reason names the contrast so a partner can act on it.
        Rule::BrandColors { accent } => match crate::brand_colors::derive(&accent) {
            Ok(colours) => Response::ok(colours),
            Err(refusal) => Response::failed(Failure::bad_request(refusal.reason)),
        },
        Rule::Board {
            project_id,
            custom_states,
            tasks,
            lists,
            board_list_id,
            now,
            utc_offset_minutes,
        } => {
            let offset = match utc_offset(utc_offset_minutes) {
                Ok(offset) => offset,
                Err(failure) => return Response::failed(failure),
            };
            let columns = crate::board_cards::columns_with_rows(custom_states.as_ref(), &lists);
            let cards = crate::board_cards::cards(&tasks, &lists, &project_id);
            let list = crate::board_cards::BoardList::of(
                board_list_id
                    .as_deref()
                    .and_then(|id| lists.iter().find(|list| list.id == id)),
            );
            Response::ok(
                columns
                    .iter()
                    .map(|column| BoardColumnCards {
                        task_ids: crate::board_cards::column_cards(
                            &cards, column, &columns, list, now, offset,
                        )
                        .into_iter()
                        .map(|task| task.id.clone())
                        .collect(),
                        column: column.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
        }
        Rule::BoardRows {
            project_id,
            saved_list_ids,
            lists,
        } => {
            let rows = crate::board_rows::resolve(saved_list_ids.as_deref(), &project_id, &lists);
            let row_list_ids: Vec<String> = rows.iter().map(|list| list.id.clone()).collect();
            let candidate_list_ids = crate::board_rows::candidates(&lists, &row_list_ids)
                .into_iter()
                .map(|list| list.id.clone())
                .collect();
            Response::ok(BoardRowsAnswer {
                row_list_ids,
                candidate_list_ids,
            })
        }
        Rule::ListProjectMoves {
            user_id,
            list,
            projects,
        } => Response::ok(crate::list_project_moves::moves(
            &user_id,
            list.as_deref(),
            &projects,
        )),
        Rule::BoardRow {
            row_list_id,
            custom_states,
            tasks,
            lists,
            now,
            utc_offset_minutes,
        } => {
            let offset = match utc_offset(utc_offset_minutes) {
                Ok(offset) => offset,
                Err(failure) => return Response::failed(failure),
            };
            let columns = crate::board_cards::columns_with_rows(custom_states.as_ref(), &lists);
            let row = lists.iter().find(|list| list.id == row_list_id);
            let held =
                crate::board_rows::row_columns(row, &row_list_id, &tasks, &columns, now, offset);
            Response::ok(
                columns
                    .iter()
                    .zip(held)
                    .map(|(column, held)| BoardColumnCards {
                        task_ids: held.into_iter().map(|task| task.id.clone()).collect(),
                        column: column.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
        }
        Rule::BoardMove {
            task,
            column,
            custom_states,
            lists,
        } => {
            let columns = crate::board::columns(custom_states.as_ref());
            match columns.iter().find(|candidate| candidate.id == column) {
                Some(target) => Response::ok(crate::board::resolve_move(&task, target, &lists)),
                None => Response::failed(Failure::bad_request(format!(
                    "{column} is not a column of this board"
                ))),
            }
        }
        Rule::WaitingWrite {
            task,
            blockers,
            change,
            now,
        } => match crate::waiting::resolve_status_write(&task, &blockers, &change, now) {
            Ok(resolved) => Response::ok(resolved),
            Err(crate::waiting::WaitingError::WaitingNeedsCondition) => {
                Response::failed(Failure::waiting_needs_condition())
            }
        },
        Rule::WaitingWake {
            task,
            blockers,
            now,
        } => Response::ok(WakeAnswer {
            action: crate::waiting::wake(&task, &blockers, now),
        }),
        Rule::WaitingSettle {
            task,
            blockers,
            now,
        } => Response::ok(crate::waiting::settle(&task, &blockers, now)),
        Rule::HandBack { assignee_id } => Response::ok(crate::waiting::hand_back(&assignee_id)),
        Rule::BoardPresets { custom_states } => {
            Response::ok(crate::board::available_presets(custom_states.as_ref()))
        }
        Rule::Decision { text } => Response::ok(crate::decisions::parse(&text)),
        Rule::TradeoffGrid { rows } => Response::ok(crate::decisions::read_grid(&rows)),
        Rule::RatingKey => Response::ok(crate::decisions::rating_key()),
        Rule::ListDefaultPicks {
            priority,
            repeating,
            due_date,
            due_time,
        } => Response::ok(crate::rows::list_default_picks::picks(
            crate::rows::list_default_picks::Chosen {
                priority,
                repeating: repeating.as_deref(),
                due_date: due_date.as_deref(),
                due_time: due_time.as_deref(),
            },
        )),
        Rule::IdentifierParse { value } => {
            Response::ok(crate::identifier::parse_identifier(&value))
        }
        Rule::IdentifierFormat { key, sequence } => {
            Response::ok(crate::identifier::format_identifier(&key, sequence))
        }
        Rule::Nesting {
            zone,
            dragged_id,
            tasks,
        } => {
            let by_id: std::collections::HashMap<String, &Task> =
                tasks.iter().map(|task| (task.id.clone(), task)).collect();
            // The outcome turns on the dragged row's own parent, so answering `none` for a row
            // the shell forgot to send would read as a refused drop and swallow it silently.
            let Some(dragged) = by_id.get(&dragged_id) else {
                return Response::failed(Failure::bad_request(format!(
                    "{dragged_id} is not among the rows sent"
                )));
            };
            Response::ok(NestingAnswer::from(crate::rows::nesting::outcome(
                &zone.into(),
                dragged,
                &by_id,
            )))
        }
        Rule::MentionTrigger { text, caret } => {
            Response::ok(crate::parse::mentions::find_trigger(&text, caret))
        }
        Rule::MentionInsert {
            text,
            caret,
            trigger_kind,
            label,
            id,
        } => {
            let (text, caret) =
                crate::parse::mentions::insert(&text, caret, trigger_kind, &label, &id);
            Response::ok(MentionInsertAnswer { text, caret })
        }
        Rule::QuickAddDefaults {
            list,
            user_id,
            now,
            utc_offset_minutes,
            priority,
            assignee_id,
        } => {
            let Some(zone) = utc_offset_minutes
                .checked_mul(60)
                .and_then(chrono::FixedOffset::east_opt)
            else {
                return Response::failed(Failure::bad_request(format!(
                    "{utc_offset_minutes} minutes is not an offset from UTC"
                )));
            };

            // The draft `createTask` would make, read back — rather than a second copy of the same
            // rules. The control is a PREVIEW of the task about to exist, so if it said one thing
            // and the created task came out another, it would be lying about its own effect.
            let mut draft = crate::model::TaskDraft::new("");
            let given = crate::list_defaults::Given {
                assignee: assignee_id.is_some(),
                priority: priority.is_some(),
                ..Default::default()
            };
            if let Some(assignee) = assignee_id.as_deref() {
                draft.assignee_id = match assignee {
                    "unassigned" | "" => None,
                    id => Some(id.to_string()),
                };
            }
            if let Some(priority) = priority {
                draft.priority = crate::model::Priority::from_i64(priority);
            }

            match list {
                Some(list) => {
                    draft.list_ids = vec![list.id.clone()];
                    crate::list_defaults::apply(
                        &mut draft,
                        given,
                        &list,
                        user_id.as_deref(),
                        now,
                        zone,
                    );
                }
                // A synthesised view — My Tasks — has no defaults to apply, and the creator is
                // still the fallback there rather than nobody.
                None if !given.assignee => draft.assignee_id = user_id,
                None => {}
            }

            Response::ok(QuickAddDefaultsAnswer {
                priority: draft.priority.as_i64(),
                assignee_id: draft.assignee_id,
                due_date_time: draft.due_date_time,
                is_all_day: draft.is_all_day,
                repeating: draft.repeating,
                is_private: draft.is_private,
            })
        }
        Rule::ReminderTriage {
            tasks,
            user_id,
            reminded_task_id,
            now,
            utc_offset_minutes,
        } => {
            let Some(zone) = utc_offset_minutes
                .checked_mul(60)
                .and_then(chrono::FixedOffset::east_opt)
            else {
                return Response::failed(Failure::bad_request(format!(
                    "{utc_offset_minutes} minutes is not an offset from UTC"
                )));
            };
            // Nobody signed in is an empty deck, not everybody's: the question is what is due for
            // ME, and there is no me.
            let deck = match user_id.as_deref() {
                Some(me) => {
                    crate::rows::triage::deck(&tasks, me, reminded_task_id.as_deref(), now, zone)
                }
                None => Vec::new(),
            };
            Response::ok(ReminderTriageAnswer {
                task_ids: deck.into_iter().map(|task| task.id.clone()).collect(),
            })
        }
        Rule::CopyOnly {
            task,
            lists,
            user_id,
        } => Response::ok(crate::rows::copy_only(&task, &lists, user_id.as_deref())),
        Rule::CommentVisibility { comment, offline } => Response::ok(CommentVisibilityAnswer {
            is_system: !offline && comment.is_system(),
            shows_text: crate::rows::comment::shows_text(&comment.content),
            is_empty: crate::rows::comment::is_empty(&comment),
            files: crate::rows::comment::files_of(&comment)
                .into_iter()
                .map(|file| VisibleFile {
                    id: file.id.clone(),
                    name: file.name.clone(),
                    mime_type: file.mime_type.clone(),
                    renders_inline: crate::rows::comment::renders_inline(&file.mime_type),
                })
                .collect(),
        }),
        Rule::Capabilities { list_ids, lists } => {
            Response::ok(crate::capabilities::for_lists(&list_ids, &lists))
        }
        Rule::PaletteRank { query, titles } => Response::ok(crate::palette::rank(&query, &titles)),
        Rule::DetailLayout {
            display_mode,
            is_in_project,
            is_read_only,
            has_blockers,
        } => {
            let mode = crate::rows::DisplayMode::from_stored(display_mode.as_deref());
            Response::ok(DetailLayoutAnswer {
                field_order: crate::rows::detail::field_order(mode)
                    .iter()
                    .map(|field| field.name())
                    .collect(),
                shows_board_state: crate::rows::detail::shows_board_state(
                    mode,
                    is_in_project,
                    is_read_only,
                ),
                shows_task_blockers: crate::rows::detail::shows_task_blockers(
                    is_in_project,
                    is_read_only,
                    has_blockers,
                ),
            })
        }
        Rule::LeadingPicker {
            display_mode,
            surface,
            assignee_id,
            current_user_id,
        } => {
            let is_someone_elses = crate::rows::LeadingControl::is_someone_elses_task(
                assignee_id.as_deref(),
                current_user_id.as_deref(),
            );
            Response::ok(LeadingPickerAnswer {
                picker_shows_project_state: crate::rows::LeadingControl::picker_shows_project_state(
                    crate::rows::DisplayMode::from_stored(display_mode.as_deref()),
                    crate::rows::Surface::from_wire(surface.as_deref()),
                    is_someone_elses,
                ),
                is_someone_elses_task: is_someone_elses,
            })
        }
        Rule::ListChips {
            list_ids,
            lists,
            hidden_list_ids,
        } => Response::ok(crate::rows::json::chips(&crate::rows::list_chips(
            &list_ids,
            &lists,
            &hidden_list_ids,
        ))),
        Rule::DayOffset {
            date,
            is_all_day,
            now,
            utc_offset_minutes,
        } => match utc_offset(utc_offset_minutes) {
            Ok(offset) => Response::ok(DayOffsetAnswer {
                days: crate::rows::day_offset(date, is_all_day, now, offset),
            }),
            Err(failure) => Response::failed(failure),
        },
        Rule::Rows {
            list,
            my_tasks,
            tasks,
            subtasks,
            lists,
            users,
            current_user_id,
            subtask_display,
            display_mode,
            surface,
            offset,
            limit,
            ids_only,
            now,
            utc_offset_minutes,
        } => {
            let Some(utc_offset) = utc_offset_minutes
                .checked_mul(60)
                .and_then(FixedOffset::east_opt)
            else {
                return Response::failed(Failure::bad_request(format!(
                    "{utc_offset_minutes} minutes is not an offset from UTC"
                )));
            };
            // A request with neither a list nor preferences is a list with no saved settings:
            // every filter unset, sorted auto. Refusing it would make the commonest call - "lay
            // these tasks out for me" - need a stanza of empty fields.
            let unset = crate::model::TaskList::new("", "");
            let assembled = crate::rows::assemble::assemble(&crate::rows::assemble::Inputs {
                list: list.as_deref().unwrap_or(&unset),
                my_tasks: my_tasks.as_ref(),
                tasks: &tasks,
                subtasks: &subtasks,
                lists: &lists,
                users: &users,
                current_user_id: current_user_id.as_deref(),
                subtask_display: subtask_display.as_deref(),
                display_mode: crate::rows::DisplayMode::from_stored(display_mode.as_deref()),
                surface: crate::rows::Surface::from_wire(surface.as_deref()),
                now,
                offset: utc_offset,
                window_offset: offset,
                limit,
                ids_only,
            });
            Response::ok(crate::rows::json::answer(&assembled, ids_only))
        }
        Rule::ManualOrder {
            requested,
            in_list_by_creation,
        } => Response::ok(crate::manual_order::reconcile(
            &requested,
            &in_list_by_creation,
        )),
        Rule::ReminderSnooze { now, minutes } => {
            Response::ok(date::format(crate::reminders::snooze_until(now, minutes)))
        }
        Rule::Palette {
            query,
            lists,
            tasks,
        } => Response::ok(crate::palette::search(&query, &lists, &tasks)),
        Rule::IdentifierLinks { text, context } => {
            Response::ok(crate::identifier::find_links(&text, &context))
        }
        Rule::PluralCategory {
            locale,
            form,
            count,
        } => Response::ok(crate::plurals::category(&locale, form, count)),
        Rule::CalloutPlacement {
            anchor,
            viewport,
            width,
            height,
        } => Response::ok(crate::rows::callout::place(anchor, viewport, width, height)),
        Rule::Theme { stored } => {
            Response::ok(crate::theme::Theme::parse(stored.as_deref().unwrap_or_default()).wire())
        }
        Rule::Shortcut {
            key,
            has_selection,
            is_text_field_focused,
            is_modal_presented,
        } => {
            let context = crate::keyboard::Context {
                has_selection,
                is_text_field_focused,
                is_modal_presented,
            };
            // `action_for` already applies the guards, so the binding is looked up again only to
            // name it — and only once the guards have let the press through.
            Response::ok(
                crate::keyboard::action_for(&key, context).and_then(|action| {
                    crate::keyboard::binding_for(&key).map(|binding| ShortcutAnswer {
                        action: crate::keyboard::action_name(action),
                        title: binding.title,
                        requires_selection: binding.requires_selection,
                    })
                }),
            )
        }
    }
}

/// [`Rule::MentionInsert`]'s answer: the new text, and where the caret lands in it (UTF-16).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionInsertAnswer {
    text: String,
    caret: usize,
}

/// [`Rule::CommentVisibility`]'s answer: whether a text bubble is drawn, whether there is
/// anything to draw at all, and how each file is drawn.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommentVisibilityAnswer {
    is_system: bool,
    shows_text: bool,
    is_empty: bool,
    files: Vec<VisibleFile>,
}

/// One file of [`Rule::CommentVisibility`]'s answer. Deliberately not
/// [`crate::rows::comment::FileRow`]: that carries a size and a `localPath` this question has no
/// opinion about, and a shell drawing a bubble should not have to ignore two fields to read one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisibleFile {
    id: String,
    name: String,
    mime_type: String,
    renders_inline: bool,
}

/// [`Rule::Shortcut`]'s answer, when a key does something.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShortcutAnswer {
    action: &'static str,
    title: &'static str,
    requires_selection: bool,
}

/// [`Rule::DetailLayout`]'s answer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DetailLayoutAnswer {
    /// Field names (`assignee`, `when`, `priority`, `lists`), top to bottom under the title.
    field_order: Vec<&'static str>,
    shows_board_state: bool,
    shows_task_blockers: bool,
}

/// [`Rule::LeadingPicker`]'s answer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LeadingPickerAnswer {
    picker_shows_project_state: bool,
    is_someone_elses_task: bool,
}

/// [`Rule::DayOffset`]'s answer.
#[derive(Debug, Serialize)]
struct DayOffsetAnswer {
    days: i64,
}

/// [`Rule::WaitingWake`]'s answer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WakeAnswer {
    action: crate::waiting::Wake,
}

/// One column of [`Rule::Board`]'s answer: the column, and its cards' ids in order.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoardColumnCards {
    #[serde(flatten)]
    column: crate::board::BoardColumn,
    task_ids: Vec<String>,
}

/// What [`Rule::BoardRows`] answers: the rows to draw, in order, and the lists *Add list* offers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoardRowsAnswer {
    row_list_ids: Vec<String>,
    candidate_list_ids: Vec<String>,
}

/// An offset from UTC in minutes, or a refusal: an offset no clock has is a bad request, not an
/// overflow.
fn utc_offset(minutes: i32) -> Result<FixedOffset, Failure> {
    minutes
        .checked_mul(60)
        .and_then(FixedOffset::east_opt)
        .ok_or_else(|| Failure::bad_request(format!("{minutes} minutes is not an offset from UTC")))
}

/// A zone by its IANA name. An unknown name is refused rather than read as UTC: a person in Tokyo
/// told their Tuesday task is due on Monday is a worse failure than an error in a log.
fn zone(name: &str) -> Result<Tz, Failure> {
    name.parse::<Tz>()
        .map_err(|_| Failure::bad_request(format!("{name} is not a time zone this build knows")))
}

/// Answer one rule given as JSON, as JSON. What a binding calls.
pub fn run_json(request: &str) -> String {
    match serde_json::from_str::<Rule>(request) {
        Ok(rule) => run(rule).to_json(),
        // A request the core cannot read is a bug in the shell, and it has to say so rather than
        // answer with a default that looks like a result.
        Err(error) => Response::failed(Failure::bad_request(error.to_string())).to_json(),
    }
}

#[cfg(test)]
mod tests {

    /// The Astrid Code project board asks the door for its columns and cards, and for what a
    /// drop writes, instead of keeping its own copy of the board rules.
    #[test]
    fn a_board_answers_its_columns_cards_and_moves() {
        let answer = run_json(
            r#"{"kind":"board","projectId":"p","now":"2026-11-08T12:00:00.000Z",
                "customStates":[{"role":"review","name":"Review","order":3},{"role":"doing","name":"In progress","order":1}],
                "lists":[{"id":"l1","name":"iOS","projectId":"p","manualSortOrder":["b2","b1"]},{"id":"other","name":"Home"}],
                "boardListId":"l1",
                "tasks":[{"id":"a","listIds":["l1"],"statusRole":"review"},{"id":"b1","listIds":["l1"]},{"id":"b2","listIds":["l1"],"statusRole":"gone"},
                         {"id":"c","listIds":["l1"],"completed":true,"updatedAt":"2026-11-08T11:00:00.000Z"},{"id":"sub","listIds":["l1"],"parentTaskId":"a"},
                         {"id":"x","listIds":["other"]}]}"#,
        );
        let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
        let columns = answer["value"].as_array().unwrap();
        let ids: Vec<&str> = columns.iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [
                "__virtual_inbox__",
                "ready",
                "doing",
                "waiting",
                "review",
                "__virtual_done__"
            ]
        );
        assert_eq!(columns[2]["name"], "In progress");
        // An unknown role falls to Inbox; the manual order arranges it; no subtask, no other project's task.
        assert_eq!(columns[0]["taskIds"], serde_json::json!(["b2", "b1"]));
        assert_eq!(columns[4]["taskIds"], serde_json::json!(["a"]));
        assert_eq!(columns[5]["taskIds"], serde_json::json!(["c"]));

        let moved: serde_json::Value = serde_json::from_str(&run_json(
            r#"{"kind":"boardMove","task":{"id":"a","listIds":["l1"],"statusRole":"review"},"column":"__virtual_inbox__"}"#,
        )).unwrap();
        assert_eq!(
            moved["value"],
            serde_json::json!({ "listIds": ["l1"], "completed": false, "statusRole": null })
        );
        let refused: serde_json::Value = serde_json::from_str(&run_json(
            r#"{"kind":"boardMove","task":{"id":"a"},"column":"nope"}"#,
        ))
        .unwrap();
        assert_eq!(refused["ok"], false);

        // An offset too large to be one is refused, not overflowed.
        let absurd: serde_json::Value = serde_json::from_str(&run_json(
            r#"{"kind":"board","projectId":"p","now":"2026-11-08T12:00:00.000Z","utcOffsetMinutes":2147483647}"#,
        ))
        .unwrap();
        assert_eq!(absurd["ok"], false);
    }

    /// AWTD-1676: the multi-list board asks the door which lists are rows and which could be
    /// added, instead of the Mac's Swift copy of web's `lib/project-board-rows.ts`.
    #[test]
    fn the_board_rows_and_their_candidates_come_from_the_door_awtd_1676() {
        let lists = r#"[{"id":"web","name":"Web","projectId":"p1"},{"id":"ios","name":"iOS","projectId":"p1"},
            {"id":"label","name":"Bug","projectId":"p1","listType":"label"},{"id":"groceries","name":"Groceries"}]"#;
        let defaults: serde_json::Value = serde_json::from_str(&run_json(&format!(
            r#"{{"kind":"boardRows","projectId":"p1","lists":{lists}}}"#
        )))
        .unwrap();
        assert_eq!(
            defaults["value"],
            serde_json::json!({ "rowListIds": ["web", "ios"], "candidateListIds": ["groceries"] })
        );
        let saved: serde_json::Value = serde_json::from_str(&run_json(&format!(
            r#"{{"kind":"boardRows","projectId":"p1","savedListIds":["groceries","gone","label"],"lists":{lists}}}"#
        )))
        .unwrap();
        assert_eq!(
            saved["value"],
            serde_json::json!({ "rowListIds": ["groceries"], "candidateListIds": ["ios", "web"] })
        );
    }

    /// AWTD-1676: one row under the project's columns. A list from no project draws its cards,
    /// and a role the project has no column for is Inbox.
    #[test]
    fn a_board_row_draws_its_list_under_the_projects_columns_awtd_1676() {
        let answer: serde_json::Value = serde_json::from_str(&run_json(
            r#"{"kind":"boardRow","rowListId":"home","now":"2026-11-08T12:00:00.000Z",
                "customStates":[{"role":"review","name":"Review","order":3}],
                "lists":[{"id":"home","name":"Home","manualSortOrder":["b","a"]}],
                "tasks":[{"id":"a","listIds":["home"],"statusRole":"custom-qa"},{"id":"b","listIds":["home"]},
                         {"id":"c","listIds":["home"],"statusRole":"review"},{"id":"x","listIds":["other"],"statusRole":"review"}]}"#,
        ))
        .unwrap();
        let columns = answer["value"].as_array().expect("columns");
        assert_eq!(columns.len(), 6);
        assert_eq!(columns[0]["id"], "__virtual_inbox__");
        assert_eq!(columns[0]["taskIds"], serde_json::json!(["b", "a"]));
        assert_eq!(columns[4]["id"], "review");
        assert_eq!(columns[4]["taskIds"], serde_json::json!(["c"]));
    }

    /// AATD-50: the custom editor's Preview words a pattern nobody has saved yet.
    #[test]
    fn aatd_50_a_repeat_being_edited_is_worded_before_it_is_saved() {
        let answer = run(serde_json::from_value(serde_json::json!({
            "kind": "repeatSummary",
            "repeating": "custom",
            "repeatingData": { "type": "custom", "unit": "weeks", "interval": 2, "weekdays": ["monday", "thursday"] },
            "repeatFrom": "COMPLETION_DATE"
        }))
        .expect("a rule"));
        assert!(answer.ok, "{:?}", answer.error);
        let parts = answer.value.expect("parts");
        assert!(!parts.as_array().expect("parts").is_empty(), "{parts}");
        assert!(parts.to_string().contains("weeks"), "{parts}");
    }

    use super::*;
    use serde_json::{json, Value};

    fn answer(request: Value) -> Value {
        serde_json::from_str(&run_json(&request.to_string())).expect("the door answers JSON")
    }

    #[test]
    fn completion_answers_in_the_wire_shape_the_shell_sent() {
        let reply = answer(json!({
            "kind": "completion",
            "task": {
                "id": "t1", "repeating": "weekly", "repeatFrom": "DUE_DATE",
                "dueDateTime": "2026-09-28T09:00:00Z", "isAllDay": false
            },
            "completed": true,
            "now": "2026-09-28T12:00:00Z",
            "timeZone": "UTC"
        }));
        assert_eq!(
            reply,
            json!({ "ok": true, "value": {
                "outcome": "rollForward",
                "dueDateTime": "2026-10-05T09:00:00Z",
                "isAllDay": false,
                "occurrenceCount": 1
            }})
        );
    }

    #[test]
    fn the_calculator_answers_on_its_own() {
        let reply = answer(json!({
            "kind": "nextOccurrence",
            "repeating": "custom",
            "pattern": { "type": "custom", "unit": "weeks", "interval": 1,
                         "weekdays": ["monday", "wednesday", "friday"] },
            "currentDueDate": "2026-09-28T09:00:00Z",
            "completion": "2026-09-28T12:00:00Z",
            "repeatFrom": "DUE_DATE",
            "occurrenceCount": 4,
            "timeZone": "America/Los_Angeles"
        }));
        assert_eq!(
            reply,
            json!({ "ok": true, "value": {
                "nextDueDate": "2026-09-30T09:00:00Z",
                "shouldTerminate": false,
                "newOccurrenceCount": 5
            }})
        );
    }

    /// An all-day series steps on UTC midnights even when the person is in California, where the
    /// completion instant is already the next UTC day.
    #[test]
    fn an_all_day_series_steps_in_utc_whatever_the_zone() {
        let reply = answer(json!({
            "kind": "nextOccurrence",
            "repeating": "daily",
            "currentDueDate": "2026-01-06T00:00:00Z",
            "completion": "2026-01-06T14:42:00Z",
            "repeatFrom": "COMPLETION_DATE",
            "timeZone": "America/Los_Angeles",
            "isAllDay": true
        }));
        assert_eq!(reply["value"]["nextDueDate"], "2026-01-07T00:00:00Z");
    }

    /// D6: an uppercase role written by an old endpoint is still that role.
    #[test]
    fn list_access_answers_every_question_at_once() {
        let reply = answer(json!({
            "kind": "listAccess",
            "list": { "id": "l1", "ownerId": "owner", "privacy": "PRIVATE",
                      "listMembers": [{ "userId": "u2", "role": "ADMIN" }] },
            "userId": "u2"
        }));
        assert_eq!(reply["value"]["role"], "admin");
        assert_eq!(reply["value"]["canManage"], true);
        assert_eq!(reply["value"]["canDelete"], false);
    }

    /// AWTD-1061: the server sends the project; the core resolves the role from it.
    #[test]
    fn list_access_reads_the_project_when_the_server_sends_it() {
        let reply = answer(json!({
            "kind": "listAccess",
            "list": { "ownerId": "owner", "privacy": "PRIVATE", "listType": "status",
                      "project": { "ownerId": "boss", "members": [],
                                   "lists": [{ "id": "d", "listMembers": [{ "userId": "u2" }] }] } },
            "userId": "u2"
        }));
        assert_eq!(reply["value"]["role"], "member");
        assert_eq!(reply["value"]["canEditTasks"], true);
        assert_eq!(reply["value"]["canManage"], false);
    }

    #[test]
    fn nobody_signed_in_has_no_access() {
        let reply = answer(json!({
            "kind": "listAccess",
            "list": { "id": "l1", "ownerId": "owner", "privacy": "PUBLIC" }
        }));
        assert_eq!(reply["value"]["role"], serde_json::Value::Null);
        assert_eq!(reply["value"]["canView"], false);
    }

    #[test]
    fn a_search_box_query_parses_to_its_filter_and_text() {
        let reply = answer(json!({
            "kind": "searchParse",
            "query": "assignee:@sam priority:urgent list:\"Bugs and Polish\" overdue AST-142"
        }));
        assert_eq!(reply["ok"], true);
        let value = &reply["value"];
        assert_eq!(value["text"], "overdue");
        assert_eq!(value["assignee"], "sam");
        assert_eq!(value["priorities"], json!(["high"]));
        assert_eq!(value["listNames"], json!(["Bugs and Polish"]));
        assert_eq!(value["identifier"], "AST-142");
        assert_eq!(value["due"], serde_json::Value::Null);
        assert_eq!(value["isEmpty"], false);
    }

    #[test]
    fn an_empty_search_asks_for_nothing() {
        let reply = answer(json!({ "kind": "searchParse", "query": "   " }));
        assert_eq!(reply["value"]["text"], "");
        assert_eq!(reply["value"]["isEmpty"], true);
    }

    #[test]
    fn a_quick_add_line_parses_to_its_parts() {
        let reply = answer(json!({
            "kind": "smartParse",
            "text": "Call mum tomorrow #family urgent",
            "lists": [{ "id": "f", "name": "Family" }],
            "today": "2026-09-28"
        }));
        assert_eq!(reply["value"]["title"], "Call mum");
        assert_eq!(reply["value"]["listIds"], json!(["f"]));
        assert_eq!(reply["value"]["dueDay"], "2026-09-29");
        assert_eq!(reply["value"]["priority"], 3);
    }

    /// The four `renderMarkdown` tests below belong to the `markdown` feature: without it the
    /// rule is gone, and what the browser build answers instead is `tests/browser_build.rs`
    /// (task 4d05306d).
    #[cfg(feature = "markdown")]
    #[test]
    fn markdown_renders_to_blocks() {
        let reply = answer(json!({ "kind": "renderMarkdown", "text": "**hi**" }));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["value"][0]["kind"], "paragraph");
        assert_eq!(reply["value"][0]["inlines"][0]["bold"], true);
    }

    /// AWTD-1064: a task id links only with the reader's context, as `render_with_identifiers`.
    #[cfg(feature = "markdown")]
    #[test]
    fn markdown_links_task_ids_with_the_readers_context() {
        let reply = answer(json!({
            "kind": "renderMarkdown",
            "text": "Fixed by AWTD-12",
            "origin": "https://astrid.cc",
            "identifiers": { "projectKey": "AWTD", "keys": ["AWTD"], "hidden": [] }
        }));
        assert_eq!(reply["ok"], true);
        let inlines = &reply["value"][0]["inlines"];
        assert_eq!(inlines[1]["text"], "AWTD-12");
        assert_eq!(inlines[1]["link"], "https://astrid.cc/t/AWTD-12");

        let without = answer(json!({ "kind": "renderMarkdown", "text": "Fixed by AWTD-12" }));
        assert_eq!(
            without["value"][0]["inlines"][0]["text"],
            "Fixed by AWTD-12"
        );
    }

    /// AWTD-1630: a relative link opens on the server the request names — the reader's — and the
    /// id's `/t/…` with it, so a partner build never sends its reader to astrid.cc.
    #[cfg(feature = "markdown")]
    #[test]
    fn a_relative_link_opens_on_the_origin_the_request_names_awtd_1630() {
        let reply = answer(json!({
            "kind": "renderMarkdown",
            "text": "[home](/lists/abc) and AWTD-12",
            "origin": "https://tasks.partner.example/",
            "identifiers": { "projectKey": "AWTD", "keys": ["AWTD"], "hidden": [] }
        }));
        assert_eq!(reply["ok"], true);
        let inlines = &reply["value"][0]["inlines"];
        assert_eq!(
            inlines[0]["link"],
            "https://tasks.partner.example/lists/abc"
        );
        assert_eq!(
            inlines[2]["link"],
            "https://tasks.partner.example/t/AWTD-12"
        );

        let plain = answer(json!({
            "kind": "renderMarkdown",
            "text": "[home](/lists/abc)",
            "origin": "https://tasks.partner.example"
        }));
        assert_eq!(
            plain["value"][0]["inlines"][0]["link"],
            "https://tasks.partner.example/lists/abc"
        );
    }

    /// AWTD-1630: a shell that names no origin yet (iOS, Android today) draws what it always drew;
    /// a `require-base-url` build has no Astrid server to fall back to, so the link stays relative.
    #[cfg(feature = "markdown")]
    #[test]
    fn a_request_without_an_origin_falls_back_per_build_awtd_1630() {
        let reply = answer(json!({ "kind": "renderMarkdown", "text": "[home](/lists/abc)" }));
        let link = &reply["value"][0]["inlines"][0]["link"];
        #[cfg(not(feature = "require-base-url"))]
        assert_eq!(link, "https://astrid.cc/lists/abc");
        #[cfg(feature = "require-base-url")]
        assert_eq!(link, "/lists/abc");
    }

    /// R2 through the door: a move into Waiting with nothing to wait on is refused with the code
    /// a client turns into the picker.
    #[test]
    fn a_waiting_write_without_a_condition_is_refused_with_its_code() {
        let reply = answer(json!({
            "kind": "waitingWrite",
            "task": { "id": "t", "statusRole": "doing", "assigneeId": "u" },
            "change": { "statusRole": "waiting" },
            "now": "2026-11-12T12:00:00Z"
        }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "refused");
        assert_eq!(reply["error"]["code"], "WAITING_NEEDS_CONDITION");

        let reply = answer(json!({
            "kind": "waitingWrite",
            "task": { "id": "t", "statusRole": "doing", "assigneeId": "u" },
            "change": { "statusRole": "waiting", "dueDateTime": "2026-11-13T00:00:00Z" },
            "now": "2026-11-12T12:00:00Z"
        }));
        assert_eq!(
            reply["value"],
            json!({ "statusRole": "waiting", "assignee": "clear", "transition": "enter" })
        );
    }

    #[test]
    fn a_rule_the_core_does_not_know_says_so() {
        let reply = answer(json!({ "kind": "somethingLater" }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "badRequest");
    }

    #[test]
    fn an_unknown_zone_is_refused_rather_than_read_as_utc() {
        let reply = answer(json!({
            "kind": "completion", "task": { "id": "t1" }, "completed": true,
            "now": "2026-09-28T12:00:00Z", "timeZone": "Mars/Olympus_Mons"
        }));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "badRequest");
    }

    /// Task 75a27f7f: `Board/TaskIdentifiers.swift` reads a typed `AWTD-1007` into its parts
    /// from a view body, so the door answers it synchronously rather than through an `App`
    /// command. Not an identifier is `null`, which is the answer a search box acts on.
    #[test]
    fn task_75a27f7f_the_door_reads_an_identifier_into_its_parts() {
        assert_eq!(
            answer(json!({ "kind": "identifierParse", "value": " awtd-1007 " }))["value"],
            json!({ "key": "AWTD", "sequence": 1007 })
        );
        // Everything that merely looks close is `null`, not an error: a title is a normal thing
        // to type into the box this answers for.
        for not_one in ["Just a title", "A-1007", "AWTD-0", "#1007", "AWTD-1007a"] {
            let reply = answer(json!({ "kind": "identifierParse", "value": not_one }));
            assert_eq!(reply["ok"], true, "{not_one}: {reply}");
            assert_eq!(reply["value"], Value::Null, "{not_one}");
        }
    }

    /// Task 75a27f7f: the other half of `TaskIdentifiers` — writing one out. The server is the
    /// only minter, so this formats parts somebody else already holds.
    #[test]
    fn task_75a27f7f_the_door_writes_an_identifier_out() {
        assert_eq!(
            answer(json!({ "kind": "identifierFormat", "key": "awtd", "sequence": 1007 }))["value"],
            json!("AWTD-1007")
        );
    }

    /// Task 75a27f7f: `Tasks/NewTaskDefaults.swift`. A list set up as high, weekly, Dana's, due
    /// tomorrow at five hands quick-add exactly that — from a list **passed over the wire**, since
    /// this door has no cache to look one up in and no clock to ask what tomorrow is.
    #[test]
    fn task_75a27f7f_the_door_resolves_a_new_task_s_defaults() {
        let reply = answer(json!({
            "kind": "quickAddDefaults",
            "list": {
                "id": "l1", "name": "Work",
                "defaultPriority": 3,
                "defaultRepeating": "weekly",
                "defaultAssigneeId": "dana",
                "defaultDueDate": "tomorrow",
                "defaultDueTime": "17:00"
            },
            "userId": "jon",
            "now": "2026-09-07T12:00:00.000Z",
            "utcOffsetMinutes": -420
        }));

        assert_eq!(reply["ok"], true, "{reply}");
        let value = &reply["value"];
        assert_eq!(value["priority"], 3);
        assert_eq!(value["assigneeId"], "dana");
        assert_eq!(value["repeating"], "weekly");
        // Tomorrow in California is Sep 8, and 17:00 there is midnight UTC on the 9th.
        assert_eq!(value["dueDateTime"], "2026-09-09T00:00:00Z");
        assert_eq!(value["isAllDay"], false);
        assert_eq!(value["isPrivate"], false);
    }

    /// Task 75a27f7f: the two things the answer turns on besides the list.
    ///
    /// A default never overrides a choice — that is what sending `priority` or `assigneeId`
    /// means. And a request with **no list** is one of the synthesised views: My Tasks is never
    /// cached, so there is no row to read defaults from, and the creator is still the fallback
    /// there rather than nobody (the web's `components/quick-add.tsx`).
    #[test]
    fn task_75a27f7f_a_choice_beats_a_default_and_a_view_still_starts_out_mine() {
        let chosen = answer(json!({
            "kind": "quickAddDefaults",
            "list": { "id": "l1", "name": "Work", "defaultPriority": 3, "defaultAssigneeId": "dana" },
            "userId": "jon",
            "now": "2026-09-07T12:00:00.000Z",
            "utcOffsetMinutes": -420,
            "priority": 1,
            "assigneeId": "sam"
        }));
        assert_eq!(chosen["value"]["priority"], 1, "{chosen}");
        assert_eq!(chosen["value"]["assigneeId"], "sam");

        // `unassigned` is a choice, not a user id — the same spelling a list's own default uses.
        let nobody = answer(json!({
            "kind": "quickAddDefaults",
            "list": { "id": "l1", "name": "Work", "defaultAssigneeId": "dana" },
            "userId": "jon",
            "now": "2026-09-07T12:00:00.000Z",
            "assigneeId": "unassigned"
        }));
        assert_eq!(nobody["value"]["assigneeId"], Value::Null, "{nobody}");

        // No list at all: My Tasks and the rest of the views.
        let view = answer(json!({
            "kind": "quickAddDefaults",
            "userId": "jon",
            "now": "2026-09-07T12:00:00.000Z"
        }));
        assert_eq!(view["ok"], true, "{view}");
        assert_eq!(view["value"]["assigneeId"], "jon");
        assert_eq!(view["value"]["dueDateTime"], Value::Null);
        assert_eq!(
            view["value"]["isAllDay"], true,
            "all-day until a time is chosen, as `TaskDraft::new` has it"
        );

        // Signed out, and an offset too large to be one, are both answered rather than guessed.
        let signed_out = answer(json!({
            "kind": "quickAddDefaults",
            "list": { "id": "l1", "name": "Work" },
            "now": "2026-09-07T12:00:00.000Z"
        }));
        assert_eq!(
            signed_out["value"]["assigneeId"],
            Value::Null,
            "{signed_out}"
        );

        let absurd = answer(json!({
            "kind": "quickAddDefaults",
            "list": { "id": "l1", "name": "Work" },
            "now": "2026-09-07T12:00:00.000Z",
            "utcOffsetMinutes": 2147483647
        }));
        assert_eq!(absurd["ok"], false, "{absurd}");
    }

    /// Task 75a27f7f: `Tasks/ReminderTriage.swift` decides what a tapped notification puts in
    /// front of somebody. Mine, open and due; the reminded one first; ids, not rows.
    #[test]
    fn task_75a27f7f_the_door_deals_the_triage_deck() {
        let tasks = json!([
            { "id": "late", "assigneeId": "me", "dueDateTime": "2000-01-01T00:00:00Z", "isAllDay": true },
            { "id": "theirs", "assigneeId": "bo", "dueDateTime": "2000-01-01T00:00:00Z", "isAllDay": true },
            { "id": "pinged", "assigneeId": "me", "dueDateTime": "2099-06-01T00:00:00Z", "isAllDay": true }
        ]);

        assert_eq!(
            answer(json!({
                "kind": "reminderTriage", "tasks": tasks, "userId": "me",
                "remindedTaskId": "pinged", "now": "2026-06-15T12:00:00Z"
            }))["value"],
            json!({ "taskIds": ["pinged", "late"] })
        );
        // Not due yet, and nobody pinged it.
        assert_eq!(
            answer(json!({
                "kind": "reminderTriage", "tasks": tasks, "userId": "me",
                "now": "2026-06-15T12:00:00Z"
            }))["value"],
            json!({ "taskIds": ["late"] })
        );
        // Nobody signed in: nobody to triage for, so an empty deck rather than everybody's.
        assert_eq!(
            answer(json!({
                "kind": "reminderTriage", "tasks": tasks, "now": "2026-06-15T12:00:00Z"
            }))["value"],
            json!({ "taskIds": [] })
        );
        // An offset no clock has is refused rather than quietly read as UTC.
        let absurd = answer(json!({
            "kind": "reminderTriage", "tasks": tasks, "userId": "me",
            "now": "2026-06-15T12:00:00Z", "utcOffsetMinutes": 999_999
        }));
        assert_eq!(absurd["ok"], false, "{absurd}");
    }

    /// Task 75a27f7f: `Filters/DragNesting.swift` and `SubtaskPromotion.swift` decide what a
    /// drop means while the drag is in flight, so this has to be synchronous too. A drop ON a
    /// row nests; a drop on the LINE leaves the parent; a cycle is refused.
    #[test]
    fn task_75a27f7f_the_door_says_what_a_drop_nests() {
        let tasks = json!([
            { "id": "parent" },
            { "id": "child", "parentTaskId": "parent" },
            { "id": "loose" }
        ]);

        // On a row: become its subtask.
        assert_eq!(
            answer(json!({
                "kind": "nesting", "zone": { "on": "parent" },
                "draggedId": "loose", "tasks": tasks
            }))["value"],
            json!({ "outcome": "makeSubtask", "parentId": "parent" })
        );
        // Never under itself, and never under its own descendant.
        assert_eq!(
            answer(json!({
                "kind": "nesting", "zone": { "on": "child" },
                "draggedId": "parent", "tasks": tasks
            }))["value"],
            json!({ "outcome": "none", "parentId": Value::Null })
        );
        // On the line between rows: a subtask leaves its parent, a top-level row only reorders.
        assert_eq!(
            answer(json!({
                "kind": "nesting", "zone": "line", "draggedId": "child", "tasks": tasks
            }))["value"]["outcome"],
            json!("moveToTopLevel")
        );
        assert_eq!(
            answer(json!({
                "kind": "nesting", "zone": "line", "draggedId": "loose", "tasks": tasks
            }))["value"]["outcome"],
            json!("reorderOnly")
        );
        // The outdent gutter climbs one level.
        assert_eq!(
            answer(json!({
                "kind": "nesting", "zone": "outdent", "draggedId": "child", "tasks": tasks
            }))["value"]["outcome"],
            json!("moveToTopLevel")
        );
        // The keyboard's indent nests under the previous sibling, among the rows as drawn.
        assert_eq!(
            answer(json!({
                "kind": "nesting", "zone": { "indent": ["loose", "parent"] },
                "draggedId": "parent", "tasks": tasks
            }))["value"],
            json!({ "outcome": "makeSubtask", "parentId": "loose" })
        );
    }

    /// Task 75a27f7f: a dragged id the shell did not send among `tasks` is a bug in the shell,
    /// and the door says so rather than answering `none` — which would look like a refused drop
    /// and silently swallow it.
    #[test]
    fn task_75a27f7f_a_dragged_row_the_shell_did_not_send_is_refused() {
        let reply = answer(json!({
            "kind": "nesting", "zone": "line", "draggedId": "ghost",
            "tasks": [{ "id": "loose" }]
        }));
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error"]["kind"], "badRequest");
    }

    /// Task 75a27f7f: `AutocompleteSupport.swift` asks on every keystroke in the comment box
    /// whether a popup is open and what has been typed into it, so it goes through this door.
    /// `null` is the answer when no trigger is live — the usual case, and not a failure.
    #[test]
    fn task_75a27f7f_the_door_finds_the_trigger_the_caret_is_inside() {
        let reply = answer(json!({ "kind": "mentionTrigger", "text": "hi @jo", "caret": 6 }));
        assert_eq!(
            reply["value"],
            json!({ "kind": "mention", "start": 3, "query": "jo" })
        );
        // A finished reference earlier in the line does not re-open anything.
        let settled = answer(json!({
            "kind": "mentionTrigger", "text": "@[Jon](u1) and then some", "caret": 24
        }));
        assert_eq!(settled["ok"], true, "{settled}");
        assert_eq!(settled["value"], Value::Null);
        // The caret counts UTF-16 units, as a TextBox and `substring` both do.
        let after_emoji = answer(json!({ "kind": "mentionTrigger", "text": "🎉 #bu", "caret": 6 }));
        assert_eq!(
            after_emoji["value"],
            json!({ "kind": "list", "start": 3, "query": "bu" })
        );
    }

    /// Task 75a27f7f: the other half — what the chosen row writes into the box. The server
    /// resolves `@[Name](id)` and nothing else, so the shell must not assemble it itself.
    #[test]
    fn task_75a27f7f_the_door_writes_a_chosen_mention_into_the_text() {
        let reply = answer(json!({
            "kind": "mentionInsert", "text": "hi @jo", "caret": 6,
            "triggerKind": "mention", "label": "Jon", "id": "u1"
        }));
        assert_eq!(
            reply["value"],
            json!({ "text": "hi @[Jon](u1) ", "caret": 14 })
        );
        // No trigger at the caret: the text comes back untouched rather than corrupted.
        let untouched = answer(json!({
            "kind": "mentionInsert", "text": "plain words", "caret": 11,
            "triggerKind": "task", "label": "T", "id": "t1"
        }));
        assert_eq!(
            untouched["value"],
            json!({ "text": "plain words", "caret": 11 })
        );
    }

    /// Task 75a27f7f: `Tasks/TaskCapabilities.swift` decides, as a row draws, whether its leading
    /// control is a checkbox or a copy — a passer-by on a public list can only copy (D15).
    #[test]
    fn task_75a27f7f_the_door_says_when_a_task_is_only_copyable() {
        let lists = json!([{ "id": "pub", "ownerId": "owner", "privacy": "PUBLIC" }]);
        let task = json!({ "id": "t", "listIds": ["pub"], "creatorId": "owner" });

        let passer_by = answer(json!({
            "kind": "copyOnly", "task": task, "lists": lists, "userId": "stranger"
        }));
        assert_eq!(passer_by["value"], json!(true));

        let owner = answer(json!({
            "kind": "copyOnly", "task": task, "lists": lists, "userId": "owner"
        }));
        assert_eq!(owner["value"], json!(false));

        // A private list is never copy-only, whoever is reading.
        let private = answer(json!({
            "kind": "copyOnly",
            "task": { "id": "t", "listIds": ["p"], "creatorId": "owner" },
            "lists": [{ "id": "p", "ownerId": "owner", "privacy": "PRIVATE" }],
            "userId": "stranger"
        }));
        assert_eq!(private["value"], json!(false));
    }

    /// Task 75a27f7f: `CommentVisibility.swift` decides what of a comment is drawn at all — a
    /// caption-less attachment gets no text bubble, and an image is shown where it sits while a
    /// document is a chip to open.
    #[test]
    fn task_75a27f7f_the_door_says_what_of_a_comment_is_drawn() {
        let reply = answer(json!({
            "kind": "commentVisibility",
            "comment": {
                "id": "c1", "content": "   ",
                "secureFiles": [
                    { "id": "f1", "originalName": "shot.PNG", "mimeType": "image/png" },
                    { "id": "f2", "originalName": "notes.pdf", "mimeType": "application/pdf" }
                ]
            }
        }));
        assert_eq!(
            reply["value"],
            json!({
                // No author and no GitHub login: the server's (AITD-667).
                "isSystem": true,
                "showsText": false,
                "isEmpty": false,
                "files": [
                    { "id": "f1", "name": "shot.PNG", "mimeType": "image/png", "rendersInline": true },
                    { "id": "f2", "name": "notes.pdf", "mimeType": "application/pdf", "rendersInline": false }
                ]
            })
        );

        // Neither text nor a file: nothing to draw, which is the row that must not be drawn.
        let nothing = answer(json!({
            "kind": "commentVisibility", "comment": { "id": "c2", "content": "" }
        }));
        assert_eq!(nothing["value"]["isEmpty"], json!(true));
        assert_eq!(nothing["value"]["files"], json!([]));

        let spoken = answer(json!({
            "kind": "commentVisibility", "comment": { "id": "c3", "content": "Done" }
        }));
        assert_eq!(spoken["value"]["showsText"], json!(true));
        assert_eq!(spoken["value"]["isEmpty"], json!(false));
    }

    /// Task 75a27f7f: the Mac's `Keyboard/*` resolves a bare key press to an action while a view
    /// has focus. Suppression is part of the answer — a key pressed into a text field is text.
    #[test]
    fn task_75a27f7f_the_door_resolves_a_pressed_key_to_its_action() {
        let reply = answer(json!({ "kind": "shortcut", "key": "n" }));
        assert_eq!(
            reply["value"],
            json!({ "action": "newTask", "title": "New task", "requiresSelection": false })
        );

        // An arrow reaches the door under either name: the web's glyph or a Windows VirtualKey.
        for key in ["ArrowDown", "\u{2193}"] {
            let moved = answer(json!({ "kind": "shortcut", "key": key, "hasSelection": true }));
            assert_eq!(moved["value"]["action"], "selectNext", "{key}");
        }

        // Selection-scoped without a selection, inside a text field, inside a modal, unbound:
        // all four are `null`, which is the answer the shell acts on, not a failure.
        for request in [
            json!({ "kind": "shortcut", "key": "x" }),
            json!({ "kind": "shortcut", "key": "n", "isTextFieldFocused": true }),
            json!({ "kind": "shortcut", "key": "n", "isModalPresented": true }),
            json!({ "kind": "shortcut", "key": "\u{00e7}" }),
        ] {
            let reply = answer(request.clone());
            assert_eq!(reply["ok"], true, "{request}: {reply}");
            assert_eq!(reply["value"], Value::Null, "{request}");
        }
    }

    /// The controls a task's lists let an editor offer (AITD-667, iOS `TaskCapabilities`). A
    /// list the task is on refuses; a list not loaded, or one the task is not on, refuses nothing.
    #[test]
    fn capabilities_are_refused_only_by_a_loaded_list_the_task_is_on_aitd667() {
        let board = json!({ "id": "board", "name": "Board",
            "supports": { "priority": false, "dueDate": false, "privateTasks": false,
                          "deleteTask": "remove_from_list" } });
        let classic = json!({ "id": "home", "name": "Home" });

        let on_board = answer(json!({
            "kind": "capabilities", "listIds": ["board"], "lists": [board, classic]
        }));
        assert_eq!(on_board["ok"], true, "{on_board}");
        assert_eq!(
            on_board["value"],
            json!({
                "priority": false, "dueDate": false, "privateTasks": false, "deletion": "removeFromList"
            })
        );

        // Also on a classic list: the delete really deletes, while the board still refuses its
        // controls.
        let both = answer(json!({
            "kind": "capabilities", "listIds": ["board", "home"], "lists": [board, classic]
        }));
        assert_eq!(
            both["value"],
            json!({
                "priority": false, "dueDate": false, "privateTasks": false, "deletion": "delete"
            })
        );

        // The board is not one of the task's lists, and a list that has not loaded refuses nothing.
        for list_ids in [json!(["home"]), json!(["not-loaded"]), json!([])] {
            let free = answer(json!({
                "kind": "capabilities", "listIds": list_ids, "lists": [board, classic]
            }));
            assert_eq!(
                free["value"],
                json!({
                    "priority": true, "dueDate": true, "privateTasks": true, "deletion": "delete"
                }),
                "{list_ids}"
            );
        }
    }

    /// A comment with no author and no GitHub login is the server's — except offline, where a
    /// cached comment can lose its `authorId` and "no author" proves nothing (AITD-667, iOS
    /// `CommentVisibility.isSystem`).
    #[test]
    fn comment_visibility_names_a_system_comment_but_never_offline_aitd667() {
        let system = json!({ "id": "c1", "content": "marked this as complete" });
        let person = json!({ "id": "c2", "content": "hi", "authorId": "u1" });
        let github = json!({ "id": "c3", "content": "hi", "remoteAuthorLogin": "octocat" });

        let ask = |comment: &Value, offline: Option<bool>| {
            let mut request = json!({ "kind": "commentVisibility", "comment": comment });
            if let Some(offline) = offline {
                request["offline"] = json!(offline);
            }
            answer(request)["value"]["isSystem"].clone()
        };
        assert_eq!(ask(&system, None), json!(true));
        assert_eq!(ask(&system, Some(false)), json!(true));
        assert_eq!(ask(&system, Some(true)), json!(false));
        assert_eq!(ask(&person, None), json!(false));
        assert_eq!(ask(&github, None), json!(false));
    }

    /// The Mac palette's ranking (AITD-667, `FuzzyMatch`, `MacPaletteSearch`, `CommandRegistry`):
    /// matches only, best score first, a tie to the shorter title, then to the order given.
    #[test]
    fn palette_rank_orders_matches_best_first_and_ties_to_the_shorter_title_aitd667() {
        let reply = answer(json!({
            "kind": "paletteRank", "query": "tdy",
            "titles": ["Yesterday", "Today's plan", "Nothing", "Today", "today"]
        }));
        assert_eq!(reply["ok"], true, "{reply}");
        // "Today" and "today" score alike and are as long: given order. "Today's plan" scores
        // the same as both and is longer. "Yesterday" matches t-d-y further apart. "Nothing"
        // has no y.
        assert_eq!(reply["value"], json!([3, 4, 1, 0]));

        let none = answer(json!({ "kind": "paletteRank", "query": "zzz", "titles": ["Today"] }));
        assert_eq!(none["value"], json!([]));
    }

    /// The whole list pipeline through the door, with rows rather than ids (task `c2109ff8`).
    ///
    /// The contract fixtures drive this rule with `idsOnly`, because what they assert is which
    /// tasks are kept and in what order. Nothing there would notice if the projection half never
    /// ran, so the row keys a shell reads are checked here instead.
    #[test]
    fn c2109ff8_rows_filters_sorts_splices_and_projects_in_one_answer() {
        let request = json!({
            "kind": "rows",
            "now": "2026-10-17T12:00:00.000Z",
            "list": { "id": "l", "sortBy": "priority", "showSubtasks": true },
            "subtaskDisplay": "always",
            "lists": [{ "id": "l", "name": "Home", "color": "#ff0000" }],
            "tasks": [
                { "id": "low", "title": "Low", "priority": 1, "listIds": ["l"] },
                { "id": "high", "title": "High", "priority": 3, "listIds": ["l"] },
                { "id": "done", "title": "Done", "completed": true, "listIds": ["l"] }
            ],
            "subtasks": [{ "id": "kid", "title": "Kid", "parentTaskId": "high", "listIds": ["l"] }],
        });
        let value = answer(request.clone())["value"].clone();

        // Sorted by priority, the subtask spliced under its parent, the completed task gone by
        // the default completion filter.
        let ids: Vec<&str> = value["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["id"].as_str().expect("an id"))
            .collect();
        assert_eq!(ids, ["high", "kid", "low"], "{value}");
        assert_eq!(value["total"], 3);
        assert_eq!(value["matched"], 2, "the completed task is filtered out");
        assert_eq!(value["sortBy"], "priority");

        // Projected, not just selected: the keys a shell draws from.
        let parent = &value["rows"][0];
        assert_eq!(parent["title"], "High");
        assert_eq!(parent["depth"], 0);
        assert_eq!(parent["subtaskCount"], 1);
        assert_eq!(parent["leading"], json!({ "kind": "unassigned" }));
        assert_eq!(parent["due"], json!({ "key": "none" }));
        assert_eq!(parent["listChips"][0]["name"], "Home");
        assert_eq!(value["rows"][1]["depth"], 1, "the subtask is indented");

        // The same request with `idsOnly` is the same selection and no rows at all — the cheap
        // answer must not be a different answer.
        let mut cheap = request;
        cheap["idsOnly"] = json!(true);
        let cheap = answer(cheap)["value"].clone();
        assert_eq!(cheap["ids"], json!(["high", "kid", "low"]));
        assert_eq!(cheap["total"], 3);
        assert!(cheap.get("rows").is_none(), "{cheap}");
    }

    /// A window is a window, not a filter: the counts describe what it was cut from.
    #[test]
    fn c2109ff8_a_row_window_still_reports_the_whole_list() {
        let value = answer(json!({
            "kind": "rows",
            "now": "2026-10-17T12:00:00.000Z",
            "list": { "id": "l", "sortBy": "createdAt" },
            "tasks": [
                { "id": "a", "createdAt": "2026-10-03T00:00:00Z" },
                { "id": "b", "createdAt": "2026-10-02T00:00:00Z" },
                { "id": "c", "createdAt": "2026-10-01T00:00:00Z" }
            ],
            "offset": 1,
            "limit": 1,
            "idsOnly": true,
        }))["value"]
            .clone();
        assert_eq!(value["ids"], json!(["b"]));
        assert_eq!(value["offset"], 1);
        assert_eq!(value["total"], 3);
    }

    /// An offset no clock has is refused rather than overflowed — the same guard `board` carries.
    #[test]
    fn c2109ff8_rows_refuses_an_impossible_utc_offset() {
        let refused = answer(json!({
            "kind": "rows",
            "now": "2026-10-17T12:00:00.000Z",
            "utcOffsetMinutes": 2147483647,
        }));
        assert_eq!(refused["ok"], false, "{refused}");
    }

    /// A callout picks its side and aims its arrow, through the door (task `011080d4`).
    #[test]
    fn o11080d4_a_callout_places_itself_through_the_door() {
        let place = |anchor_left: f64, anchor_top: f64| {
            answer(json!({
                "kind": "calloutPlacement",
                "anchor": { "left": anchor_left, "top": anchor_top, "width": 320.0, "height": 64.0 },
                "viewport": { "left": 0.0, "top": 0.0, "width": 1400.0, "height": 900.0 },
                "width": 420.0,
                "height": 520.0,
            }))["value"]
                .clone()
        };

        // A column on the left: the room is to its right.
        let right = place(40.0, 300.0);
        assert_eq!(right["side"], "right", "{right}");
        assert_eq!(right["left"], 360.0, "on the anchor's edge (AITD-679)");

        // A column on the right: it opens the other way.
        let left = place(1000.0, 300.0);
        assert_eq!(left["side"], "left", "{left}");

        // The arrow points at the row's middle, which for an unclamped callout is its half-height.
        assert_eq!(right["arrowOffset"], 260.0);
        assert_eq!(
            right["top"].as_f64().unwrap() + right["arrowOffset"].as_f64().unwrap(),
            332.0,
            "the row's middle"
        );

        // A row at the top pushes the callout down and walks the arrow UP its edge to keep
        // pointing at it -- the case a client gets wrong by drawing the arrow at the midpoint.
        let high = place(40.0, 10.0);
        assert_eq!(high["top"], 8.0);
        assert_eq!(
            high["top"].as_f64().unwrap() + high["arrowOffset"].as_f64().unwrap(),
            42.0,
            "still the row's middle"
        );
    }

    /// The row a plural key's translation is read from, through the door (task `bcc8a11e`).
    ///
    /// `astrid_rules::plurals::category` was reachable only by a client that links Rust, so the
    /// WinUI shell could not adopt the catalog's plural keys and kept `count == 1` arms in C#.
    #[test]
    fn bcc8a11e_a_json_door_client_can_pick_a_plural_row() {
        let category = |locale: &str, form: &str, count: i64| {
            answer(json!({
                "kind": "pluralCategory",
                "locale": locale,
                "form": form,
                "count": count,
            }))["value"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| panic!("{locale}/{form}/{count} was refused"))
        };

        // The pair that shows why `form` cannot be inferred from the number: the same 3, in the
        // same language, is a different row depending on what is being counted.
        assert_eq!(category("en", "cardinal", 3), "other", "3 weeks");
        assert_eq!(category("en", "ordinal", 3), "few", "the 3rd");
        assert_eq!(category("en", "cardinal", 1), "one");
        assert_eq!(category("en", "ordinal", 1), "one", "the 1st");

        // The language subtag decides, so a regional tag is answered rather than falling through.
        assert_eq!(category("de-DE", "cardinal", 1), "one");
        assert_eq!(category("de-DE", "cardinal", 2), "other");

        // One form for the whole language.
        assert_eq!(category("ja", "cardinal", 7), "other");

        // A tag this build has never met is a client newer than this core: a word, not a crash.
        assert_eq!(category("xx-YY", "cardinal", 2), "other");

        // CLDR's `n` operand is the absolute value. No repeat makes one, but "the -1st" is worse.
        assert_eq!(category("en", "ordinal", -1), "one");

        // A form this build does not know IS refused, because guessing it would answer the wrong
        // question rather than the same question in an unknown language.
        let refused = answer(json!({
            "kind": "pluralCategory", "locale": "en", "form": "fractional", "count": 1
        }));
        assert_eq!(refused["ok"], false, "{refused}");
    }

    /// The four thin doors this task opened over rules that already existed.
    #[test]
    fn c2109ff8_the_remaining_list_rules_are_reachable_without_linking_rust() {
        // The whole palette, not just the scorer: commands, lists and tasks in one answer.
        let palette = answer(json!({
            "kind": "palette",
            "query": "home",
            "lists": [{ "id": "l", "name": "Home" }],
            "tasks": [{ "id": "t", "title": "Buy a home" }],
        }))["value"]
            .clone();
        let kinds: Vec<&str> = palette
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["kind"].as_str().expect("a kind"))
            .collect();
        assert!(kinds.contains(&"list"), "{palette}");
        assert!(kinds.contains(&"task"), "{palette}");

        // The autolinker, which the VS Code extension and web both need and neither could call.
        let links = answer(json!({
            "kind": "identifierLinks",
            "text": "see AWTD-12 and awtd-13",
            "context": { "keys": ["AWTD"] },
        }))["value"]
            .clone();
        assert_eq!(links.as_array().expect("links").len(), 1, "{links}");
        assert_eq!(links[0]["identifier"], "AWTD-12");
        assert_eq!(links[0]["href"], "/t/AWTD-12");

        // A hand-arranged order against the list as it stands now.
        let reconciled = answer(json!({
            "kind": "manualOrder",
            "requested": ["c", "a", "gone"],
            "inListByCreation": ["a", "b", "c"],
        }))["value"]
            .clone();
        assert_eq!(reconciled, json!(["c", "a", "b"]), "{reconciled}");

        // A snooze, and the theme — a read normalises rather than failing, so a value written by
        // a newer build leaves an app that still draws.
        assert_eq!(
            answer(json!({
                "kind": "reminderSnooze",
                "now": "2026-09-09T09:01:00.000Z",
                "minutes": 15,
            }))["value"],
            // `date::format`'s shape, which is what every other timestamp on this door emits.
            "2026-09-09T09:16:00Z"
        );
        assert_eq!(
            answer(json!({ "kind": "theme", "stored": "dark" }))["value"],
            "dark"
        );
        assert_eq!(
            answer(json!({ "kind": "theme", "stored": "plaid" }))["value"],
            "ocean"
        );
        assert_eq!(answer(json!({ "kind": "theme" }))["value"], "ocean");
    }

    /// AWTD2-216 (iOS c8a1ff51, AITD-327/332/555): the detail layout through the door.
    #[test]
    fn awtd2_216_detail_layout_answers_order_and_the_two_rows() {
        let list = answer(json!({
            "kind": "detailLayout", "displayMode": "list", "isInProject": true,
        }))["value"]
            .clone();
        assert_eq!(
            list,
            json!({
                "fieldOrder": ["assignee", "when", "priority", "lists"],
                "showsBoardState": true,
                "showsTaskBlockers": true,
            })
        );
        // Project mode drops who and priority; a read-only viewer sees no state mover, but still
        // sees a blocker that exists.
        let project = answer(json!({
            "kind": "detailLayout", "displayMode": "project",
            "isInProject": true, "isReadOnly": true, "hasBlockers": true,
        }))["value"]
            .clone();
        assert_eq!(
            project,
            json!({
                "fieldOrder": ["when", "lists"],
                "showsBoardState": false,
                "showsTaskBlockers": true,
            })
        );
        let off_board = answer(json!({ "kind": "detailLayout" }))["value"].clone();
        assert_eq!(off_board["showsBoardState"], false);
        assert_eq!(off_board["showsTaskBlockers"], false);
    }

    /// AWTD2-216 (iOS AITD-375, 42013da7): the leading picker's two questions.
    #[test]
    fn awtd2_216_leading_picker_answers_state_and_ownership() {
        let someone_elses = answer(json!({
            "kind": "leadingPicker", "assigneeId": "them", "currentUserId": "me",
        }))["value"]
            .clone();
        assert_eq!(
            someone_elses,
            json!({ "pickerShowsProjectState": true, "isSomeoneElsesTask": true })
        );
        // Unassigned is nobody's, not someone else's.
        let unassigned =
            answer(json!({ "kind": "leadingPicker", "currentUserId": "me" }))["value"].clone();
        assert_eq!(
            unassigned,
            json!({ "pickerShowsProjectState": false, "isSomeoneElsesTask": false })
        );
        let card = answer(json!({
            "kind": "leadingPicker", "surface": "boardCard", "assigneeId": "me",
            "currentUserId": "me",
        }))["value"]
            .clone();
        assert_eq!(card["pickerShowsProjectState"], true);
        assert_eq!(card["isSomeoneElsesTask"], false);
    }

    /// AWTD2-216 (iOS 42013da7): chips skip hidden lists and board columns.
    #[test]
    fn awtd2_216_list_chips_skip_hidden_lists_and_columns() {
        let chips = answer(json!({
            "kind": "listChips",
            "listIds": ["home", "viewed", "col", "gone"],
            "lists": [
                { "id": "home", "name": "Home", "color": "#ff0000" },
                { "id": "viewed", "name": "Viewed" },
                { "id": "col", "name": "Doing", "listType": "status" },
            ],
            "hiddenListIds": ["viewed"],
        }))["value"]
            .clone();
        let ids: Vec<&str> = chips
            .as_array()
            .expect("chips")
            .iter()
            .map(|chip| chip["id"].as_str().expect("id"))
            .collect();
        assert_eq!(ids, ["home"], "{chips}");
        assert_eq!(chips[0]["name"], "Home");
        assert_eq!(chips[0]["glyph"], "hash");
    }

    /// AWTD2-216 (iOS c8a1ff51's neighbour `DueDateLabel.dayOffset`): whole days, in the reader's
    /// zone for a timed date and by calendar date for an all-day one.
    #[test]
    fn awtd2_216_day_offset_counts_days_in_the_readers_zone() {
        let days = |date: &str, all_day: bool, offset: i32| {
            answer(json!({
                "kind": "dayOffset", "date": date, "isAllDay": all_day,
                "now": "2026-10-17T03:00:00.000Z", "utcOffsetMinutes": offset,
            }))["value"]["days"]
                .clone()
        };
        // 03:00 UTC is still the 16th at UTC-8, and so is 02:00 UTC on the 17th: same day.
        assert_eq!(days("2026-10-17T02:00:00.000Z", false, -480), json!(0));
        assert_eq!(days("2026-10-17T02:00:00.000Z", false, 0), json!(0));
        assert_eq!(days("2026-10-18T09:00:00.000Z", false, -480), json!(2));
        // All-day: the UTC midnight is the calendar date, whatever the zone.
        assert_eq!(days("2026-10-17T00:00:00.000Z", true, -480), json!(1));
        assert_eq!(days("2026-10-16T00:00:00.000Z", true, 0), json!(-1));
        let bad = answer(json!({
            "kind": "dayOffset", "date": "2026-10-17T00:00:00.000Z",
            "now": "2026-10-17T00:00:00.000Z", "utcOffsetMinutes": 99999999,
        }));
        assert_ne!(bad["ok"], json!(true), "{bad}");
    }
}
