//! Everything that can act as this account, and a way to stop each one.
//!
//! Ports the rules behind astrid-web's Connections page (`lib/connections/`, AWTD-981) and the
//! Apple clients' `ConnectionsModel.swift` / `OAuthClientDraft.swift` (AITD-419, AITD-420), whose
//! tests are the specification this module was written against.
//!
//! ## Five sources, three types
//!
//! `GET /api/v1/users/me/connections` lists five credential *kinds* — an OAuth client the account
//! made, an app approved on the consent page, a Custom Agent's client, a user-level access token
//! and the webhook server — and they are not five types. The first three are one `OAuthClient`
//! table read by three queries that differ only in who owns the row. So every row carries two
//! facets beside its kind: a **category** (app, token, webhook), which is what a screen groups by,
//! and an **owner** (yours, third-party, agent), which is the badge an app row wears. The kind is
//! untouched by that: it is the path segment of the revoke route, so the facets group and the kind
//! revokes. A client that adopted one and dropped the other would group beautifully and revoke
//! nothing.
//!
//! ## Decoded leniently, on purpose
//!
//! A kind this build has never heard of is a row it cannot revoke — the path segment would be one
//! it cannot name — not a decode failure that blanks the whole list. The same for a category, a
//! status, or a field the server omits: the server may add a sixth source before the Store ships
//! the build that knows it.
//!
//! ## The review
//!
//! The page answers the question a reader actually arrives with — "which of these can I turn
//! off?" — by looking at the dates it already has (web `review-connections.ts`). Conservative in
//! one direction: suggesting a live credential be revoked breaks whatever depends on it, staying
//! quiet about an idle one costs nothing. So a kind that records no usage (an access token) and a
//! row that cannot be revoked are never suggested.
//!
//! ## A secret is shown once
//!
//! Minting answers with plaintext exactly once and never again — the server stores a hash. Nothing
//! here writes a secret into the cache; the shell shows it until the panel closes.
//!
//! ## What left
//!
//! The mobile MCP token this module's predecessor minted (`/api/v1/auth/mobile-mcp-token`) is gone
//! from the server (astrid-web #285): the tokens it made are listed here as access tokens, and
//! revoked here, one at a time.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Context, Result};
use crate::api::endpoints;
use crate::model::date;

// ── The rows ─────────────────────────────────────────────────────────────────────────────────

/// The five credential sources the server lists, plus the one this build does not know yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionKind {
    /// An OAuth client the account made — the developer console, or a transport preset.
    OauthClient,
    /// A dynamically registered client approved on the consent page (Claude Code, VS Code…).
    AuthorizedApp,
    /// A Custom Agent the account registered; its client belongs to the agent's bot user.
    CustomAgent,
    /// A user-level access token, such as the GitHub.com Copilot cloud agent's.
    AccessToken,
    /// The account's webhook server.
    Webhook,
    /// A kind added server-side after this build shipped.
    #[serde(other)]
    Unknown,
}

impl ConnectionKind {
    /// Display order under kind headings: what a reader made or approved first, the plumbing last.
    pub const DISPLAY_ORDER: [ConnectionKind; 6] = [
        ConnectionKind::AuthorizedApp,
        ConnectionKind::OauthClient,
        ConnectionKind::CustomAgent,
        ConnectionKind::AccessToken,
        ConnectionKind::Webhook,
        ConnectionKind::Unknown,
    ];

    /// The path segment of the revoke route, and the wire spelling.
    pub fn wire(self) -> &'static str {
        match self {
            ConnectionKind::OauthClient => "oauthClient",
            ConnectionKind::AuthorizedApp => "authorizedApp",
            ConnectionKind::CustomAgent => "customAgent",
            ConnectionKind::AccessToken => "accessToken",
            ConnectionKind::Webhook => "webhook",
            ConnectionKind::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Self {
        serde_json::from_value(Value::String(raw.to_string())).unwrap_or(ConnectionKind::Unknown)
    }

    /// Whether a `lastUsedAt` on this kind is a real observation rather than a placeholder. An
    /// access token has no such column, so every one reports "never" — the one that ran a minute
    /// ago as loudly as the one nobody has touched since it was made.
    fn tracks_usage(self) -> bool {
        matches!(
            self,
            ConnectionKind::OauthClient
                | ConnectionKind::AuthorizedApp
                | ConnectionKind::CustomAgent
                | ConnectionKind::Webhook
        )
    }
}

/// What a row IS, once the three `OAuthClient`-backed kinds are seen for what they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionCategory {
    /// A client id + secret, whoever owns it.
    App,
    /// A bearer string, pasted.
    Token,
    /// The server Astrid calls OUT to, rather than one calling in.
    Webhook,
    /// A category added server-side after this build shipped.
    #[serde(other)]
    Unknown,
}

impl ConnectionCategory {
    /// What acts as you, then how, then what points out. The unknown trails, as with kinds.
    pub const DISPLAY_ORDER: [ConnectionCategory; 4] = [
        ConnectionCategory::App,
        ConnectionCategory::Token,
        ConnectionCategory::Webhook,
        ConnectionCategory::Unknown,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            ConnectionCategory::App => "app",
            ConnectionCategory::Token => "token",
            ConnectionCategory::Webhook => "webhook",
            ConnectionCategory::Unknown => "unknown",
        }
    }
}

/// For an app, whose it is. Absent for a token or a webhook, which have no owner to draw.
///
/// No `Unknown` case, unlike the kind and the category: an owner this build cannot name is a
/// badge it cannot write, and no badge is the honest answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionOwner {
    /// Made in the developer console.
    You,
    /// Approved on the consent page.
    ThirdParty,
    /// Belongs to a Custom Agent the account registered.
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    Active,
    Expired,
    Disabled,
    #[serde(other)]
    Unknown,
}

/// What the server knows about a row beyond the common fields. Every field optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectionDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Why a row looks unused enough to be worth revoking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewReason {
    /// Used once and then forgotten.
    Idle,
    /// Created and never used.
    NeverUsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionReview {
    pub reason: ReviewReason,
    /// Whole days since the last use, or since creation when there has been none.
    pub days: i64,
}

/// Used once and then forgotten: no use in this long reads as abandoned.
pub const IDLE_AFTER_DAYS: i64 = 90;
/// Created and never used. Longer than a holiday, shorter than a quarter.
pub const NEVER_USED_AFTER_DAYS: i64 = 30;

