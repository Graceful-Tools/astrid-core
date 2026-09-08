//! Every path this client knows, built in one place.
//!
//! Ported from `astrid-ios/Astrid App/Core/Networking/APIEndpoint.swift`.
//!
//! Ids go through [`super::path::escaped_path_component`] here rather than at the call sites,
//! which is the difference between one guard and forty. The Apple enum interpolated ids raw and
//! relied on the client's backstop; that backstop exists here too, but a path that is correct
//! before it reaches the backstop is one the backstop never has to refuse.
//!
//! ## The response envelopes
//!
//! v1 wraps: `{ "task": … }`, `{ "tasks": […], "meta": { "total": n } }`, `{ "lists": […] }`. The
//! key each route uses is named beside the path, so a caller does not have to remember which of
//! them is bare.

use super::path::escaped_path_component;

pub const TASKS: &str = "/api/v1/tasks";
pub const LISTS: &str = "/api/v1/lists";
pub const PROJECTS: &str = "/api/v1/projects";
pub const CHAT_CHANNELS: &str = "/api/v1/chat/channels";
pub const CAPABILITIES: &str = "/api/v1/capabilities";
/// Where a new file is uploaded. Multipart: the bytes, and a JSON context saying which list it
/// belongs to, which is how the server decides who may read it afterwards.
pub const REQUEST_UPLOAD: &str = "/api/v1/secure-upload/request-upload";

// ─── External sync ────────────────────────────────────────────────────────────────────────────
//
// The server holds the tokens for both providers and proxies their APIs, so a client never sees a
// Google or GitHub credential. What differs is who drives the mirroring: a cron does it for
// GitHub, and the client does it for Google.

pub const INTEGRATIONS: &str = "/api/v1/integrations";

/// My Tasks' filters and sort, which belong to the account rather than to a list.
pub const MY_TASKS_PREFERENCES: &str = "/api/v1/users/me/my-tasks-preferences";

// ─── The agents ───────────────────────────────────────────────────────────────────────────────

pub const AGENT_MODES: &str = "/api/v1/users/me/agent-modes";
/// Which services have a key. The server answers with that, never with the key.
pub const AI_CREDENTIALS: &str = "/api/v1/users/me/ai-credentials";
pub const AI_CREDENTIALS_TEST: &str = "/api/v1/users/me/ai-credentials/test";
/// Where an account's own agent is told about work, and the secret that signs it.
pub const WEBHOOK_SETTINGS: &str = "/api/v1/users/me/webhook-settings";
/// The agents an account has registered of its own.
pub const CUSTOM_AGENTS: &str = "/api/v1/custom-agents/agents";
pub const CUSTOM_AGENT_REGISTER: &str = "/api/v1/custom-agents/register";

/// One registered agent.
pub fn custom_agent(id: &str) -> String {
    format!("{CUSTOM_AGENTS}/{id}")
}

// ─── API access ───────────────────────────────────────────────────────────────────────────────

/// This device swapping the session it holds for a token it can put in a header.
///
/// Cookie-authenticated on purpose: only a client that is already signed in may mint one. It mints
/// or RETURNS a 90-day token, so asking twice does not litter the account with credentials nobody
/// is holding.
pub const MOBILE_MCP_TOKEN: &str = "/api/v1/auth/mobile-mcp-token";
/// Client-credentials pairs, for a machine that is not this one.
pub const OAUTH_CLIENTS: &str = "/api/v1/oauth/clients";

/// One registered pair.
pub fn oauth_client(client_id: &str) -> String {
    format!("{OAUTH_CLIENTS}/{client_id}")
}

pub const COPILOT_STATUS: &str = "/api/v1/integrations/copilot/status";
pub const COPILOT_AUTHORIZE: &str = "/api/v1/integrations/copilot/authorize";
pub const GOOGLE_TASKLISTS: &str = "/api/v1/sync/google/tasklists";
pub const GOOGLE_TASKS: &str = "/api/v1/sync/google/tasks";
pub const GOOGLE_TASK_LINKS: &str = "/api/v1/sync/google/task-links";
pub const GITHUB_REPOSITORIES: &str = "/api/v1/github/repositories";

/// Where a provider's browser hand-off starts.
pub fn integration_authorize(provider: &str) -> String {
    format!(
        "/api/v1/integrations/{}/authorize",
        escaped_path_component(provider)
    )
}

