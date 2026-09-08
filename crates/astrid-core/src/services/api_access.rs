//! The credentials this account hands to something that is not a person.
//!
//! ## Two credentials, because there are two jobs
//!
//! An **MCP token** is this device swapping the session it already holds for something it can put
//! in a header. `/api/v1/auth/mobile-mcp-token` is the endpoint iOS uses for exactly that, and it
//! is cookie-authenticated on purpose: only a client that is already signed in may mint one. It
//! mints or returns a 90-day token, so asking twice does not litter the account with credentials
//! nobody is holding.
//!
//! An **OAuth client** is a client-credentials pair for a machine that is *not* this one — CI, a
//! script, another person's runner. Registering one requires an interactive session and refuses a
//! delegated token, which the web route says in as many words: a leaked narrow-scope token that
//! could register a client would be a token that could escalate itself.
//!
//! ## A secret is shown once
//!
//! Both endpoints answer with plaintext exactly once and never again — the server stores a hash.
//! So nothing here writes either into the cache, and the shell shows it until the panel closes.
//! Storing it would put a credential on disk that the account cannot rotate by revoking, because
//! nobody would know it was there.
//!
//! ## Why this exists on Windows at all
//!
//! The loops in `docs/AUTOMATION.md` need an MCP token and a client-credentials pair, and the only
//! place to make either was astrid-web's *API Access* settings page. A signed-in client that
//! cannot mint its own credential sends its user to a browser to do something the client is
//! already authorised for.

use serde::Serialize;

use super::{Context, Result};
use crate::api::endpoints;

/// One registered OAuth client, as a screen needs it.
///
/// No secret: the server returns it once at creation and stores a hash. A row that carried a
/// `secret: None` field would suggest one could be read back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClientRow {
    /// What the pair is called. The only thing distinguishing two of them on screen.
    pub name: String,
    /// The public half.
    ///
    /// Safe to show, the half somebody has to copy again later, and — the reason there is no other
    /// id here — what a delete addresses. The route matches `clientId`, not the database row, so a
    /// row carrying its own id would offer an address that answers 404.
    pub client_id: String,
    /// What it may do. Shown because "a pair for CI" and "a pair that can delete every list" look
    /// identical without it.
    pub scopes: Vec<String>,
    pub created_at: Option<String>,
    /// A revoked pair stays in the list saying so, rather than vanishing.
    pub is_active: bool,
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

/// The API-access panel in one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAccess {
    pub clients: Vec<OAuthClientRow>,
}

pub struct ApiAccessService {
    context: Context,
}

impl ApiAccessService {
    pub fn new(context: Context) -> Self {
        ApiAccessService { context }
    }

