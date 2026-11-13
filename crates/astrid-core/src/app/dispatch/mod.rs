//! Where a command becomes work.
//!
//! [`run`] is a router and nothing else: it names the domain each `Command` belongs to and hands
//! the command there whole. The arms live in the domain module beside this file, so the file you
//! open to change what a command does is the one named after the thing it is about (task
//! 8a91c5b0, which found 203 arms and 1,571 lines here).
//!
//! **The router is ONE exhaustive `match`**, and that is the whole reason it is worth having. The
//! compiler refuses a new `Command` variant no domain claims, and refuses one two domains claim.
//! A chain of `Option`-returning handlers, each answering "mine?", would have read more naturally
//! and moved that check to runtime — where a forgotten command is a panic or a silent refusal in
//! front of somebody instead of a build that does not finish.
//!
//! Every arm is still short on purpose: it names a service call, and the service decides. An arm
//! that grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file where
//! nobody would look for it.

use super::command::{Command, Failure, Response, WaitOptions};
use super::App;
use crate::filters;
use crate::model::date;
use crate::rows::{self, RowContext, TaskRow};
use crate::services::{TaskChanges, TaskDraft};

pub(crate) async fn run(app: &App, command: Command) -> Response {
    match command {
        // ── Tasks ──
        command @ (Command::ToggleAssignee { .. }
        | Command::NestTask { .. }
        | Command::Tasks { .. }
        | Command::ResolveIds { .. }
        | Command::Task { .. }
        | Command::TaskDetail { .. }
        | Command::DueDateOptions { .. }
        | Command::DueDateOnDay { .. }
        | Command::DueDateAtTime { .. }
        | Command::DueDateAllDay { .. }
        | Command::TaskBlockers { .. }
        | Command::TaskProjectFields { .. }
        | Command::SetTaskProjectField { .. }
        | Command::AddTaskBlocker { .. }
        | Command::RemoveTaskBlocker { .. }
        | Command::TaskBlockerCandidates { .. }
        | Command::SearchTasks { .. }
        | Command::RepeatOptions { .. }
        | Command::AssigneeOptions { .. }
        | Command::QuickAddDefaults { .. }
        | Command::CreateTask { .. }
        | Command::UpdateTask { .. }
        | Command::CompleteTask { .. }
        | Command::DeleteTask { .. }
        | Command::SetClosedReason { .. }
        | Command::ShareTask { .. }
        | Command::CopyTask { .. }
        | Command::SetTaskLists { .. }
        | Command::ListPicks { .. }
        | Command::AddTaskToList { .. }
        | Command::RemoveTaskFromList { .. }
        | Command::CreateListForTask { .. }
        | Command::SetTaskStatusRole { .. }
        | Command::StartTimer { .. }
        | Command::StopTimer { .. }) => tasks::run(app, command).await,
        // ── Rows, counts and badges ──
        command @ (Command::RowsForList { .. }
        | Command::ListCounts { .. }
        | Command::ReminderTriageDeck { .. }
        | Command::FeaturedRows { .. }
        | Command::ListBadges { .. }
        | Command::Sidebar { .. }
        | Command::ProfileTasks { .. }) => row_json::run(app, command).await,
        // ── Lists, sharing and membership ──
        command @ (Command::Lists
        | Command::List { .. }
        | Command::ShareList { .. }
        | Command::ResolveShortcode { .. }
        | Command::ListAgentOptions { .. }
        | Command::ListInsights { .. }
        | Command::CreateList { .. }
        | Command::UpdateList { .. }
        | Command::DeleteList { .. }
        | Command::SetListFavorite { .. }
        | Command::SetManualOrder { .. }
        | Command::SetListImage { .. }
        | Command::ListImage { .. }
        | Command::PublicLists
        | Command::CopyList { .. }
        | Command::PublicList { .. }
        | Command::MyTasksList
        | Command::ListMembers { .. }
        | Command::EligibleNewOwners { .. }
        | Command::TransferListOwnership { .. }
        | Command::RefreshListMembers { .. }
        | Command::InviteToList { .. }
        | Command::SetMemberRole { .. }
        | Command::RemoveMember { .. }
        | Command::CancelInvitation { .. }
        | Command::SetInvitationRole { .. }
        | Command::LeaveList { .. }) => lists::run(app, command).await,
        // ── Boards, statuses and projects ──
        command @ (Command::Board { .. }
        | Command::DropBoardCard { .. }
        | Command::MoveTaskToColumn { .. }
        | Command::AddBoardCard { .. }
        | Command::TaskStatusOptions { .. }
        | Command::SetTaskStatus { .. }
        | Command::AddBoardStatus { .. }
        | Command::RenameBoardStatus { .. }
        | Command::ReorderBoardStatus { .. }
        | Command::RemoveBoardStatus { .. }
        | Command::Projects
        | Command::CreateProject { .. }
        | Command::CreateBoardForList { .. }
        | Command::DeleteProject { .. }) => board::run(app, command).await,
        // ── Comments ──
        command @ (Command::Comments { .. }
        | Command::PostComment { .. }
        | Command::EditComment { .. }
        | Command::CommentSuggestions { .. }
        | Command::ApplyCommentSuggestion { .. }
        | Command::DeleteComment { .. }
        | Command::RefreshComments { .. }) => comments::run(app, command).await,
        // ── Attachments and the clipboard ──
        command @ (Command::Attachments { .. }
        | Command::DownloadAttachment { .. }
        | Command::AttachFile { .. }
        | Command::ClipboardPaste { .. }) => attachments::run(app, command).await,
        // ── Chat ──
        command @ (Command::Chat { .. }
        | Command::RefreshChat { .. }
        | Command::SendChatMessage { .. }
        | Command::ResolveChatChannel { .. }
        | Command::ChatMessages { .. }
        | Command::LoadChatMessages { .. }
        | Command::ForgetChatMessage { .. }
        | Command::PostAgentResponse { .. }
        | Command::RequestAstridResponse { .. }) => chat::run(app, command).await,
        // ── Reminders ──
        command @ (Command::ReminderOptions { .. }
        | Command::RemindersDue
        | Command::NextReminder
        | Command::ReminderShown { .. }
        | Command::SnoozeReminder { .. }) => reminders::run(app, command),
        // ── List filters ──
        command @ (Command::FilterOptions { .. } | Command::SetFilter { .. }) => {
            list_filters::run(app, command).await
        }
        // ── The account, its agents and its integrations ──
        command @ (Command::IsSignedIn
        | Command::CurrentUser
        | Command::SearchUsers { .. }
        | Command::Agents
        | Command::AstridModel
        | Command::SetAstridModel { .. }
        | Command::WebhookSettings
        | Command::SaveWebhook { .. }
        | Command::DeleteWebhook
        | Command::TestWebhook
        | Command::CustomAgents
        | Command::RegisterCustomAgent { .. }
        | Command::DeleteCustomAgent { .. }
        | Command::SetCustomAgentPhoto { .. }
        | Command::ConnectCopilot
        | Command::DisconnectCopilot
        | Command::SetAgentMode { .. }
        | Command::SaveAgentCredential { .. }
        | Command::TestAgentCredential { .. }
        | Command::AgentCredentials
        | Command::AgentModes
        | Command::CopilotStatus
        | Command::AvailableAgents { .. }
        | Command::DeleteAgentCredential { .. }
        | Command::ExternalSync { .. }
        | Command::ExternalAccounts
        | Command::CreateAndLinkGoogleList { .. }
        | Command::ConnectProvider { .. }
        | Command::DisconnectProvider { .. }
        | Command::LinkList { .. }
        | Command::UnlinkList { .. }
        | Command::Features
        | Command::FeatureFlags
        | Command::MyTasksFilters
        | Command::RefreshMyTasks
        | Command::SetMyTasksFilters { .. }
        | Command::GoogleSyncMode
        | Command::SetGoogleSyncMode { .. }
        | Command::ImportExternalLedger { .. }
        | Command::SyncExternal
        | Command::ProfileStats
        | Command::UpdateProfile { .. }
        | Command::ResendVerification
        | Command::SendVerification
        | Command::CancelVerification
        | Command::ChangeEmail { .. }
        | Command::Passkeys
        | Command::RenamePasskey { .. }
        | Command::RevokePasskey { .. }
        | Command::PasskeyRegistrationOptions
        | Command::RegisterPasskey { .. }
        | Command::Contacts
        | Command::ClearContacts
        | Command::UploadContacts { .. }
        | Command::SearchContacts { .. }
        | Command::RecommendedCollaborators { .. }
        | Command::DeleteAccount { .. }
        | Command::ExportAccount { .. }
        | Command::Settings
        | Command::RefreshSettings
        | Command::UpdateReminderSettings { .. }
        | Command::UpdateSmartTaskSettings { .. }
        | Command::RefreshCapabilities
        | Command::FeedbackListId
        | Command::Notifications
        | Command::RefreshNotifications
        | Command::MarkNotificationsRead { .. }
        | Command::MarkAllNotificationsRead
        | Command::BeginSignIn { .. }
        | Command::CompleteSignIn { .. }
        | Command::CancelSignIn
        | Command::SignOut) => account::run(app, command).await,
        // ── Connections and credentials ──
        command @ (Command::Connections
        | Command::RevokeConnection { .. }
        | Command::LoadOAuthClient { .. }
        | Command::CheckOAuthClientDraft { .. }
        | Command::CreateOAuthClient { .. }
        | Command::UpdateOAuthClient { .. }
        | Command::MintTransportCredentials { .. }
        | Command::MintAccessToken { .. }) => connections::run(app, command).await,
        // ── Sync, the outbox and the cache ──
        command @ (Command::ImportJournalEntry { .. }
        | Command::SeedCache { .. }
        | Command::OutboxStats
        | Command::Sync
        | Command::StreamState
        | Command::ClearCache
        | Command::NetworkRestored
        | Command::ReconnectStream
        | Command::RetryDeadLetters
        | Command::Drain) => network::run(app, command).await,
        // ── The shell itself — keys, editing, theme, palette ──
        command @ (Command::ResolveShortcut { .. }
        | Command::Shortcuts
        | Command::Hotkey
        | Command::SetHotkey { .. }
        | Command::BeginEditing { .. }
        | Command::EndEditing { .. }
        | Command::CancelEditing { .. }
        | Command::CommitAllEditing
        | Command::Theme
        | Command::SetTheme { .. }
        | Command::TitleDisplay
        | Command::SetTitleDisplay { .. }
        | Command::HasSeenTour
        | Command::TourSeen
        | Command::Palette { .. }) => shell::run(app, command),
    }
}

