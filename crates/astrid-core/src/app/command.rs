//! The vocabulary the shell speaks.
//!
//! One enum in, one enum out. Both are `serde`-tagged on `kind`, so the wire form is readable in a
//! log and stable enough to write a C# type against by hand.
//!
//! ## What belongs here
//!
//! A command is something a person did — "complete this task", "show me this list". It is not a
//! step in doing it. `SaveTaskThenRefreshThenScroll` would be the shell deciding, which is the one
//! thing it may not do; the core decides what completing a task entails and the shell asks for the
//! result.
//!
//! ## Failures are values
//!
//! Every failure is a [`Failure`] with a machine-readable kind, because the shell has to tell three
//! situations apart and they look identical in a message string: **sign in again** (the session is
//! gone), **it will send later** (offline, and the Outbox has it), and **that did not work**
//! (the server refused). Showing the second as an error is how a working offline app comes to look
//! broken.

use serde::{Deserialize, Serialize};

use crate::rows::{DisplayMode, Surface};

/// A flag that is on unless the caller says otherwise — `enabled`, most often, where
/// omitting it should mean "yes" rather than silently turning something off.
pub(crate) fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
// `rename_all` names the variants, `rename_all_fields` names the fields inside them. Both are
// needed: without the second, `{"kind":"list","listId":"l1"}` reads as a command with no id and
// every screen that uses it fails as "bad request" with nothing to say why.
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Command {
    // ── Reads. These never touch the network. ────────────────────────────────────────────────
    /// Every list, for the sidebar.
    Lists,
    /// One list.
    List {
        list_id: String,
    },
    /// The rows to draw for a list, already filtered, sorted, spliced and projected.
    ///
    /// One command rather than "give me the tasks, and here is how I will filter them": the
    /// filtering, the sort, the subtask splice and the row projection are four contracts, and a
    /// shell that ran them itself would be four chances to disagree with web.
    RowsForList {
        list_id: String,
        #[serde(default)]
        display_mode: Option<String>,
        #[serde(default)]
        surface: Option<String>,
        /// Where to start, for a virtualised list. Absent means the beginning.
        #[serde(default)]
        offset: Option<usize>,
        /// How many rows are wanted. Absent means all of them — which is the right answer for a
        /// list of forty and the wrong one for a list of ten thousand.
        #[serde(default)]
        limit: Option<usize>,
    },
    /// One task in full, for the detail view.
    Task {
        task_id: String,
    },
    /// Everything one task's detail screen needs: the task, its comments, its subtasks, its list
    /// chips, and the order to lay the fields out in.
    ///
    /// One command rather than five, because the field order is a cross-platform product decision
    /// and a shell that assembled the screen itself would be the fifth place to get it wrong.
    TaskDetail {
        task_id: String,
        #[serde(default)]
        display_mode: Option<String>,
    },
    /// The quick date and time choices for a task, with the instant each one means and which
    /// one it is already set to.
    ///
    /// Resolved here rather than in the shell because the arithmetic is the part that goes wrong:
    /// a day is 23 or 25 hours across a daylight-saving boundary, an all-day date is stored
    /// differently from a timed one, and "morning" means 09:00 where the reader is rather than in
    /// UTC. Three clients read the same list in the same order — see `astrid_core::rows::due_picks`.
    /// What a chosen calendar day means for this task, as an instant.
    ///
    /// A calendar hands back a DAY; whether that is a date or a date-and-time depends on the task,
    /// and an all-day date written as a local midnight reads back as the day before west of UTC.
    /// So the shell asks rather than computes — the same reason the quick picks carry their
    /// instants. See `rows::due_picks::on_day`.
    DueDateOnDay {
        task_id: String,
        /// The calendar day the reader clicked, as `YYYY-MM-DD`.
        day: String,
    },
    DueDateOptions {
        task_id: String,
    },
    /// A task's comments.
    Comments {
        task_id: String,
    },
    /// Find a task by what it says.
    ///
    /// Over the cache, which is where every client searches: there is no server search endpoint,
    /// and the Apple service's "online" path reads the same cached array its offline path does.
    /// So this is instant, works on a train, and is the same set of rules everywhere.
    SearchTasks {
        query: String,
        #[serde(default)]
        list_id: Option<String>,
        #[serde(default)]
        include_completed: Option<bool>,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// The board a list belongs to: its columns, and the cards in each.
    ///
    /// Answers with rows, so a card draws like a row — see [`crate::board`] for why a column id is
    /// a role rather than a list id.
    Board {
        list_id: String,
        /// How many cards to carry per column. The count comes back whole.
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Move a card to a column.
    MoveTaskToColumn {
        task_id: String,
        column_id: String,
        /// The list the board was opened from, so the move knows which project's columns to
        /// resolve the target against.
        list_id: String,
    },
    /// When to be reminded about one task, as offsets from its due time.
    ReminderOptions {
        task_id: String,
    },
    /// Which reminders have come due and have not been shown yet.
    ///
    /// Read while the app is running. The server owns push and email; this is only what a running
    /// client can notice about a reminder whose time has arrived — see [`crate::reminders`].
    RemindersDue,
    /// Remember that a reminder was shown, so it is not shown again.
    ReminderShown {
        task_id: String,
    },
    /// Move a reminder forward and let it be shown again when it arrives.
    SnoozeReminder {
        task_id: String,
        minutes: i64,
    },
    /// The repeat presets, and how this task's repeat describes itself.
    ///
    /// The summary comes back as parts with resource keys rather than a sentence — see
    /// [`crate::rows::repeat`], where the reason is written down.
    RepeatOptions {
        task_id: String,
    },
    /// Who this task can be assigned to, in the order the picker shows them.
    ///
    /// One rule for every surface: the detail pane, the row picker and later the board all ask
    /// this. On iOS the picker built the list inline, and the board could not offer an AI agent
    /// at all — see [`crate::rows::assignee`].
    AssigneeOptions {
        task_id: String,
    },
    /// The signed-in user.
    CurrentUser,
    /// Whether there is a stored session. Not whether it is still valid — only the server knows
    /// that, and it says so with a 401.
    IsSignedIn,
    /// The Outbox's state, for the "not synced yet" indicator.
    OutboxStats,
    /// What a pressed key means, given what is on screen.
    ///
    /// The shell asks rather than knowing, because the bare-key scheme is a cross-platform
    /// contract locked by `contracts/fixtures/shortcuts.json` — including the part that is easiest
    /// to get wrong, which is *when a key is allowed to fire at all*. A shell that dispatched from
    /// its own table would drift from web the first time somebody added a shortcut there.
    ///
    /// Pure: no cache, no network, no clock. It is safe to ask on the UI thread while a key is
    /// being handled.
    ResolveShortcut {
        key: String,
        #[serde(default)]
        has_selection: bool,
        #[serde(default)]
        is_text_field_focused: bool,
        #[serde(default)]
        is_modal_presented: bool,
    },
    /// The whole scheme, for a shortcuts sheet.
    Shortcuts,

    // ── Writes. These update the cache and journal the change. ───────────────────────────────
    CreateTask {
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        list_ids: Vec<String>,
        #[serde(default)]
        priority: Option<i64>,
        #[serde(default)]
        due_date_time: Option<String>,
        #[serde(default)]
        is_all_day: Option<bool>,
        #[serde(default)]
        assignee_id: Option<String>,
        #[serde(default)]
        parent_task_id: Option<String>,
        /// True when the title was typed into the quick-add box, where the web's smart parsing
        /// reads `#list` tags out of it when the account has it on (task 6ac2639a). A subtask
        /// typed into a detail is not parsed, as it is not on the web.
        #[serde(default)]
        quick_add: bool,
    },
    /// Edit a task. The body is the same shape the API takes, so a field the shell learns about
    /// needs no change here — see [`crate::services::TaskChanges`] for how absent and null differ.
    UpdateTask {
        task_id: String,
        changes: serde_json::Value,
    },
    /// Complete or un-complete a task. **The only way to do either.**
    CompleteTask {
        task_id: String,
        completed: bool,
    },
    DeleteTask {
        task_id: String,
    },
    /// Close a task as something other than done — `canceled` | `duplicate` | `not_planned` — or
    /// reopen it with `null` (task 016ce981). Not a way to complete one: a canceled close does not
    /// roll a repeating task forward, on purpose.
    SetClosedReason {
        task_id: String,
        #[serde(default)]
        closed_reason: Option<String>,
    },
    /// The board columns a task's menu can put it in, and which one it is in now. The project's
    /// own columns when the task is in a project, the defaults every board shares when not.
    TaskStatusOptions {
        task_id: String,
    },
    /// Put a task in a column by id from its menu: the same move a dragged card makes, so the two
    /// cannot disagree — including that Done means completed.
    SetTaskStatus {
        task_id: String,
        column_id: String,
    },
    /// Mint a link to the task that other people can open, as the web's Share does. Online-only.
    ShareTask {
        task_id: String,
    },
    SetTaskLists {
        task_id: String,
        list_ids: Vec<String>,
    },
    /// The lists a task can be put in, for the detail's list editor (task d3f3b111): the ones it
    /// is in, the ones that match what was typed, and whether to offer creating what was typed.
    ListPicks {
        task_id: String,
        #[serde(default)]
        query: String,
    },
    AddTaskToList {
        task_id: String,
        list_id: String,
    },
    RemoveTaskFromList {
        task_id: String,
        list_id: String,
    },
    /// Make a list and put the task in it, in one step — the editor's **Create "…"**. The new
    /// list's colour and privacy are the core's decision, see `rows::list_picks`.
    CreateListForTask {
        task_id: String,
        name: String,
    },
    /// A board's columns (task e5214fba), addressed by the list the board was opened from. The
    /// rules are `crate::board`, locked against the web; a refused write is a bad request with
    /// the web's own message.
    AddBoardStatus {
        list_id: String,
        name: String,
    },
    RenameBoardStatus {
        list_id: String,
        role: String,
        name: String,
    },
    ReorderBoardStatus {
        list_id: String,
        role: String,
        /// `up` or `down`.
        direction: String,
    },
    RemoveBoardStatus {
        list_id: String,
        role: String,
    },
    /// What a list's coding-agent settings can be set to (task f44b4a0c): the agents this
    /// account may put on a list, and the repositories its GitHub connection can reach.
    ListAgentOptions {
        list_id: String,
    },
    SetTaskStatusRole {
        task_id: String,
        #[serde(default)]
        status_role: Option<String>,
    },
    CreateList {
        name: String,
        #[serde(default)]
        color: Option<String>,
    },
    UpdateList {
        list_id: String,
        changes: serde_json::Value,
    },
    DeleteList {
        list_id: String,
    },
    SetListFavorite {
        list_id: String,
        favorite: bool,
    },
    PostComment {
        task_id: String,
        content: String,
        /// The comment this one answers (task 97c817dd); absent for a top-level comment.
        #[serde(default)]
        parent_comment_id: Option<String>,
    },
    /// Change what a comment says. Offline through the Outbox, like posting one.
    EditComment {
        comment_id: String,
        content: String,
    },
    /// What the comment box's popup should show for this text and caret (task 3271a0c5): the
    /// trigger the caret is inside, if any, and the rows for it. Positions are UTF-16 units.
    CommentSuggestions {
        task_id: String,
        text: String,
        caret: usize,
    },
    /// Put a chosen row into the text, the way the server will read it back. The row's kind is
    /// `triggerKind`, because `kind` on the wire is the command itself.
    ApplyCommentSuggestion {
        text: String,
        caret: usize,
        trigger_kind: crate::parse::mentions::TriggerKind,
        id: String,
        label: String,
    },
    DeleteComment {
        comment_id: String,
    },

    // ── Things that need the network by their nature. ────────────────────────────────────────
    /// One sync pass: push, fetch, apply.
    Sync,
    /// Drain the Outbox without fetching. What a "retry now" button does.
    Drain,
    /// Refresh a task's comments from the server.
    RefreshComments {
        task_id: String,
    },
    /// Search for people to assign or invite.
    SearchUsers {
        query: String,
    },
    /// The Agent Hub: every agent, the mode it is set to, and whether it has what it needs.
    ///
    /// One command, like the external-sync panel, because a screen that drew the agents before it
    /// knew which had a credential would show "needs setup" on all of them and then correct itself.
    Agents,
    /// Where an account's own agent is told about work.
    ///
    /// Answers for an account that has never configured one: the screen builds its event and agent
    /// pickers from this, so there is something to answer with before there is anything to set.
    WebhookSettings,
    /// Save where deliveries go, and what is delivered.
    SaveWebhook {
        url: String,
        #[serde(default = "crate::app::command::yes")]
        enabled: bool,
        #[serde(default)]
        events: Vec<String>,
        #[serde(default)]
        agents: Vec<String>,
        /// Ask for a new signing secret. The server answers with it once and never again.
        #[serde(default)]
        regenerate_secret: bool,
    },
    DeleteWebhook,
    /// Fire a `test.ping` at the configured URL.
    ///
    /// The only way to know a webhook works: the URL is somebody else's server, and one nothing
    /// has ever reached is a setting that looks configured and is not.
    TestWebhook,
    /// The client-credentials pairs this account has registered.
    ///
    /// Read-only, so a panel can be opened without minting anything. Nothing here answers with a
    /// secret: the server stores a hash and shows plaintext once, at creation.
    ApiAccess,
    /// Mint an MCP token for this device, or hand back the one it already has.
    ///
    /// The server decides which. Asking twice gives the same token rather than a second one, so a
    /// screen that lost its copy can get it back without revoking anything.
    CreateMcpToken,
    /// Revoke every MCP token minted from a device. All of them: the endpoint takes no id.
    RevokeMcpTokens,
    /// Register a client-credentials pair, and answer with the secret shown only this once.
    CreateOAuthClient {
        name: String,
    },
    /// Revoke one pair, addressed by its public half.
    DeleteOAuthClient {
        client_id: String,
    },
    /// The agents this account has registered of its own.
    CustomAgents,
    /// Register one. Answers with the credentials the server will show only this once.
    RegisterCustomAgent {
        name: String,
        /// What the agent may see. Absent is every list this account has, which is a bigger grant
        /// than most people mean.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        list_ids: Option<Vec<String>>,
    },
    DeleteCustomAgent {
        agent_id: String,
    },
    /// Start connecting Copilot, or stop.
    ///
    /// The same browser hand-off as every other provider: somebody's GitHub password belongs in
    /// their browser.
    ConnectCopilot,
    DisconnectCopilot,
    /// Change how one agent runs.
    SetAgentMode {
        agent: String,
        mode: crate::services::AgentMode,
    },
    /// Store a key for one service. Straight to the server; never cached here.
    SaveAgentCredential {
        service_id: String,
        key: String,
    },
    /// Ask the server whether a stored key works.
    TestAgentCredential {
        service_id: String,
    },
    DeleteAgentCredential {
        service_id: String,
    },
    /// What a list's external sync looks like: what is connected, what it could be linked to,
    /// and what it is linked to.
    ///
    /// One command rather than three, because the screen needs all of it before it can draw a
    /// single row — and three round trips to fill one panel is three chances to show it half done.
    ExternalSync {
        list_id: String,
    },
    /// The URL to open in a browser to connect a provider.
    ConnectProvider {
        provider: crate::services::Provider,
    },
    DisconnectProvider {
        provider: crate::services::Provider,
    },
    /// Mirror a list to a container on the other side.
    LinkList {
        provider: crate::services::Provider,
        list_id: String,
        container_id: String,
    },
    UnlinkList {
        provider: crate::services::Provider,
        link_id: String,
    },
    /// How Google lists get linked, and what a list made here is called.
    GoogleSyncMode,
    /// How Google lists get linked: one at a time by hand, or all of them.
    ///
    /// The choice is the account's, not this machine's, so it follows somebody to their laptop.
    SetGoogleSyncMode {
        mode: crate::external::auto_link::SyncMode,
        /// Appended to the name of a list made here for a remote one. Absent leaves it alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suffix: Option<String>,
    },
    /// Run one Google pass over every linked list.
    ///
    /// GitHub needs no equivalent: a cron on the server does that one, so a list linked here syncs
    /// whether or not this app is running.
    SyncExternal,
    /// Whether the first-run tour has been seen on this machine.
    ///
    /// In the cache rather than in the account: it is about this installation — where its hotkey
    /// is, what its palette does — and somebody who has used the app for a year on a laptop still
    /// wants to be told those things the first time they open it on a desktop.
    HasSeenTour,
    /// Remember that the tour has been seen.
    TourSeen,
    /// Everything one box can find: commands, lists, tasks, ranked.
    ///
    /// The matcher is the Mac's, character for character — see [`crate::palette`]. The ranking is
    /// what a palette *is*, and two clients that rank differently are two products.
    Palette {
        #[serde(default)]
        query: String,
    },
    /// The three numbers on the signed-in account's profile.
    ProfileStats,
    /// Write everything this account has to a file on this machine.
    ///
    /// `format` is `json` or `csv`. The path comes from the shell's save dialog, because choosing
    /// where a file goes is the one part of this the core should not decide.
    ExportAccount {
        format: String,
        path: String,
    },
    /// Change the signed-in user's name, photo, or both (task 19fd9289). `photoPath` is a file on
    /// this machine, uploaded first. The answer is the account screen, redrawn.
    UpdateProfile {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        photo_path: Option<String>,
    },
    /// Send the verification email again. Answers with the server's message.
    ResendVerification,
    /// The contacts this account imported for collaborator suggestions (task 438494c7). Online-only.
    Contacts,
    /// Remove every imported contact. Answers with how many went.
    ClearContacts,
    /// Delete the account for good. `confirmation` must be the phrase the web requires, typed
    /// exactly; anything else is refused without a request. Signs out afterwards.
    DeleteAccount {
        confirmation: String,
    },
    /// The account: who is signed in, and what their reminder settings are.
    ///
    /// From the cache, so the screen draws instantly. [`Command::RefreshSettings`] catches it up.
    Settings,
    /// Fetch the account and its settings from the server.
    RefreshSettings,
    /// Change the reminder settings.
    ///
    /// The fields are the server's — `enablePushReminders`, `dailyDigestTime`, and so on — merged
    /// into what is already stored, so a screen can send one toggle without restating the rest.
    UpdateReminderSettings {
        changes: serde_json::Value,
    },
    /// Change the account's task defaults or its task-detail layout (task c0f3db19). The fields
    /// are the server's — `emailToTaskEnabled`, `defaultTaskDueOffset`, `defaultDueTime`,
    /// `taskDisplayMode`, `subtaskDisplay`, `smartTaskCreationEnabled` — merged into what is
    /// stored, and refused here when the server would refuse them. Answers with the account screen.
    UpdateSmartTaskSettings {
        changes: serde_json::Value,
    },
    /// Start timing a task.
    ///
    /// The start time goes in the cache rather than in memory, so a timer survives a restart — on
    /// Apple it does not, and a timer left running an hour ago is simply lost.
    StartTimer {
        task_id: String,
    },
    /// Stop timing, and record what the session was worth.
    StopTimer {
        task_id: String,
    },
    /// The files on a task: its own, and its comments'.
    ///
    /// There is no "attach to task" endpoint anywhere — a file reaches a task by being uploaded and
    /// then named by a comment — so this is the only way to answer "what is attached to this?".
    Attachments {
        task_id: String,
    },
    /// Fetch a file's bytes and answer with the path they were written to.
    DownloadAttachment {
        task_id: String,
        file_id: String,
    },
    /// Attach a file from this machine, and post the comment that carries it.
    ///
    /// Works offline like everything else. The bytes are copied into a pending directory and the
    /// journal row names the copy — a journal holding a photograph is a journal nobody can read.
    /// See [`crate::services::attachment`].
    AttachFile {
        task_id: String,
        /// A path on this machine, from the file picker.
        path: String,
        #[serde(default)]
        content: Option<String>,
    },
    /// Which look the app wears, and the ones it could.
    ///
    /// Per installation rather than per account — a laptop in the evening and a desktop under an
    /// office light are different questions. See [`crate::theme`].
    Theme,
    SetTheme {
        theme: crate::theme::Theme,
    },
    /// The My Tasks entry for the sidebar: the view the app opens on.
    MyTasksList,
    /// What My Tasks is filtered and sorted by.
    ///
    /// From the cache, so a screen draws before the network answers; [`Command::RefreshMyTasks`]
    /// is the catch-up.
    MyTasksFilters,
    /// Fetch My Tasks' filters from the account.
    RefreshMyTasks,
    /// Change what My Tasks is filtered and sorted by, for this account on every device.
    SetMyTasksFilters {
        #[serde(flatten)]
        filters: crate::filters::my_tasks::Preferences,
    },
    /// What is on the clipboard, and which of it somebody meant to attach.
    ///
    /// Reading the clipboard is the shell's job; deciding what it means is not — see
    /// [`crate::paste`] for why files beat a rendition of them and why text is left alone.
    ClipboardPaste {
        /// Files the board names on disk, in the order it lists them.
        #[serde(default)]
        files: Vec<String>,
        /// The format of an image with no file behind it. Absent when there is no image.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_extension: Option<String>,
        #[serde(default)]
        has_text: bool,
    },
    /// What a list is filtered and sorted by, and what else it could be.
    ///
    /// Answers for My Tasks too, from the account's preferences rather than a list row.
    FilterOptions {
        list_id: String,
    },
    /// Set one filter on one list.
    ///
    /// A separate command rather than an `updateList` from the shell, because where a filter is
    /// written depends on what is being filtered: a list's go on the list, My Tasks' go on the
    /// account. That is a decision, and decisions are not the shell's.
    SetFilter {
        list_id: String,
        /// The field the group names — `filterCompletion`, `sortBy`, and so on.
        field: String,
        value: String,
    },
    /// A list's chat, from the cache.
    ///
    /// Answers with the channel and the transcript projected — see [`crate::rows::chat`] for what
    /// "mine", "sending" and "nobody said it" mean and why the shell is not asked to decide them.
    Chat {
        list_id: String,
    },
    /// Fetch the channels and then this list's messages.
    ///
    /// Separate from [`Command::Chat`] because opening a conversation should draw instantly from
    /// the cache and catch up afterwards, the same order the task list uses.
    RefreshChat {
        list_id: String,
    },
    /// Say something. In the transcript before this returns, whatever the network is doing.
    SendChatMessage {
        channel_id: String,
        content: String,
        #[serde(default)]
        reply_to_id: Option<String>,
    },
    /// Who a list is shared with, and what this account may do about it.
    ///
    /// Reaches the network: membership is not a local fact, and a cached member list that is a day
    /// old is how somebody removed last week is still offered a role.
    ListMembers {
        list_id: String,
    },
    /// Invite somebody by email.
    InviteToList {
        list_id: String,
        email: String,
        role: String,
    },
    SetMemberRole {
        list_id: String,
        user_id: String,
        role: String,
    },
    RemoveMember {
        list_id: String,
        user_id: String,
    },
    /// Leave a list somebody else owns.
    LeaveList {
        list_id: String,
    },
    /// Fetch what the deployment supports.
    RefreshCapabilities,
    /// Start signing in. Answers with the URL for the shell to open in the browser.
    BeginSignIn,
    /// Finish signing in, from the URL Windows activated the app with.
    CompleteSignIn {
        callback_url: String,
    },
    /// Abandon the sign-in in progress — the user closed the browser prompt.
    CancelSignIn,
    /// Forget everything: the cache, the journal, the credential.
    SignOut,
}

impl Command {
    /// The display mode a row command asked for.
    pub fn display_mode(stored: Option<&str>) -> DisplayMode {
        DisplayMode::from_stored(stored)
    }

    /// Which surface a row command is drawing.
    ///
    /// Unrecognised means a list row, the commonest surface — and the one whose behaviour is least
    /// surprising if a newer shell asks for something this build has not heard of.
    pub fn surface(named: Option<&str>) -> Surface {
        match named {
            Some("boardCard") => Surface::BoardCard,
            Some("detail") => Surface::Detail,
            _ => Surface::ListRow,
        }
    }
}

/// What kind of failure it was. The part the shell branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// The shell sent something this build cannot read.
    BadRequest,
    /// The session is gone. Sign in again — retrying will not help.
    Unauthorized,
    /// The server considered it and said no.
    Refused,
    /// It did not reach the server. **Not necessarily a failure**: a write is already journalled
    /// and will go when the network does. The shell shows this as "offline", not as an error.
    Offline,
    /// The thing being acted on is not here.
    NotFound,
    /// The cache could not be read or written.
    Cache,
}

/// Why a command did not work.
///
/// One shape for every failure — a kind, a message, and the two optional details that some kinds
/// carry — rather than a tagged union whose payload differs per case. The shell reads this in C#,
/// where a shape that changes per variant is a `switch` over `JsonElement` at every call site.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub kind: FailureKind,
    /// For a person to read, and for a log. Never the thing to branch on.
    pub message: String,
    /// The HTTP status, when the server gave one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// What was not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl Failure {
    fn of(kind: FailureKind, message: impl Into<String>) -> Self {
        Failure {
            kind,
            message: message.into(),
            status: None,
            id: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::of(FailureKind::BadRequest, message)
    }

    pub fn unauthorized() -> Self {
        Self::of(FailureKind::Unauthorized, "the session is not valid")
    }

    pub fn refused(status: u16, message: impl Into<String>) -> Self {
        Failure {
            status: Some(status),
            ..Self::of(FailureKind::Refused, message)
        }
    }

    pub fn offline(message: impl Into<String>) -> Self {
        Self::of(FailureKind::Offline, message)
    }

    pub fn not_found(what: &str, id: impl Into<String>) -> Self {
        let id = id.into();
        Failure {
            id: Some(id.clone()),
            ..Self::of(FailureKind::NotFound, format!("no {what} with id {id}"))
        }
    }

    pub fn cache(message: impl Into<String>) -> Self {
        Self::of(FailureKind::Cache, message)
    }

    /// Whether this means "sign in again".
    pub fn needs_sign_in(&self) -> bool {
        self.kind == FailureKind::Unauthorized
    }

    /// Whether the work is still going to happen. An offline write is in the journal; showing it
    /// as a failure is how a working offline app comes to look broken.
    pub fn is_still_pending(&self) -> bool {
        self.kind == FailureKind::Offline
    }
}

impl From<crate::services::ServiceError> for Failure {
    fn from(error: crate::services::ServiceError) -> Self {
        use crate::api::ApiError;
        use crate::services::ServiceError;
        match error {
            ServiceError::Api(ApiError::Unauthorized) => Failure::unauthorized(),
            ServiceError::Api(ApiError::Http { status, message }) => {
                Failure::refused(status, message)
            }
            ServiceError::Api(ApiError::Transport(error)) => Failure::offline(error.to_string()),
            // A decode failure or a refused path: the server was reached, or would have been, and
            // trying again unchanged will not help — which is what `refused` means to the shell.
            ServiceError::Api(error) => Failure::bad_request(error.to_string()),
            ServiceError::Store(error) => Failure::cache(error.to_string()),
            // A file on this machine: the same shape of problem as the cache, and the same thing
            // for the shell to do about it, which is say so rather than retry.
            ServiceError::LocalFile(error) => Failure::cache(error),
            ServiceError::NotFound { kind, id } => Failure::not_found(kind, id),
        }
    }
}

impl From<crate::store::StoreError> for Failure {
    fn from(error: crate::store::StoreError) -> Self {
        Failure::cache(error.to_string())
    }
}

/// What a command answered.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
}