/// One thing that can act as the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    /// `OAuthClient.id`, `MCPToken.id`, the Custom Agent's `User.id`, or the literal `webhook`.
    pub id: String,
    pub kind: ConnectionKind,
    /// What the screen groups by. `None` means the server predates AWTD-981 — not that the row is
    /// uncategorised.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<ConnectionCategory>,
    /// Whose app it is; `None` for a token, a webhook, or an owner this build cannot name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<ConnectionOwner>,
    pub name: String,
    /// The email this credential authors as. `None` means the account holder themself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acts_as: Option<String>,
    pub scopes: Vec<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub status: ConnectionStatus,
    /// Whether this build can revoke it. An unknown kind never can, whatever the server said.
    pub revocable: bool,
    /// Which settings page owns further management: `agents` or `connections`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manage_in: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<ConnectionDetail>,
    /// The client this screen may edit, when it is one. See [`Connection::editable_client_id`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editable_client_id: Option<String>,
    /// Why it looks unused, when it does. Filled by [`with_reviews`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<ConnectionReview>,
}

impl Connection {
    /// Read one row as the server sent it. `None` only when there is no id at all — a row that
    /// cannot be addressed is not a row.
    pub fn from_value(value: &Value) -> Option<Self> {
        let id = string(value, "id")?;
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .map(ConnectionKind::parse)
            .unwrap_or(ConnectionKind::Unknown);
        let category = value.get("category").and_then(Value::as_str).map(|raw| {
            serde_json::from_value(Value::String(raw.to_string()))
                .unwrap_or(ConnectionCategory::Unknown)
        });
        let owner = value
            .get("owner")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_value(Value::String(raw.to_string())).ok());
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .map(|raw| {
                serde_json::from_value(Value::String(raw.to_string()))
                    .unwrap_or(ConnectionStatus::Unknown)
            })
            .unwrap_or(ConnectionStatus::Unknown);
        // An unknown kind cannot be revoked from here whatever the server says: the path segment
        // would be one this build cannot name.
        let revocable = value
            .get("revocable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && kind != ConnectionKind::Unknown;
        let detail: Option<ConnectionDetail> = value
            .get("detail")
            .filter(|detail| detail.is_object())
            .and_then(|detail| serde_json::from_value(detail.clone()).ok());
        let manage_in = string(value, "manageIn");
        let mut connection = Connection {
            id,
            kind,
            category,
            owner,
            name: string(value, "name").unwrap_or_default(),
            acts_as: string(value, "actsAs"),
            scopes: strings(value, "scopes"),
            created_at: string(value, "createdAt").unwrap_or_default(),
            last_used_at: string(value, "lastUsedAt"),
            expires_at: string(value, "expiresAt"),
            status,
            revocable,
            manage_in,
            detail,
            editable_client_id: None,
            review: None,
        };
        connection.editable_client_id = connection.editable_client_id();
        Some(connection)
    }

    pub fn managed_on_agents_page(&self) -> bool {
        self.manage_in.as_deref() == Some("agents")
    }

    /// Which client this screen offers an Edit for (AITD-419).
    ///
    /// Only an OAuth client — an approved app has no redirect URIs of ours to change — and only
    /// one this page owns: the Agent Hub manages its transport clients, and two owners of one row
    /// is how two screens come to disagree. A row whose `detail.clientId` the server did not send
    /// is one this build cannot address.
    pub fn editable_client_id(&self) -> Option<String> {
        if self.kind != ConnectionKind::OauthClient || self.managed_on_agents_page() {
            return None;
        }
        self.detail
            .as_ref()
            .and_then(|detail| detail.client_id.clone())
            .filter(|client_id| !client_id.is_empty())
    }

    /// The route that revokes this row: the kind and the id, together, since ids repeat across
    /// kinds.
    pub fn revoke_path(&self) -> String {
        endpoints::connection(self.kind.wire(), &self.id)
    }
}

/// The answer to `GET /api/v1/users/me/connections`, as rows. Whatever the list looks like.
pub fn rows(answer: &Value) -> Vec<Connection> {
    let list = answer
        .get("connections")
        .and_then(Value::as_array)
        .or_else(|| answer.as_array());
    list.map(|list| list.iter().filter_map(Connection::from_value).collect())
        .unwrap_or_default()
}

// ── The review ───────────────────────────────────────────────────────────────────────────────

fn whole_days_since(value: Option<&str>, now: DateTime<Utc>) -> Option<i64> {
    let then = date::parse(value?)?;
    Some((now - then).num_days())
}

/// Why one row looks unused, if it does (web `reviewConnections`).
pub fn review(connection: &Connection, now: DateTime<Utc>) -> Option<ConnectionReview> {
    if !connection.revocable || !connection.kind.tracks_usage() {
        return None;
    }
    if let Some(idle) = whole_days_since(connection.last_used_at.as_deref(), now) {
        return (idle >= IDLE_AFTER_DAYS).then_some(ConnectionReview {
            reason: ReviewReason::Idle,
            days: idle,
        });
    }
    let age = whole_days_since(Some(connection.created_at.as_str()), now)?;
    (age >= NEVER_USED_AFTER_DAYS).then_some(ConnectionReview {
        reason: ReviewReason::NeverUsed,
        days: age,
    })
}

/// Stamp every row with its review, so the count at the top and the line under a row come from
/// one judgement.
pub fn with_reviews(mut connections: Vec<Connection>, now: DateTime<Utc>) -> Vec<Connection> {
    for connection in &mut connections {
        connection.review = review(connection, now);
    }
    connections
}

// ── The sections ─────────────────────────────────────────────────────────────────────────────

/// What a section is headed by. Two flavours, because the server has two vintages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "by")]
pub enum SectionHeading {
    Category { category: ConnectionCategory },
    Kind { kind: ConnectionKind },
}

/// One heading and the rows under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSection {
    pub heading: SectionHeading,
    /// The resource the shell titles the section with: `connections.category.app`,
    /// `connections.kind.authorizedApp`.
    pub title_key: String,
    /// Owner badges belong to the category grouping only. Under a kind heading the row's owner is
    /// already what the heading says, and the badge would say it twice.
    pub shows_owner_badges: bool,
    pub rows: Vec<Connection>,
}

/// The rows as sections, in display order, empty ones omitted — a heading over nothing is a
/// category the reader has to rule out for no reason.
///
/// By category when the server stamped every row with one, which collapses the three
/// `OAuthClient`-backed kinds into one Apps section (AITD-420). By kind when any row lacks one:
/// `category` is emitted by every build carrying AWTD-981, so a gap means this app is talking to a
/// deployment that predates it — all-or-nothing per response, not row by row, or one old row
/// would strand the rest under a heading of its own.
pub fn sections(connections: &[Connection]) -> Vec<ConnectionSection> {
    if connections
        .iter()
        .all(|connection| connection.category.is_some())
    {
        ConnectionCategory::DISPLAY_ORDER
            .iter()
            .filter_map(|category| {
                let rows: Vec<Connection> = connections
                    .iter()
                    .filter(|connection| connection.category == Some(*category))
                    .cloned()
                    .collect();
                (!rows.is_empty()).then(|| ConnectionSection {
                    heading: SectionHeading::Category {
                        category: *category,
                    },
                    title_key: format!("connections.category.{}", category.wire()),
                    shows_owner_badges: true,
                    rows,
                })
            })
            .collect()
    } else {
        ConnectionKind::DISPLAY_ORDER
            .iter()
            .filter_map(|kind| {
                let rows: Vec<Connection> = connections
                    .iter()
                    .filter(|connection| connection.kind == *kind)
                    .cloned()
                    .collect();
                (!rows.is_empty()).then(|| ConnectionSection {
                    heading: SectionHeading::Kind { kind: *kind },
                    title_key: format!("connections.kind.{}", kind.wire()),
                    shows_owner_badges: false,
                    rows,
                })
            })
            .collect()
    }
}