/// A command that reached the wrong domain module.
///
/// [`run`] is one exhaustive `match` over every `Command` variant and the groups it routes are
/// disjoint, so the compiler has already proved this is unreachable. It answers rather than
/// panicking anyway: a routing slip would be a mistake in this file, and a mistake in this file
/// must not take the window down with it.
pub(super) fn misrouted(domain: &str) -> Response {
    Response::failed(Failure::bad_request(format!(
        "this command is not one of dispatch::{domain}'s"
    )))
}

/// For a write whose answer is that it happened. `Response::done()` rather than `ok(())`, so the
/// shell is not handed a `null` to decide something about.
fn answer_done(result: crate::services::Result<()>) -> Response {
    match result {
        Ok(()) => Response::done(),
        Err(error) => Response::failed(error.into()),
    }
}

fn answer<T: serde::Serialize>(result: crate::services::Result<T>) -> Response {
    match result {
        Ok(value) => Response::ok(value),
        Err(error) => Response::failed(error.into()),
    }
}

/// What a list permits, as the shells read it (task 897e25c5, astrid-web's
/// GITHUB_PROJECTS_WHITELABEL §11.2).
///
/// One projection, used by the task detail and by the list-settings flyout, because the keys are
/// the list's and a second copy of this shape is a second chance to spell `dueDate` differently.
/// Every answer comes from [`crate::model::TaskList`]'s accessors, so **an absent key reads as
/// classic** and gating on these takes nothing away from a list the server says nothing about.
///
/// `None` — the list is not in this cache, or the task is on no list at all — is classic too, and
/// deliberately so: a membership the reader cannot see must never hide a control that works.
fn supports_json(list: Option<&crate::model::TaskList>) -> serde_json::Value {
    let classic = crate::model::ListSupports::default();
    let (priority, due_date, comments, private_tasks, multiple_assignees, delete_task) = match list
    {
        Some(list) => (
            list.supports_priority(),
            list.supports_due_date(),
            list.supports_comments(),
            list.supports_private_tasks(),
            list.supports_multiple_assignees(),
            list.delete_task_mode(),
        ),
        None => (
            classic.priority,
            classic.due_date,
            classic.comments,
            classic.private_tasks,
            classic.multiple_assignees,
            classic.delete_task,
        ),
    };
    serde_json::json!({
        "priority": priority,
        "dueDate": due_date,
        "comments": comments,
        "privateTasks": private_tasks,
        "multipleAssignees": multiple_assignees,
        // The destructive action's wording AND what it sends: a GitHub board's item comes off the
        // project and the issue stays, so "Delete" there would promise something it does not do.
        "deleteTask": delete_task,
    })
}

// A module per domain: its own arms, and the helpers only its arms call.
mod account;
mod attachments;
mod board;
mod chat;
mod comments;
mod connections;
mod list_filters;
mod lists;
mod network;
mod reminders;
mod row_json;
mod shell;
mod tasks;

// Glob-imported so that a domain file can reach another domain's helper through `use super::*`,
// which is the only reason anything is re-exported here. `board`, `chat`, `list_filters` and
// `reminders` are absent deliberately: once task 8a91c5b0 moved the arms into the domain modules,
// nothing outside each of those four used anything in it, and the compiler said so. From `board`,
// only the Waiting helpers: `setTaskStatusRole` and `updateTask` take the same rule as a move.
use account::*;
use attachments::*;
use board::{resolve_waiting, write_status};
use comments::*;
use lists::*;
pub(in crate::app) use reminders::due_reminders;
use row_json::*;
use tasks::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod rows_as_ios_tests;

#[cfg(test)]
mod board_as_ios_tests;

#[cfg(test)]
mod pickers_as_ios_tests;
