//! Connections, OAuth clients and minted credentials.

use super::*;

/// The panel is the server's rows grouped the way the web groups them — three sections, not
/// five — each row stamped with its review, and the count of rows that look unused up top. The
/// clock is 2026-09-07, so a client made in May and never used is 129 days old.
#[tokio::test]
async fn the_connections_panel_groups_by_category_and_reviews_the_unused() {
    let app = app_with(StubTransport::new().push_json(
        "users/me/connections",
        200,
        json!({
            "connections": [
                { "id": "c1", "kind": "oauthClient", "category": "app", "owner": "you", "name": "My script",
                  "actsAs": null, "scopes": ["tasks:read"], "createdAt": "2026-05-01T10:00:00Z",
                  "lastUsedAt": null, "expiresAt": null, "status": "active", "revocable": true,
                  "manageIn": "connections", "detail": { "clientId": "astrid_client_1" } },
                { "id": "tok-1", "kind": "accessToken", "category": "token", "owner": null,
                  "name": "GitHub Copilot cloud agent", "actsAs": "copilot@astrid.cc", "scopes": ["*"],
                  "createdAt": "2026-05-01T10:00:00Z", "lastUsedAt": null, "expiresAt": "2027-05-01T10:00:00Z",
                  "status": "active", "revocable": true, "manageIn": "agents" },
            ],
            "meta": { "apiVersion": "v1", "authSource": "session", "total": 2 },
        }),
    ));

    let panel = call(&app, json!({ "kind": "connections" })).await;
    assert_eq!(panel["ok"], true, "{panel}");
    let sections = panel["value"]["sections"].as_array().expect("sections");
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0]["titleKey"], "connections.category.app");
    assert_eq!(sections[0]["showsOwnerBadges"], true);
    assert_eq!(sections[0]["rows"][0]["owner"], "you");
    assert_eq!(
        sections[0]["rows"][0]["editableClientId"],
        "astrid_client_1"
    );
    assert_eq!(sections[0]["rows"][0]["review"]["reason"], "neverUsed");
    assert_eq!(sections[0]["rows"][0]["review"]["days"], 129);
    assert_eq!(sections[1]["titleKey"], "connections.category.token");
    assert!(
        sections[1]["rows"][0]["review"].is_null(),
        "an access token records no usage and is never suggested"
    );
    assert_eq!(panel["value"]["reviewCount"], 1);
    assert!(panel["value"]["connections"][0]["clientSecret"].is_null());
}

/// A revoke names the row's kind and id together, since ids repeat across kinds; a kind this
/// build cannot name is refused before anything is sent.
#[tokio::test]
async fn revoking_a_connection_addresses_it_by_kind_and_id_and_refuses_an_unknown_kind() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "users/me/connections",
        200,
        json!({ "success": true, "kind": "authorizedApp", "id": "dcr-1" }),
    ));

    let done = call(
        &app,
        json!({ "kind": "revokeConnection", "connectionKind": "authorizedApp", "id": "dcr-1" }),
    )
    .await;
    assert_eq!(done["ok"], true, "{done}");
    let sent = transport.requests();
    assert!(
        sent.iter()
            .any(|request| request.method.as_str() == "DELETE"
                && request
                    .url
                    .ends_with("/api/v1/users/me/connections/authorizedApp/dcr-1")),
        "sent: {:?}",
        sent.iter().map(|request| &request.url).collect::<Vec<_>>()
    );

    let refused = call(
        &app,
        json!({ "kind": "revokeConnection", "connectionKind": "quantumLink", "id": "future-1" }),
    )
    .await;
    assert_eq!(refused["ok"], false);
    assert_eq!(
        transport.requests().len(),
        1,
        "an unknown kind never reaches the server"
    );
}

/// The secret exists in plaintext exactly once, in this answer. The body carries what the
/// developer console sends: a trimmed name, the scopes, the grants in wire order.
#[tokio::test]
async fn registering_a_client_answers_with_the_secret_shown_once() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "oauth/clients",
        201,
        json!({
            "client": { "clientId": "astrid_client_abc", "clientSecret": "shown-once", "name": "Windows fixall" },
            "warning": "Save the client_secret now",
        }),
    ));

    let minted = call(
        &app,
        json!({
            "kind": "createOAuthClient",
            "name": "  Windows fixall  ",
            "description": "",
            "scopes": ["tasks:read", "lists:read"],
            "grantTypes": ["client_credentials"],
            "redirectUriText": "",
        }),
    )
    .await;

    assert_eq!(minted["ok"], true, "{minted}");
    assert_eq!(minted["value"]["clientId"], "astrid_client_abc");
    assert_eq!(minted["value"]["clientSecret"], "shown-once");
    let sent = transport.requests();
    let body: serde_json::Value =
        serde_json::from_slice(&sent[0].body.clone().unwrap_or_default()).expect("a body");
    assert_eq!(body["name"], "Windows fixall");
    assert_eq!(body["grantTypes"], json!(["client_credentials"]));
    assert!(body.get("description").is_none(), "{body}");
    assert!(body.get("redirectUris").is_none(), "{body}");
}

/// A draft the core can reject is one the server never sees.
#[tokio::test]
async fn an_invalid_client_draft_never_reaches_the_server() {
    let (app, transport) = app_and_transport(StubTransport::new());

    let refused = call(
        &app,
        json!({ "kind": "createOAuthClient", "name": "   ", "grantTypes": ["client_credentials"] }),
    )
    .await;

    assert_eq!(refused["ok"], false);
    assert!(transport.requests().is_empty());
}