/// The Connections page in one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsPanel {
    pub connections: Vec<Connection>,
    pub sections: Vec<ConnectionSection>,
    /// How many rows look unused — the line at the top, before any row is read.
    pub review_count: usize,
}

pub fn panel(answer: &Value, now: DateTime<Utc>) -> ConnectionsPanel {
    let connections = with_reviews(rows(answer), now);
    let sections = sections(&connections);
    let review_count = connections
        .iter()
        .filter(|connection| connection.review.is_some())
        .count();
    ConnectionsPanel {
        connections,
        sections,
        review_count,
    }
}

// ── OAuth clients: the developer console's half ──────────────────────────────────────────────

/// The grant types a client can be registered with — astrid-web `types/oauth.ts` `GrantType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GrantType {
    #[serde(rename = "client_credentials")]
    ClientCredentials,
    #[serde(rename = "authorization_code")]
    AuthorizationCode,
    #[serde(rename = "refresh_token")]
    RefreshToken,
}

impl GrantType {
    /// Display and wire order. The wire body depends on it being stable, so it is named rather
    /// than inherited by luck.
    pub const DISPLAY_ORDER: [GrantType; 3] = [
        GrantType::ClientCredentials,
        GrantType::AuthorizationCode,
        GrantType::RefreshToken,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            GrantType::ClientCredentials => "client_credentials",
            GrantType::AuthorizationCode => "authorization_code",
            GrantType::RefreshToken => "refresh_token",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_value(Value::String(raw.to_string())).ok()
    }
}

/// The scopes the picker offers, mirroring astrid-web `lib/oauth/oauth-scopes.ts` `OAUTH_SCOPES`
/// minus the wildcard.
///
/// A mirrored list goes stale, and the failure mode is worth stating: a scope added server-side
/// after this build shipped is one this picker cannot tick — not one it breaks on. The server, not
/// this list, is what validates a create. `*` is absent on purpose: it grants the whole account and
/// the server refuses to register it, so offering it would be a toggle that always fails.
pub const REGISTERABLE_SCOPES: [&str; 25] = [
    "tasks:read",
    "tasks:write",
    "tasks:delete",
    "lists:read",
    "lists:write",
    "lists:delete",
    "lists:manage_members",
    "projects:read",
    "projects:write",
    "projects:delete",
    "comments:read",
    "comments:write",
    "comments:delete",
    "chat:read",
    "chat:write",
    "user:read",
    "user:write",
    "attachments:read",
    "attachments:write",
    "attachments:delete",
    "contacts:read",
    "contacts:write",
    "public:read",
    "public:write",
    "sse:connect",
];

/// One OAuth client as the server describes it — the fields an editor needs, which is more than
/// a connection row carries (a row knows the client id but not its redirect URIs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClientSummary {
    pub client_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub scopes: Vec<String>,
    pub is_active: bool,
}

impl OAuthClientSummary {
    /// Decoded leniently for the same reason as a connection: this build must survive a server
    /// that has learned a new field, and an absent field is a default, not a decode failure.
    /// Absent `isActive` reads as active: the screen only ever asks for a client it just saw.
    pub fn from_value(value: &Value) -> Option<Self> {
        let client = value.get("client").unwrap_or(value);
        Some(OAuthClientSummary {
            client_id: string(client, "clientId")?,
            name: string(client, "name").unwrap_or_default(),
            description: string(client, "description"),
            redirect_uris: strings(client, "redirectUris"),
            grant_types: strings(client, "grantTypes"),
            scopes: strings(client, "scopes"),
            is_active: client
                .get("isActive")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        })
    }
}

/// A credential the server has just made, in the one form it will ever be readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MintedClient {
    pub client_id: String,
    /// Plaintext, once. Never cached — see the module header.
    pub client_secret: String,
    pub name: String,
}

impl MintedClient {
    fn from_value(answer: &Value, fallback_name: &str) -> Self {
        let client = answer.get("client").unwrap_or(answer);
        MintedClient {
            client_id: string(client, "clientId").unwrap_or_default(),
            client_secret: string(client, "clientSecret").unwrap_or_default(),
            name: string(client, "name").unwrap_or_else(|| fallback_name.to_string()),
        }
    }
}

/// The client shapes the agents page mints for its own transports — see astrid-web
/// `lib/oauth/oauth-client-presets.ts`. The server decides scopes and grant types from the preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OAuthClientPreset {
    GithubActions,
    WebhookServer,
}

/// Why a draft cannot be sent yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "key")]
pub enum DraftProblem {
    NameMissing,
    GrantRequired,
    RedirectRequired,
    RedirectInvalid { uri: String },
}

/// What the editor holds while a person is typing, and the one place that says whether it can be
/// sent yet. Every rule here has a twin in astrid-web `components/oauth-app-manager.tsx`
/// (`handleCreate`, `toggleGrantType`) and in the Apple clients' `OAuthClientDraft.swift`.
///
/// What is deliberately NOT here: deciding which scopes are acceptable. The server owns that, and
/// a build that shipped before a scope existed must not be the thing that decides it is invalid.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OAuthClientDraft {
    pub name: String,
    pub description: String,
    pub scopes: BTreeSet<String>,
    pub grant_types: BTreeSet<GrantType>,
    /// Grant types the server has and this build does not. Kept so an edit sends them back
    /// rather than silently narrowing a client that was minted with a newer grant.
    pub unrecognized_grant_types: Vec<String>,
    /// One URI per line — the same shape as the web dialog's textarea.
    pub redirect_uri_text: String,
}

impl OAuthClientDraft {
    /// An empty draft: one grant, the one a script needs.
    pub fn new() -> Self {
        OAuthClientDraft {
            grant_types: BTreeSet::from([GrantType::ClientCredentials]),
            ..Default::default()
        }
    }