/// One provider's list ↔ container links.
pub fn sync_links(provider: &str) -> String {
    format!("/api/v1/sync/{}/links", escaped_path_component(provider))
}
pub const ME: &str = "/api/v1/users/me";
pub const USER_SETTINGS: &str = "/api/v1/users/me/settings";
pub const USER_SEARCH: &str = "/api/v1/users/search";
/// Everything this account has, as one file. `?format=json` or `?format=csv`.
pub const EXPORT: &str = "/api/v1/users/me/export";

/// Somebody's profile: who they are, and the three numbers under it.
pub fn user_profile(user_id: &str) -> String {
    format!("/api/v1/users/{}/profile", escaped_path_component(user_id))
}
pub const PUBLIC_LISTS: &str = "/api/v1/public/lists";

/// The envelope key each collection route answers under.
pub mod envelope {
    pub const TASK: &str = "task";
    pub const TASKS: &str = "tasks";
    pub const LIST: &str = "list";
    pub const LISTS: &str = "lists";
    pub const PROJECT: &str = "project";
    pub const PROJECTS: &str = "projects";
    pub const COMMENT: &str = "comment";
    pub const COMMENTS: &str = "comments";
    pub const MESSAGE: &str = "message";
    pub const MESSAGES: &str = "messages";
    pub const CHANNELS: &str = "channels";
    pub const MEMBERS: &str = "members";
    pub const USERS: &str = "users";
}

pub fn task(id: &str) -> String {
    format!("{TASKS}/{}", escaped_path_component(id))
}

pub fn task_comments(task_id: &str) -> String {
    format!("{}/comments", task(task_id))
}

pub fn comment(id: &str) -> String {
    format!("/api/v1/comments/{}", escaped_path_component(id))
}

pub fn list(id: &str) -> String {
    format!("{LISTS}/{}", escaped_path_component(id))
}

/// One file's bytes. The same path with `?info=true` answers with its metadata instead.
pub fn secure_file(file_id: &str) -> String {
    format!("/api/v1/secure-files/{}", escaped_path_component(file_id))
}

pub fn list_members(list_id: &str) -> String {
    format!("{}/members", list(list_id))
}

pub fn list_member(list_id: &str, user_id: &str) -> String {
    format!(
        "{}/members/{}",
        list(list_id),
        escaped_path_component(user_id)
    )
}

pub fn list_invitations(list_id: &str) -> String {
    format!("{}/invitations", list(list_id))
}

pub fn leave_list(list_id: &str) -> String {
    format!("{}/leave", list(list_id))
}

pub fn copy_list(list_id: &str) -> String {
    format!("{}/copy", list(list_id))
}

pub fn project(id: &str) -> String {
    format!("{PROJECTS}/{}", escaped_path_component(id))
}

pub fn channel_messages(channel_id: &str) -> String {
    format!(
        "{CHAT_CHANNELS}/{}/messages",
        escaped_path_component(channel_id)
    )
}

pub fn shortcode(code: &str) -> String {
    format!("/api/v1/shortcodes/{}", escaped_path_component(code))
}

#[cfg(test)]
mod tests {
    use super::super::path::is_safe_request_path;
    use super::*;

    #[test]
    fn paths_are_built_under_the_versioned_prefix() {
        assert_eq!(task("t1"), "/api/v1/tasks/t1");
        assert_eq!(task_comments("t1"), "/api/v1/tasks/t1/comments");
        assert_eq!(list_member("l1", "u1"), "/api/v1/lists/l1/members/u1");
        assert_eq!(channel_messages("c1"), "/api/v1/chat/channels/c1/messages");
    }

    /// The point of building paths here: an id that arrived from a deep link cannot add a segment,
    /// whichever call site handed it over.
    #[test]
    fn an_id_can_never_add_a_path_segment() {
        assert_eq!(task("../admin"), "/api/v1/tasks/..%2Fadmin");
        assert!(!is_safe_request_path(&task("../admin")));
        assert!(!is_safe_request_path(&list_member("l1", "../admin")));
    }

    /// An id that arrives already percent-encoded is encoded again — `%` is not unreserved — so
    /// what the server decodes once is a literal segment named `..%2F..`, not a traversal. The
    /// backstop lets it through for that reason, and it is right to.
    #[test]
    fn an_already_encoded_traversal_is_neutralised_rather_than_passed_on() {
        let path = task("..%2F..%2Fadmin");
        assert_eq!(path, "/api/v1/tasks/..%252F..%252Fadmin");
        assert!(is_safe_request_path(&path));
    }

    #[test]
    fn an_ordinary_id_is_left_readable() {
        assert!(is_safe_request_path(&task("cm3x8k2p40001")));
        assert!(is_safe_request_path(&comment("cm3x8k2p40002")));
    }
}