/// The editor asks the core on every keystroke: the first problem to fix, and the grants after
/// a toggle — with `refresh_token` and `authorization_code` travelling together, and the last
/// grant refusing to go.
#[tokio::test]
async fn checking_a_draft_names_the_first_problem_and_toggles_grants_in_pairs() {
    let app = app_with(StubTransport::new());

    let check = call(
        &app,
        json!({
            "kind": "checkOAuthClientDraft",
            "name": "Browser app",
            "grantTypes": ["client_credentials"],
            "toggleGrant": "authorization_code",
        }),
    )
    .await;
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!(
        check["value"]["grantTypes"],
        json!(["client_credentials", "authorization_code", "refresh_token"])
    );
    assert_eq!(check["value"]["problem"]["key"], "redirectRequired");
    assert_eq!(check["value"]["canSend"], false);
    assert!(check["value"]["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .any(|scope| scope == "chat:write"));

    let last = call(
        &app,
        json!({
            "kind": "checkOAuthClientDraft",
            "name": "Script",
            "grantTypes": ["client_credentials"],
            "toggleGrant": "client_credentials",
        }),
    )
    .await;
    assert_eq!(
        last["value"]["grantTypes"],
        json!(["client_credentials"]),
        "the last grant cannot be turned off"
    );
    assert_eq!(last["value"]["canSend"], true);

    let bad_uri = call(
        &app,
        json!({
            "kind": "checkOAuthClientDraft",
            "name": "Browser app",
            "grantTypes": ["authorization_code", "refresh_token"],
            "redirectUriText": "myapp://cb",
        }),
    )
    .await;
    assert_eq!(bad_uri["value"]["problem"]["key"], "redirectInvalid");
    assert_eq!(bad_uri["value"]["problem"]["uri"], "myapp://cb");
}

/// An edit loads the client — the list knows its id but not its redirect URIs — and writes back
/// only the URIs.
#[tokio::test]
async fn editing_a_client_loads_it_and_writes_back_only_its_redirect_uris() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "oauth/clients/astrid_client_abc",
                200,
                json!({
                    "client": { "clientId": "astrid_client_abc", "name": "Browser app",
                                "redirectUris": ["https://old.test/cb"],
                                "grantTypes": ["authorization_code", "refresh_token"],
                                "scopes": ["tasks:read"], "isActive": true },
                }),
            )
            .push_json(
                "oauth/clients/astrid_client_abc",
                200,
                json!({
                    "client": { "clientId": "astrid_client_abc", "name": "Browser app",
                                "redirectUris": ["https://new.test/cb"],
                                "grantTypes": ["authorization_code", "refresh_token"],
                                "scopes": ["tasks:read"], "isActive": true },
                }),
            ),
    );

    let loaded = call(
        &app,
        json!({ "kind": "loadOAuthClient", "clientId": "astrid_client_abc" }),
    )
    .await;
    assert_eq!(loaded["ok"], true, "{loaded}");
    assert_eq!(
        loaded["value"]["redirectUris"],
        json!(["https://old.test/cb"])
    );
    assert_eq!(
        loaded["value"]["grantTypes"],
        json!(["authorization_code", "refresh_token"])
    );

    let updated = call(
        &app,
        json!({
            "kind": "updateOAuthClient",
            "clientId": "astrid_client_abc",
            "name": "Browser app",
            "grantTypes": ["authorization_code", "refresh_token"],
            "redirectUriText": "https://new.test/cb\n\nhttps://other.test/cb",
        }),
    )
    .await;
    assert_eq!(updated["ok"], true, "{updated}");
    let sent = transport.requests();
    let put = sent
        .iter()
        .find(|request| request.method.as_str() == "PUT")
        .expect("a PUT");
    let body: serde_json::Value =
        serde_json::from_slice(&put.body.clone().unwrap_or_default()).expect("a body");
    assert_eq!(
        body,
        json!({ "redirectUris": ["https://new.test/cb", "https://other.test/cb"] })
    );

    // Clearing the callback of an authorization-code client is not savable.
    let refused = call(
        &app,
        json!({
            "kind": "updateOAuthClient",
            "clientId": "astrid_client_abc",
            "name": "Browser app",
            "grantTypes": ["authorization_code", "refresh_token"],
            "redirectUriText": "",
        }),
    )
    .await;
    assert_eq!(refused["ok"], false);
}

/// The preset request carries only the preset and the agent: scopes and grant types are the
/// server's decision, never the client's.
#[tokio::test]
async fn minting_transport_credentials_sends_only_the_preset_and_the_agent() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "oauth/clients",
        201,
        json!({ "client": { "clientId": "astrid_client_minted", "clientSecret": "shh" } }),
    ));

    let minted = call(
        &app,
        json!({ "kind": "mintTransportCredentials", "preset": "webhookServer", "agent": "claude" }),
    )
    .await;

    assert_eq!(minted["ok"], true, "{minted}");
    assert_eq!(minted["value"]["clientId"], "astrid_client_minted");
    assert_eq!(minted["value"]["clientSecret"], "shh");
    let sent = transport.requests();
    let body: serde_json::Value =
        serde_json::from_slice(&sent[0].body.clone().unwrap_or_default()).expect("a body");
    assert_eq!(
        body,
        json!({ "preset": "webhookServer", "agent": "claude" })
    );

    let refused = call(
        &app,
        json!({ "kind": "mintTransportCredentials", "preset": "anything", "agent": "claude" }),
    )
    .await;
    assert_eq!(refused["ok"], false);
}