    /// A draft filled from a client being edited.
    pub fn from_client(client: &OAuthClientSummary) -> Self {
        OAuthClientDraft {
            name: client.name.clone(),
            description: client.description.clone().unwrap_or_default(),
            scopes: client.scopes.iter().cloned().collect(),
            grant_types: client
                .grant_types
                .iter()
                .filter_map(|raw| GrantType::parse(raw))
                .collect(),
            unrecognized_grant_types: client
                .grant_types
                .iter()
                .filter(|raw| GrantType::parse(raw).is_none())
                .cloned()
                .collect(),
            redirect_uri_text: client.redirect_uris.join("\n"),
        }
    }

    /// The lines that are actually URIs. Blank lines and stray indentation are what typing looks
    /// like, not mistakes to report.
    pub fn redirect_uris(&self) -> Vec<String> {
        self.redirect_uri_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }

    pub fn trimmed_name(&self) -> &str {
        self.name.trim()
    }

    /// The FIRST thing to fix, in the order the fields appear — a list of every problem at once
    /// is a wall to read, and fixing the top one usually reveals whether the rest were real.
    pub fn problem(&self) -> Option<DraftProblem> {
        if self.trimmed_name().is_empty() {
            return Some(DraftProblem::NameMissing);
        }
        if self.grant_types.is_empty() {
            return Some(DraftProblem::GrantRequired);
        }
        let uris = self.redirect_uris();
        if self.grant_types.contains(&GrantType::AuthorizationCode) && uris.is_empty() {
            return Some(DraftProblem::RedirectRequired);
        }
        uris.into_iter()
            .find(|uri| !is_allowed_redirect_uri(uri))
            .map(|uri| DraftProblem::RedirectInvalid { uri })
    }

    pub fn is_valid(&self) -> bool {
        self.problem().is_none()
    }

    /// Toggle one grant, with the pairing the web's `toggleGrantType` applies.
    ///
    /// `refresh_token` is not a way to get a token — it is how the authorization-code flow keeps
    /// one alive, so the two travel together in both directions. And the last grant cannot be
    /// turned off: a client with none can never authenticate at all.
    pub fn toggling(grant: GrantType, current: &BTreeSet<GrantType>) -> BTreeSet<GrantType> {
        let mut next = current.clone();
        if current.contains(&grant) {
            next.remove(&grant);
            if grant == GrantType::AuthorizationCode {
                next.remove(&GrantType::RefreshToken);
            }
            return if next.is_empty() {
                current.clone()
            } else {
                next
            };
        }
        next.insert(grant);
        if grant == GrantType::AuthorizationCode {
            next.insert(GrantType::RefreshToken);
        }
        if grant == GrantType::RefreshToken {
            next.insert(GrantType::AuthorizationCode);
        }
        next
    }

    /// Stable wire order: the grants this build knows in display order, then anything the server
    /// had that it did not.
    pub fn wire_grant_types(&self) -> Vec<String> {
        GrantType::DISPLAY_ORDER
            .iter()
            .filter(|grant| self.grant_types.contains(grant))
            .map(|grant| grant.wire().to_string())
            .chain(self.unrecognized_grant_types.iter().cloned())
            .collect()
    }

    /// The body `POST /api/v1/oauth/clients` reads: the developer console's shape, where the
    /// caller chose everything. An empty description and no URIs are absent, not `""` and `[]`.
    pub fn create_body(&self) -> Value {
        let mut body = json!({
            "name": self.trimmed_name(),
            "scopes": self.scopes.iter().collect::<Vec<_>>(),
            "grantTypes": self.wire_grant_types(),
        });
        let description = self.description.trim();
        if !description.is_empty() {
            body["description"] = json!(description);
        }
        let uris = self.redirect_uris();
        if !uris.is_empty() {
            body["redirectUris"] = json!(uris);
        }
        body
    }

    /// An edit writes the redirect URIs, which is what the web dialog changes too — name and
    /// description are shown but not editable, so the two consoles agree on what an edit means.
    pub fn update_body(&self) -> Value {
        json!({ "redirectUris": self.redirect_uris() })
    }
}

/// http and https only, and absolute — the same test the web dialog makes with `new URL()`. A
/// custom scheme is refused here rather than at the server, so the reason arrives next to the
/// field that caused it.
pub fn is_allowed_redirect_uri(value: &str) -> bool {
    match url::Url::parse(value) {
        Ok(parsed) => {
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str().is_some_and(|host| !host.is_empty())
        }
        Err(_) => false,
    }
}

// ── The service ──────────────────────────────────────────────────────────────────────────────

pub struct ConnectionsService {
    context: Context,
}

impl ConnectionsService {
    pub fn new(context: Context) -> Self {
        ConnectionsService { context }
    }

    /// Everything that can act as the account, grouped and reviewed.
    pub async fn panel(&self) -> Result<ConnectionsPanel> {
        let request = self.context.client.get(endpoints::CONNECTIONS);
        let answer: Value = self.context.client.send(request).await?;
        Ok(panel(&answer, self.context.clock.now()))
    }

    /// Stop one credential. The caller checks the kind is one this build can name.
    pub async fn revoke(&self, kind: ConnectionKind, id: &str) -> Result<()> {
        let request = self
            .context
            .client
            .delete(endpoints::connection(kind.wire(), id));
        self.context.client.send(request).await?;
        Ok(())
    }

    /// One client, for editing.
    pub async fn oauth_client(&self, client_id: &str) -> Result<OAuthClientSummary> {
        let request = self.context.client.get(endpoints::oauth_client(client_id));
        let answer: Value = self.context.client.send(request).await?;
        OAuthClientSummary::from_value(&answer).ok_or(super::ServiceError::NotFound {
            kind: "client",
            id: client_id.to_string(),
        })
    }

    /// Register a client the caller shaped. Answers with the secret shown only this once. The
    /// caller validates the draft first; this sends what it is given.
    pub async fn create_oauth_client(&self, draft: &OAuthClientDraft) -> Result<MintedClient> {
        let request = self
            .context
            .client
            .post(endpoints::OAUTH_CLIENTS)
            .value(draft.create_body());
        let answer: Value = self.context.client.send(request).await?;
        Ok(MintedClient::from_value(&answer, draft.trimmed_name()))
    }

    /// Change a client's redirect URIs.
    pub async fn update_oauth_client(
        &self,
        client_id: &str,
        draft: &OAuthClientDraft,
    ) -> Result<OAuthClientSummary> {
        let request = self
            .context
            .client
            .put(endpoints::oauth_client(client_id))
            .value(draft.update_body());
        let answer: Value = self.context.client.send(request).await?;
        OAuthClientSummary::from_value(&answer).ok_or(super::ServiceError::NotFound {
            kind: "client",
            id: client_id.to_string(),
        })
    }

