//! Connections: what can act as the account, and the OAuth clients it makes by hand.
//!
//! Split out of one dispatch file by domain; the arms in `super::run` call these. The rules are
//! `crate::services::connections`; this file only turns a command into a call and a refusal.

use super::*;
use crate::app::command::OAuthClientDraftFields;
use crate::services::connections::{ConnectionKind, OAuthClientPreset, REGISTERABLE_SCOPES};

/// Stop one connection. A kind this build cannot name is refused before anything is sent: the
/// path segment would be one the server cannot match, and a 404 on screen is a row that will not
/// go away.
pub(super) async fn revoke(app: &App, kind: &str, id: &str) -> Response {
    let kind = ConnectionKind::parse(kind);
    if kind == ConnectionKind::Unknown {
        return Response::failed(Failure::bad_request(
            "that connection is of a kind this build cannot revoke",
        ));
    }
    answer_done(app.context.connections().revoke(kind, id).await)
}

/// Judge a draft, optionally after flipping one grant with its pair.
pub(super) fn check_draft(fields: &OAuthClientDraftFields, toggle_grant: Option<&str>) -> Response {
    use crate::services::connections::{GrantType, OAuthClientDraft};
    let mut draft = fields.to_draft();
    if let Some(grant) = toggle_grant.and_then(GrantType::parse) {
        draft.grant_types = OAuthClientDraft::toggling(grant, &draft.grant_types);
    }
    Response::ok(serde_json::json!({
        "problem": draft.problem(),
        "canSend": draft.is_valid(),
        "grantTypes": draft.wire_grant_types(),
        "redirectUris": draft.redirect_uris(),
        "scopes": REGISTERABLE_SCOPES,
    }))
}

/// Register a client. A draft the core can reject is one the server never sees.
pub(super) async fn create_client(app: &App, fields: &OAuthClientDraftFields) -> Response {
    let draft = fields.to_draft();
    if let Some(problem) = draft.problem() {
        return Response::failed(Failure::bad_request(problem_sentence(&problem)));
    }
    answer(app.context.connections().create_oauth_client(&draft).await)
}

/// Change a client's redirect URIs, under the same rule as a create.
pub(super) async fn update_client(
    app: &App,
    client_id: &str,
    fields: &OAuthClientDraftFields,
) -> Response {
    let draft = fields.to_draft();
    if let Some(problem) = draft.problem() {
        return Response::failed(Failure::bad_request(problem_sentence(&problem)));
    }
    answer(
        app.context
            .connections()
            .update_oauth_client(client_id, &draft)
            .await,
    )
}

pub(super) async fn mint_transport_credentials(app: &App, preset: &str, agent: &str) -> Response {
    let Ok(preset) =
        serde_json::from_value::<OAuthClientPreset>(serde_json::Value::String(preset.to_string()))
    else {
        return Response::failed(Failure::bad_request("that is not a client preset"));
    };
    answer(
        app.context
            .connections()
            .mint_transport_credentials(preset, agent)
            .await,
    )
}

/// For a log and a fallback. The shell words the problem itself, from the `checkOAuthClientDraft`
/// answer, in the reader's language.
fn problem_sentence(problem: &crate::services::connections::DraftProblem) -> String {
    use crate::services::connections::DraftProblem;
    match problem {
        DraftProblem::NameMissing => "a client needs a name".to_string(),
        DraftProblem::GrantRequired => "a client needs at least one grant type".to_string(),
        DraftProblem::RedirectRequired => {
            "the authorization-code grant needs a redirect URI".to_string()
        }
        DraftProblem::RedirectInvalid { uri } => {
            format!("{uri} is not an absolute http or https URL")
        }
    }
}