impl Response {
    pub fn ok(value: impl Serialize) -> Self {
        Response {
            ok: true,
            value: serde_json::to_value(value).ok(),
            error: None,
        }
    }

    pub fn done() -> Self {
        Response {
            ok: true,
            value: None,
            error: None,
        }
    }

    pub fn failed(failure: Failure) -> Self {
        Response {
            ok: false,
            value: None,
            error: Some(failure),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|error| {
            // Serialising a response cannot normally fail. If it somehow does, the shell still has
            // to get an answer it can read, or it waits forever on a call that already finished.
            format!(
                "{{\"ok\":false,\"error\":{{\"kind\":\"cache\",\"0\":{}}}}}",
                serde_json::Value::String(error.to_string())
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_reads_from_the_shape_the_shell_sends() {
        let command: Command =
            serde_json::from_str(r#"{"kind":"completeTask","taskId":"t1","completed":true}"#)
                .expect("decodes");
        assert!(matches!(
            command,
            Command::CompleteTask {
                completed: true,
                ..
            }
        ));
    }

    /// The three situations the shell has to tell apart, and which look identical in a message.
    #[test]
    fn the_failures_that_mean_different_things_can_be_told_apart() {
        assert!(Failure::unauthorized().needs_sign_in());
        assert!(!Failure::unauthorized().is_still_pending());

        let offline = Failure::offline("dns");
        assert!(offline.is_still_pending());
        assert!(!offline.needs_sign_in());

        let refused = Failure::refused(422, "no");
        assert!(!refused.is_still_pending());
        assert!(!refused.needs_sign_in());
        assert_eq!(refused.status, Some(422));
    }

    #[test]
    fn a_failure_carries_its_kind_where_the_shell_can_read_it() {
        let json = Response::failed(Failure::unauthorized()).to_json();
        assert!(json.contains(r#""ok":false"#));
        assert!(json.contains(r#""kind":"unauthorized""#));
    }

    /// What was not found is named, so the shell can say which thing rather than "something".
    #[test]
    fn a_not_found_failure_names_the_thing() {
        let failure = Failure::not_found("list", "l9");
        assert_eq!(failure.id.as_deref(), Some("l9"));
        assert!(failure.message.contains("list"));
    }

    #[test]
    fn a_successful_command_with_nothing_to_return_still_says_ok() {
        let json = Response::done().to_json();
        assert_eq!(json, r#"{"ok":true}"#);
    }

    /// A surface a newer shell knows about must not turn every row into a board card.
    #[test]
    fn an_unknown_surface_is_a_list_row() {
        assert_eq!(Command::surface(None), Surface::ListRow);
        assert_eq!(Command::surface(Some("somethingLater")), Surface::ListRow);
        assert_eq!(Command::surface(Some("boardCard")), Surface::BoardCard);
        assert_eq!(Command::surface(Some("detail")), Surface::Detail);
    }
}