    /// Client credentials for a transport the agents page configures. The body names only the
    /// preset and the agent: scopes and grant types are the server's decision, never the client's.
    pub async fn mint_transport_credentials(
        &self,
        preset: OAuthClientPreset,
        agent: &str,
    ) -> Result<MintedClient> {
        let request = self
            .context
            .client
            .post(endpoints::OAUTH_CLIENTS)
            .value(json!({ "preset": preset, "agent": agent }));
        let answer: Value = self.context.client.send(request).await?;
        Ok(MintedClient::from_value(&answer, ""))
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that predates AWTD-981: `kind` only, no `category`/`owner`. The Apple clients'
    /// fixture, verbatim, so the three ports are checked against one set of rows.
    const LEGACY: &str = r#"{"connections":[
      {"id":"c1","kind":"oauthClient","name":"My script","actsAs":null,"scopes":["tasks:read"],
       "createdAt":"2026-09-01T10:00:00.000Z","lastUsedAt":null,"expiresAt":null,
       "status":"active","revocable":true,"manageIn":"connections","detail":{"clientId":"astrid_client_1"}},
      {"id":"dcr-1","kind":"authorizedApp","name":"Claude Code","actsAs":"claude@example.test",
       "scopes":["tasks:read","tasks:write"],"createdAt":"2026-09-01T10:00:00Z","lastUsedAt":"2026-09-19T10:00:00Z",
       "expiresAt":"2026-10-19T10:00:00Z","status":"active","revocable":true,"manageIn":"connections",
       "detail":{"activeTokens":2}},
      {"id":"agent-1","kind":"customAgent","name":"nightly","actsAs":"nightly.oc@example.test","scopes":[],
       "createdAt":"2026-09-01T10:00:00Z","lastUsedAt":null,"expiresAt":null,"status":"active","revocable":true,"manageIn":"agents"},
      {"id":"tok-1","kind":"accessToken","name":"GitHub Copilot cloud agent","actsAs":"copilot@example.test","scopes":["*"],
       "createdAt":"2026-09-01T10:00:00Z","lastUsedAt":null,"expiresAt":"2027-09-01T10:00:00Z","status":"active","revocable":true,"manageIn":"agents"},
      {"id":"webhook","kind":"webhook","name":"hooks.example.test","actsAs":null,"scopes":[],
       "createdAt":"2026-09-01T10:00:00Z","lastUsedAt":null,"expiresAt":null,"status":"active","revocable":true,"manageIn":"agents"},
      {"id":"future-1","kind":"quantumLink","name":"Something new","scopes":[],
       "createdAt":"2026-09-01T10:00:00Z","status":"active","revocable":true}
    ],"meta":{"apiVersion":"v1","authSource":"session","total":6}}"#;

    /// A server carrying AWTD-981: every row stamped with its `category`, and an app row with its
    /// `owner`. Same six rows, so the two fixtures differ in exactly the thing under test.
    const FACETS: &str = r#"{"connections":[
      {"id":"c1","kind":"oauthClient","category":"app","owner":"you","name":"My script","actsAs":null,"scopes":["tasks:read"],
       "createdAt":"2026-09-01T10:00:00.000Z","lastUsedAt":null,"expiresAt":null,
       "status":"active","revocable":true,"manageIn":"connections","detail":{"clientId":"astrid_client_1"}},
      {"id":"dcr-1","kind":"authorizedApp","category":"app","owner":"thirdParty","name":"Claude Code","actsAs":"claude@example.test",
       "scopes":["tasks:read","tasks:write"],"createdAt":"2026-09-01T10:00:00Z","lastUsedAt":"2026-09-19T10:00:00Z",
       "expiresAt":"2026-10-19T10:00:00Z","status":"active","revocable":true,"manageIn":"connections",
       "detail":{"activeTokens":2}},
      {"id":"agent-1","kind":"customAgent","category":"app","owner":"agent","name":"nightly","actsAs":"nightly.oc@example.test","scopes":[],
       "createdAt":"2026-09-01T10:00:00Z","lastUsedAt":null,"expiresAt":null,"status":"active","revocable":true,"manageIn":"agents"},
      {"id":"tok-1","kind":"accessToken","category":"token","owner":null,"name":"GitHub Copilot cloud agent","actsAs":"copilot@example.test","scopes":["*"],
       "createdAt":"2026-09-01T10:00:00Z","lastUsedAt":null,"expiresAt":"2027-09-01T10:00:00Z","status":"active","revocable":true,"manageIn":"agents"},
      {"id":"webhook","kind":"webhook","category":"webhook","owner":null,"name":"hooks.example.test","actsAs":null,"scopes":[],
       "createdAt":"2026-09-01T10:00:00Z","lastUsedAt":null,"expiresAt":null,"status":"active","revocable":true,"manageIn":"agents"},
      {"id":"future-1","kind":"quantumLink","category":"teleport","owner":"nobody","name":"Something new","scopes":[],
       "createdAt":"2026-09-01T10:00:00Z","status":"active","revocable":true}
    ],"meta":{"apiVersion":"v1","authSource":"session","total":6}}"#;

    fn legacy() -> Vec<Connection> {
        rows(&serde_json::from_str(LEGACY).expect("json"))
    }

    fn facets() -> Vec<Connection> {
        rows(&serde_json::from_str(FACETS).expect("json"))
    }

    fn now() -> DateTime<Utc> {
        date::parse("2026-09-25T12:00:00Z").expect("an instant")
    }

    // ── Decoding ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn decodes_every_kind_the_server_lists_and_survives_one_it_does_not() {
        let rows = legacy();
        assert_eq!(rows.len(), 6);
        assert_eq!(
            rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
            [
                ConnectionKind::OauthClient,
                ConnectionKind::AuthorizedApp,
                ConnectionKind::CustomAgent,
                ConnectionKind::AccessToken,
                ConnectionKind::Webhook,
                ConnectionKind::Unknown,
            ]
        );
        assert_eq!(rows[1].acts_as.as_deref(), Some("claude@example.test"));
        assert_eq!(
            rows[1]
                .detail
                .as_ref()
                .and_then(|detail| detail.active_tokens),
            Some(2)
        );
        assert!(
            rows[0].acts_as.is_none(),
            "no actsAs means the user themself"
        );
        assert!(rows[2].managed_on_agents_page());
        assert!(!rows[0].managed_on_agents_page());
    }

    #[test]
    fn an_unknown_kind_is_a_row_this_build_cannot_revoke() {
        let unknown = legacy().pop().expect("a row");
        assert_eq!(unknown.kind, ConnectionKind::Unknown);
        assert!(
            !unknown.revocable,
            "the server said revocable, but the path segment would be one we cannot name"
        );
        assert_eq!(unknown.name, "Something new");
    }

    #[test]
    fn the_revoke_path_pairs_the_rows_own_kind_and_id() {
        let rows = legacy();
        assert_eq!(
            rows[1].revoke_path(),
            "/api/v1/users/me/connections/authorizedApp/dcr-1"
        );
        assert_eq!(
            rows[4].revoke_path(),
            "/api/v1/users/me/connections/webhook/webhook"
        );
        assert_eq!(
            rows[2].revoke_path(),
            "/api/v1/users/me/connections/customAgent/agent-1"
        );
    }

    /// A row with no id cannot be addressed, so it is not a row; an empty list is an empty panel.
    #[test]
    fn a_row_without_an_id_is_dropped_and_nothing_listed_is_empty() {
        assert!(rows(&json!({})).is_empty());
        assert!(rows(&json!({ "connections": [{ "kind": "webhook" }] })).is_empty());
        assert_eq!(rows(&json!([{ "id": "x", "kind": "webhook" }])).len(), 1);
    }

    // ── Sections (AITD-420 / AWTD-981) ───────────────────────────────────────────────────────

    /// The old server sends no `category`, so the page draws what it always drew.
    #[test]
    fn sections_fall_back_to_kinds_when_the_server_predates_the_facets() {
        let rows: Vec<Connection> = legacy()
            .into_iter()
            .filter(|row| row.kind != ConnectionKind::Webhook)
            .collect();
        let sections = sections(&rows);
        assert_eq!(
            sections
                .iter()
                .map(|section| section.heading.clone())
                .collect::<Vec<_>>(),
            [
                SectionHeading::Kind {
                    kind: ConnectionKind::AuthorizedApp
                },
                SectionHeading::Kind {
                    kind: ConnectionKind::OauthClient
                },
                SectionHeading::Kind {
                    kind: ConnectionKind::CustomAgent
                },
                SectionHeading::Kind {
                    kind: ConnectionKind::AccessToken
                },
                SectionHeading::Kind {
                    kind: ConnectionKind::Unknown
                },
            ]
        );
        assert!(
            sections.iter().all(|section| !section.shows_owner_badges),
            "the kind heading already names the row; a badge would say it twice"
        );
        assert_eq!(sections[0].title_key, "connections.kind.authorizedApp");
    }

    /// Three sections, not five: `oauthClient`, `authorizedApp` and `customAgent` are one
    /// credential with three owners, so they collapse into Apps.
    #[test]
    fn groups_by_category_into_apps_tokens_and_webhook() {
        let rows: Vec<Connection> = facets()
            .into_iter()
            .filter(|row| row.kind != ConnectionKind::Unknown)
            .collect();
        let sections = sections(&rows);
        assert_eq!(
            sections
                .iter()
                .map(|section| section.heading.clone())
                .collect::<Vec<_>>(),
            [
                SectionHeading::Category {
                    category: ConnectionCategory::App
                },
                SectionHeading::Category {
                    category: ConnectionCategory::Token
                },
                SectionHeading::Category {
                    category: ConnectionCategory::Webhook
                },
            ],
            "five peer sections collapse to three, in app/token/webhook order"
        );
        assert_eq!(
            sections
                .iter()
                .map(|section| section
                    .rows
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            [
                vec!["c1", "dcr-1", "agent-1"],
                vec!["tok-1"],
                vec!["webhook"]
            ]
        );
        assert!(sections.iter().all(|section| section.shows_owner_badges));
        assert_eq!(sections[0].title_key, "connections.category.app");
    }

    /// The webhook keeps its own section: it is the one row we call OUT to.
    #[test]
    fn the_webhook_is_not_folded_in_with_the_apps() {
        let rows: Vec<Connection> = facets()
            .into_iter()
            .filter(|row| row.kind != ConnectionKind::Unknown)
            .collect();
        let sections = sections(&rows);
        let webhook = sections.last().expect("a section");
        assert_eq!(
            webhook.heading,
            SectionHeading::Category {
                category: ConnectionCategory::Webhook
            }
        );
        assert!(
            webhook.rows[0].acts_as.is_none(),
            "it does not act as the account — it is the reverse direction"
        );
        assert!(webhook.rows[0].scopes.is_empty());
    }

    #[test]
    fn empty_categories_are_omitted() {
        let only_apps: Vec<Connection> = facets()
            .into_iter()
            .filter(|row| row.category == Some(ConnectionCategory::App))
            .collect();
        let sections = sections(&only_apps);
        assert_eq!(
            sections.len(),
            1,
            "a heading over nothing is a category to rule out for no reason"
        );
        assert_eq!(
            sections[0].heading,
            SectionHeading::Category {
                category: ConnectionCategory::App
            }
        );
    }

    /// An app row wears whose it is; a token or a webhook has no owner distinction to draw, and an
    /// owner string this build never heard of draws nothing.
    #[test]
    fn owner_is_the_row_badge_and_only_for_apps() {
        let rows = facets();
        assert_eq!(
            rows.iter().map(|row| row.owner).collect::<Vec<_>>(),
            [
                Some(ConnectionOwner::You),
                Some(ConnectionOwner::ThirdParty),
                Some(ConnectionOwner::Agent),
                None,
                None,
                None,
            ]
        );
    }

    /// A category this build has never heard of is a row it still shows, in its own section — not
    /// a reason to drop back to kind sections for the whole page.
    #[test]
    fn an_unknown_category_gets_its_own_section_rather_than_collapsing_the_page() {
        let rows = facets();
        assert_eq!(
            rows[5].category,
            Some(ConnectionCategory::Unknown),
            "an unrecognised string is unknown, not absent"
        );
        let headings: Vec<SectionHeading> = sections(&rows)
            .into_iter()
            .map(|section| section.heading)
            .collect();
        assert_eq!(
            headings,
            [
                SectionHeading::Category {
                    category: ConnectionCategory::App
                },
                SectionHeading::Category {
                    category: ConnectionCategory::Token
                },
                SectionHeading::Category {
                    category: ConnectionCategory::Webhook
                },
                SectionHeading::Category {
                    category: ConnectionCategory::Unknown
                },
            ]
        );
    }

    /// One row without a `category` means the whole response came from an old deployment.
    #[test]
    fn a_missing_category_is_an_old_server_not_an_uncategorised_row() {
        let mut mixed = vec![legacy().remove(0)];
        mixed.extend(facets().into_iter().skip(1));
        let sections = sections(&mixed);
        assert_eq!(
            sections[0].heading,
            SectionHeading::Kind {
                kind: ConnectionKind::AuthorizedApp
            },
            "the facets are all-or-nothing per response; a gap means a server that predates them"
        );
    }

    // ── The review ───────────────────────────────────────────────────────────────────────────

    /// Idle after ninety days, never used after thirty; an access token records no usage and is
    /// never suggested; a row that cannot be revoked is advice with no button behind it.
    #[test]
    fn the_review_names_the_idle_and_the_never_used_and_nothing_it_cannot_judge() {
        let rows = with_reviews(facets(), now());
        // My script: created 2026-09-01, never used — twenty-four days, not yet thirty.
        assert_eq!(rows[0].review, None);
        // Claude Code: last used six days ago.
        assert_eq!(rows[1].review, None);
        // The access token: would read as never used, but the kind records no usage.
        assert_eq!(rows[3].review, None);
        // The unknown kind cannot be revoked, so it is not suggested either.
        assert_eq!(rows[5].review, None);

        let later = date::parse("2026-12-25T12:00:00Z").expect("an instant");
        let rows = with_reviews(facets(), later);
        assert_eq!(
            rows[0].review,
            Some(ConnectionReview {
                reason: ReviewReason::NeverUsed,
                days: 115
            })
        );
        assert_eq!(
            rows[1].review,
            Some(ConnectionReview {
                reason: ReviewReason::Idle,
                days: 97
            })
        );
        assert_eq!(rows[3].review, None, "an access token is never suggested");
        let panel = panel(&serde_json::from_str(FACETS).expect("json"), later);
        assert_eq!(
            panel.review_count, 4,
            "script, Claude Code, nightly and the webhook"
        );
    }

    // ── Which rows offer an Edit ─────────────────────────────────────────────────────────────

    #[test]
    fn only_an_oauth_client_this_screen_owns_is_editable_here() {
        let row = |value: Value| Connection::from_value(&value).expect("a row");
        assert_eq!(
            row(
                json!({ "id": "c1", "kind": "oauthClient", "manageIn": "connections",
                        "detail": { "clientId": "astrid_client_abc" } })
            )
            .editable_client_id,
            Some("astrid_client_abc".to_string())
        );
        assert_eq!(
            row(
                json!({ "id": "c2", "kind": "oauthClient", "manageIn": "agents",
                        "detail": { "clientId": "astrid_client_xyz" } })
            )
            .editable_client_id,
            None,
            "the Agent Hub owns this one; two owners disagree"
        );
        assert_eq!(
            row(
                json!({ "id": "c3", "kind": "authorizedApp", "manageIn": "connections",
                        "detail": { "activeTokens": 2 } })
            )
            .editable_client_id,
            None,
            "an approved app has no redirect URIs of ours to change"
        );
        assert_eq!(
            row(json!({ "id": "c4", "kind": "oauthClient", "manageIn": "connections" }))
                .editable_client_id,
            None,
            "no client id is a row this build cannot address"
        );
    }

    // ── The draft (AITD-419) ─────────────────────────────────────────────────────────────────

    #[test]
    fn a_draft_without_a_name_is_not_sendable() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "   ".into();
        assert_eq!(draft.problem(), Some(DraftProblem::NameMissing));
        assert!(!draft.is_valid());
    }

    #[test]
    fn a_draft_with_no_grant_type_is_not_sendable() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "My script".into();
        draft.grant_types.clear();
        assert_eq!(draft.problem(), Some(DraftProblem::GrantRequired));
    }

    /// The authorization-code grant sends the user to a browser and back, so the "back" has to be
    /// a URL the server already knows. Without one the client is registerable but unusable.
    #[test]
    fn authorization_code_needs_somewhere_to_come_back_to() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "Browser app".into();
        draft.grant_types = BTreeSet::from([GrantType::AuthorizationCode]);
        assert_eq!(draft.problem(), Some(DraftProblem::RedirectRequired));
        draft.redirect_uri_text = "https://example.test/callback".into();
        assert_eq!(draft.problem(), None);
    }

    #[test]
    fn client_credentials_needs_no_redirect_uri() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "Server script".into();
        assert_eq!(draft.problem(), None);
    }

    #[test]
    fn a_redirect_uri_must_be_an_absolute_http_or_https_url() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "Browser app".into();
        draft.grant_types = BTreeSet::from([GrantType::AuthorizationCode]);
        draft.redirect_uri_text = "myapp://callback".into();
        assert_eq!(
            draft.problem(),
            Some(DraftProblem::RedirectInvalid {
                uri: "myapp://callback".into()
            })
        );
        draft.redirect_uri_text = "/just/a/path".into();
        assert_eq!(
            draft.problem(),
            Some(DraftProblem::RedirectInvalid {
                uri: "/just/a/path".into()
            })
        );
        draft.redirect_uri_text = "http://localhost:3000/callback".into();
        assert_eq!(
            draft.problem(),
            None,
            "http is allowed — a local dev callback is the common case"
        );
    }

    /// The box is one URI per line, and the blank lines a person leaves while typing are not
    /// errors.
    #[test]
    fn redirect_uris_are_one_per_line_with_blanks_and_padding_ignored() {
        let mut draft = OAuthClientDraft::new();
        draft.redirect_uri_text = "  https://a.test/cb  \n\n\thttps://b.test/cb\n".into();
        assert_eq!(
            draft.redirect_uris(),
            vec!["https://a.test/cb", "https://b.test/cb"]
        );
    }

    /// The problem reported is the FIRST one a reader can act on, not a pile of them.
    #[test]
    fn the_problem_reported_is_the_first_one_to_fix() {
        let mut draft = OAuthClientDraft::new();
        draft.grant_types = BTreeSet::from([GrantType::AuthorizationCode]);
        draft.redirect_uri_text = "nonsense".into();
        assert_eq!(draft.problem(), Some(DraftProblem::NameMissing));
    }

    // ── Grant types travel in pairs ──────────────────────────────────────────────────────────

    #[test]
    fn turning_on_refresh_token_also_turns_on_authorization_code() {
        let next = OAuthClientDraft::toggling(
            GrantType::RefreshToken,
            &BTreeSet::from([GrantType::ClientCredentials]),
        );
        assert_eq!(
            next,
            BTreeSet::from([
                GrantType::ClientCredentials,
                GrantType::AuthorizationCode,
                GrantType::RefreshToken
            ])
        );
    }

    #[test]
    fn turning_on_authorization_code_also_turns_on_refresh_token() {
        let next = OAuthClientDraft::toggling(
            GrantType::AuthorizationCode,
            &BTreeSet::from([GrantType::ClientCredentials]),
        );
        assert_eq!(
            next,
            BTreeSet::from([
                GrantType::ClientCredentials,
                GrantType::AuthorizationCode,
                GrantType::RefreshToken
            ])
        );
    }

    #[test]
    fn turning_off_authorization_code_takes_refresh_token_with_it() {
        let next = OAuthClientDraft::toggling(
            GrantType::AuthorizationCode,
            &BTreeSet::from([
                GrantType::ClientCredentials,
                GrantType::AuthorizationCode,
                GrantType::RefreshToken,
            ]),
        );
        assert_eq!(next, BTreeSet::from([GrantType::ClientCredentials]));
    }

    /// Every grant off would be a client that cannot obtain a token by any route, so the last one
    /// cannot be turned off.
    #[test]
    fn the_last_grant_type_cannot_be_turned_off() {
        let next = OAuthClientDraft::toggling(
            GrantType::ClientCredentials,
            &BTreeSet::from([GrantType::ClientCredentials]),
        );
        assert_eq!(next, BTreeSet::from([GrantType::ClientCredentials]));
    }

    // ── The body that goes on the wire ───────────────────────────────────────────────────────

    #[test]
    fn the_create_body_carries_exactly_what_the_server_reads() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "  My script  ".into();
        draft.description = "  Nightly export  ".into();
        draft.scopes = BTreeSet::from(["tasks:read".to_string(), "lists:read".to_string()]);
        let body = draft.create_body();
        assert_eq!(body["name"], "My script", "the name is trimmed");
        assert_eq!(body["description"], "Nightly export");
        assert_eq!(body["scopes"], json!(["lists:read", "tasks:read"]));
        assert_eq!(body["grantTypes"], json!(["client_credentials"]));
        assert!(
            body.get("redirectUris").is_none(),
            "no URIs means the key is absent, not an empty array"
        );
    }

    #[test]
    fn an_empty_description_is_absent_rather_than_an_empty_string() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "My script".into();
        draft.description = "   ".into();
        assert!(draft.create_body().get("description").is_none());
    }

    /// Grant types go out in a stable order so two identical drafts produce identical bodies.
    #[test]
    fn grant_types_are_sent_in_a_stable_order() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "Browser app".into();
        draft.grant_types = BTreeSet::from([
            GrantType::RefreshToken,
            GrantType::AuthorizationCode,
            GrantType::ClientCredentials,
        ]);
        draft.redirect_uri_text = "https://example.test/cb".into();
        assert_eq!(
            draft.create_body()["grantTypes"],
            json!(["client_credentials", "authorization_code", "refresh_token"])
        );
    }

    // ── The scope catalog mirrors the server's ───────────────────────────────────────────────

    #[test]
    fn the_scope_picker_never_offers_the_wildcard_and_has_no_duplicates() {
        assert!(!REGISTERABLE_SCOPES.contains(&"*"));
        assert!(REGISTERABLE_SCOPES.contains(&"tasks:read"));
        assert!(REGISTERABLE_SCOPES.contains(&"chat:write"));
        assert!(REGISTERABLE_SCOPES.contains(&"lists:manage_members"));
        let unique: BTreeSet<&str> = REGISTERABLE_SCOPES.iter().copied().collect();
        assert_eq!(unique.len(), REGISTERABLE_SCOPES.len());
    }

    // ── Decoding a client to edit ────────────────────────────────────────────────────────────

    #[test]
    fn a_client_decodes_from_the_servers_shape_and_survives_missing_fields() {
        let client = OAuthClientSummary::from_value(&json!({
            "client": {"id": "row-1", "clientId": "astrid_client_abc", "name": "My script", "description": null,
                       "redirectUris": ["https://example.test/cb"], "grantTypes": ["client_credentials"],
                       "scopes": ["tasks:read"], "scopeGroup": null, "isActive": true},
            "meta": {"apiVersion": "v1"}
        }))
        .expect("a client");
        assert_eq!(client.client_id, "astrid_client_abc");
        assert_eq!(client.name, "My script");
        assert_eq!(client.description, None);
        assert_eq!(client.redirect_uris, vec!["https://example.test/cb"]);
        assert!(client.is_active);

        let bare = OAuthClientSummary::from_value(&json!({
            "client": {"clientId": "astrid_client_xyz"}, "meta": {"apiVersion": "v1"}
        }))
        .expect("a client");
        assert_eq!(bare.client_id, "astrid_client_xyz");
        assert!(
            bare.redirect_uris.is_empty(),
            "a field the server omitted is empty"
        );
        assert!(bare.is_active, "absent isActive reads as active");
    }

    /// A client loaded for editing fills the draft, so the authorization-code rule applies to an
    /// EDIT too: you cannot empty the redirect URIs of a client that needs one.
    #[test]
    fn loading_a_client_into_a_draft_keeps_its_grant_types_in_force() {
        let client = OAuthClientSummary {
            client_id: "astrid_client_abc".into(),
            name: "Browser app".into(),
            description: None,
            redirect_uris: vec!["https://example.test/cb".into()],
            grant_types: vec!["authorization_code".into(), "refresh_token".into()],
            scopes: vec!["tasks:read".into()],
            is_active: true,
        };
        let mut draft = OAuthClientDraft::from_client(&client);
        assert_eq!(draft.name, "Browser app");
        assert_eq!(
            draft.grant_types,
            BTreeSet::from([GrantType::AuthorizationCode, GrantType::RefreshToken])
        );
        assert_eq!(draft.redirect_uri_text, "https://example.test/cb");
        assert_eq!(draft.problem(), None);
        draft.redirect_uri_text.clear();
        assert_eq!(
            draft.problem(),
            Some(DraftProblem::RedirectRequired),
            "clearing the callback of an authorization-code client must not be savable"
        );
    }

    /// A grant type this build has never heard of must not vanish on a round trip.
    #[test]
    fn an_unknown_grant_type_survives_an_edit() {
        let client = OAuthClientSummary {
            client_id: "astrid_client_abc".into(),
            name: "Future app".into(),
            description: None,
            redirect_uris: vec![],
            grant_types: vec!["client_credentials".into(), "device_code".into()],
            scopes: vec![],
            is_active: true,
        };
        let draft = OAuthClientDraft::from_client(&client);
        assert_eq!(
            draft.grant_types,
            BTreeSet::from([GrantType::ClientCredentials])
        );
        assert_eq!(draft.unrecognized_grant_types, vec!["device_code"]);
        assert_eq!(
            draft.create_body()["grantTypes"],
            json!(["client_credentials", "device_code"])
        );
    }

    /// An edit writes the redirect URIs and nothing else.
    #[test]
    fn an_edit_writes_back_only_the_redirect_uris() {
        let mut draft = OAuthClientDraft::new();
        draft.name = "Browser app".into();
        draft.redirect_uri_text = "https://new.test/cb\nhttps://other.test/cb".into();
        assert_eq!(
            draft.update_body(),
            json!({ "redirectUris": ["https://new.test/cb", "https://other.test/cb"] })
        );
    }

    /// The row carries no secret field at all, and a minted pair is read from the envelope the
    /// server answers with.
    #[test]
    fn a_row_carries_no_secret_and_a_mint_is_read_from_its_envelope() {
        let wire = serde_json::to_value(&legacy()[0]).expect("serialises");
        assert!(wire.get("clientSecret").is_none());
        let minted = MintedClient::from_value(
            &json!({ "client": { "clientId": "astrid_client_abc", "clientSecret": "shh" },
                     "warning": "Save the client_secret now" }),
            "CI",
        );
        assert_eq!(minted.client_id, "astrid_client_abc");
        assert_eq!(minted.client_secret, "shh");
        assert_eq!(
            minted.name, "CI",
            "the name the caller gave, when the server sends none"
        );
    }
}