    /// Mint an MCP token, or hand back the one this account already has.
    ///
    /// The server decides which — the endpoint is "mint or return" — so pressing the button twice
    /// gives the same token rather than a second one, and a screen that lost the first copy can
    /// get it back without revoking anything.
    pub async fn mcp_token(&self) -> Result<String> {
        let request = self
            .context
            .client
            .post(endpoints::MOBILE_MCP_TOKEN)
            .value(serde_json::json!({}));
        let answer: serde_json::Value = self.context.client.send(request).await?;
        Ok(answer
            .get("token")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// Revoke every MCP token this account has minted from a device.
    ///
    /// All of them, not one: the endpoint takes no id, because the token it mints is identified by
    /// its description rather than by anything the holder was given.
    pub async fn revoke_mcp_tokens(&self) -> Result<()> {
        let request = self.context.client.delete(endpoints::MOBILE_MCP_TOKEN);
        self.context.client.send(request).await?;
        Ok(())
    }

    /// The client-credentials pairs this account has registered.
    pub async fn oauth_clients(&self) -> Result<ApiAccess> {
        let request = self.context.client.get(endpoints::OAUTH_CLIENTS);
        let answer: serde_json::Value = self.context.client.send(request).await?;
        Ok(ApiAccess {
            clients: rows(&answer),
        })
    }

    /// Register a pair.
    ///
    /// Nothing but a name is sent. The server's defaults are `client_credentials` with read and
    /// write on tasks and lists, which is what a queue reader needs; naming the scopes here would
    /// be this client's opinion about a grant, restated in a third place and free to drift from
    /// the two that already hold it.
    pub async fn create_oauth_client(&self, name: &str) -> Result<MintedClient> {
        let request = self
            .context
            .client
            .post(endpoints::OAUTH_CLIENTS)
            .value(serde_json::json!({ "name": name }));
        let answer: serde_json::Value = self.context.client.send(request).await?;
        let client = answer.get("client").unwrap_or(&answer);
        Ok(MintedClient {
            client_id: string(client, "clientId"),
            client_secret: string(client, "clientSecret"),
            name: {
                let given = string(client, "name");
                if given.is_empty() {
                    name.to_string()
                } else {
                    given
                }
            },
        })
    }

    pub async fn delete_oauth_client(&self, client_id: &str) -> Result<()> {
        let request = self
            .context
            .client
            .delete(endpoints::oauth_client(client_id));
        self.context.client.send(request).await?;
        Ok(())
    }
}

/// Project whatever the list endpoint answered with.
///
/// The clients arrive under `clients`; a bare array is accepted too, because a list that renders
/// as empty and a list that failed to parse look identical on screen and only one of them is
/// worth a bug report.
fn rows(answer: &serde_json::Value) -> Vec<OAuthClientRow> {
    let list = answer
        .get("clients")
        .and_then(serde_json::Value::as_array)
        .or_else(|| answer.as_array());
    let Some(list) = list else {
        return Vec::new();
    };
    list.iter()
        .map(|client| OAuthClientRow {
            name: string(client, "name"),
            client_id: string(client, "clientId"),
            scopes: client
                .get("scopes")
                .and_then(serde_json::Value::as_array)
                .map(|scopes| {
                    scopes
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            created_at: client
                .get("createdAt")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            // Absent means active: the field is a revocation flag, and a client that arrived
            // without one has not been revoked.
            is_active: client
                .get("isActive")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        })
        .collect()
}

fn string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_client_is_projected_with_its_public_half_and_its_grant() {
        let clients = rows(&json!({
            "clients": [
                {
                    "id": "c1",
                    "clientId": "astrid_client_abc",
                    "name": "CI",
                    "scopes": ["tasks:read", "lists:read"],
                    "createdAt": "2026-09-08T00:00:00.000Z",
                    "isActive": true,
                },
            ],
        }));

        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].client_id, "astrid_client_abc");
        assert_eq!(clients[0].scopes, vec!["tasks:read", "lists:read"]);
        assert!(clients[0].is_active);
    }

    /// The flag is a revocation, so its absence is not a revocation. Defaulting the other way
    /// would draw every pair as dead on a deployment that does not send it.
    #[test]
    fn a_client_with_no_revocation_flag_is_live() {
        let clients = rows(&json!({ "clients": [{ "clientId": "astrid_client_abc" }] }));
        assert!(clients[0].is_active);
    }

    /// The delete route matches the public half. A row carrying the database id as well would
    /// offer a second address, and the second one answers 404.
    #[test]
    fn a_client_is_addressed_by_its_public_half_and_nothing_else() {
        let clients = rows(&json!({
            "clients": [{ "id": "row-1", "clientId": "astrid_client_abc" }],
        }));
        assert_eq!(clients[0].client_id, "astrid_client_abc");
        let wire = serde_json::to_value(&clients[0]).expect("serialises");
        assert!(wire.get("id").is_none(), "one address, not two");
    }

    /// A bare array is accepted as well as the envelope: a list that renders empty and one that
    /// failed to parse look identical on screen, and only one of them is worth a bug report.
    #[test]
    fn a_bare_array_is_read_as_well_as_the_envelope() {
        let clients = rows(&json!([{ "clientId": "astrid_client_abc", "name": "CI" }]));
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].name, "CI");
    }

    /// An account with nothing registered is an empty panel, not a failure.
    #[test]
    fn nothing_registered_is_an_empty_panel() {
        assert!(rows(&json!({})).is_empty());
        assert!(rows(&json!({ "clients": [] })).is_empty());
    }

    /// The row carries no secret field at all. One that did — even as null — would suggest a
    /// secret could be read back, and the server stores only a hash.
    #[test]
    fn a_row_carries_no_secret() {
        let row = OAuthClientRow {
            name: "CI".into(),
            client_id: "astrid_client_abc".into(),
            scopes: vec!["tasks:read".into()],
            created_at: None,
            is_active: true,
        };
        let wire = serde_json::to_value(&row).expect("serialises");
        assert!(wire.get("clientSecret").is_none());
        assert!(wire.get("secret").is_none());
    }
}
