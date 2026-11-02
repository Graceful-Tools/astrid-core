//! Lists: settings, sharing, images and membership.

use super::*;

/// A list's agent and repository are offered from what the account can use, and written
/// the way the server prefers them (task f44b4a0c).
#[tokio::test]
async fn a_list_s_agent_and_repository_are_offered_and_written_task_f44b4a0c() {
    let transport = StubTransport::new()
            .push_json(
                "/available-agents",
                200,
                json!({ "agents": [
                    { "id": "ai-agent-claude", "name": "Claude Agent", "isAIAgent": true },
                    { "id": "ai-agent-codex", "name": "Codex" }
                ] }),
            )
            .push_json(
                "/github/repositories",
                200,
                json!({ "repositories": [
                    { "id": 1, "name": "astrid-windows", "fullName": "Graceful-Tools/astrid-windows", "private": true }
                ] }),
            )
            .push_json("/members", 200, json!({ "members": [] }));
    let app = app_with(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({
                "id": "l1", "name": "Work",
                "aiAgentConfig": { "enabledTypes": ["claude_agent"], "defaultAgentId": null }
            }))
            .expect("a list"),
        )
        .expect("stores");

    let options = call(&app, json!({ "kind": "listAgentOptions", "listId": "l1" })).await;
    assert_eq!(options["ok"], true, "{options}");
    assert_eq!(options["value"]["agents"][0]["id"], "ai-agent-claude");
    assert_eq!(options["value"]["agents"][0]["name"], "Claude Agent");
    assert_eq!(options["value"]["agents"].as_array().unwrap().len(), 2);
    assert_eq!(
        options["value"]["repositories"][0]["fullName"],
        "Graceful-Tools/astrid-windows"
    );
    assert_eq!(options["value"]["githubConnected"], true);
    assert!(options["value"]["defaultAgentId"].is_null());

    let set = call(
            &app,
            json!({
                "kind": "updateList", "listId": "l1",
                "changes": { "defaultAgentId": "ai-agent-claude", "githubRepositoryId": "Graceful-Tools/astrid-windows" }
            }),
        )
        .await;
    assert_eq!(set["ok"], true, "{set}");
    // The list carries both, and the enabled types it had are kept beside the agent.
    assert_eq!(
        set["value"]["aiAgentConfig"]["defaultAgentId"],
        "ai-agent-claude"
    );
    assert_eq!(
        set["value"]["aiAgentConfig"]["enabledTypes"][0],
        "claude_agent"
    );
    assert_eq!(
        set["value"]["githubRepositoryId"],
        "Graceful-Tools/astrid-windows"
    );
    let settings = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
    assert_eq!(settings["value"]["defaultAgentId"], "ai-agent-claude");
    assert_eq!(
        settings["value"]["githubRepositoryId"],
        "Graceful-Tools/astrid-windows"
    );

    // Cleared with an explicit null, which is how "account default" and "no repository"
    // are told apart from "leave it".
    let cleared = call(
        &app,
        json!({
            "kind": "updateList", "listId": "l1",
            "changes": { "defaultAgentId": null, "githubRepositoryId": null }
        }),
    )
    .await;
    assert!(cleared["value"]["aiAgentConfig"]["defaultAgentId"].is_null());
    assert!(cleared["value"]["githubRepositoryId"].is_null());
}

/// GitHub not connected is not an error: no repositories, and the answer says so.
#[tokio::test]
async fn without_github_the_repositories_are_empty_and_the_answer_says_why() {
    let transport = StubTransport::new()
        .push_json("/available-agents", 200, json!({ "agents": [] }))
        .push_json(
            "/github/repositories",
            400,
            json!({ "error": "GitHub is not connected" }),
        );
    let app = app_with(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work" })).expect("a list"),
        )
        .expect("stores");

    let options = call(&app, json!({ "kind": "listAgentOptions", "listId": "l1" })).await;

    assert_eq!(options["ok"], true, "{options}");
    assert_eq!(options["value"]["githubConnected"], false);
    assert!(options["value"]["repositories"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn asking_for_a_list_that_is_not_there_says_which_one() {
    let app = app_with(StubTransport::new());
    let answer = call(&app, json!({ "kind": "list", "listId": "nope" })).await;
    assert_eq!(answer["error"]["kind"], "notFound");
    assert_eq!(answer["error"]["id"], "nope");
}

/// The owner is offered a handover where a member is offered Leave; the probe's four answers
/// reach the shell as four states, and the handover is one POST that drops the list here.
#[tokio::test]
async fn an_owner_is_offered_a_handover_and_the_probe_keeps_its_four_answers_apart() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "transfer-ownership",
                200,
                json!({ "eligibleOwners": [
                    { "id": "dana", "name": "Dana", "email": "dana@x.io" }
                ]}),
            )
            .push_json("transfer-ownership", 200, json!({ "eligibleOwners": [] }))
            .push_json(
                "transfer-ownership",
                403,
                json!({ "error": "Only the owner can transfer" }),
            )
            .push_json("transfer-ownership", 404, json!({ "error": "not found" }))
            .push_json(
                "transfer-ownership",
                200,
                json!({ "message": "Ownership transferred successfully" }),
            ),
    );
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "ownerId": "me" }))
                .expect("a list"),
        )
        .expect("stores");

    let settings = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
    assert_eq!(
        settings["value"]["canTransferOwnership"], true,
        "{settings}"
    );
    assert_eq!(
        settings["value"]["canLeave"], false,
        "an owner is not offered a plain Leave"
    );

    let probe = call(&app, json!({ "kind": "eligibleNewOwners", "listId": "l1" })).await;
    assert_eq!(probe["ok"], true, "{probe}");
    assert_eq!(probe["value"]["availability"], "available");
    assert_eq!(probe["value"]["owners"][0]["id"], "dana");

    let nobody = call(&app, json!({ "kind": "eligibleNewOwners", "listId": "l1" })).await;
    assert_eq!(nobody["value"]["availability"], "noEligibleOwners");

    let refused = call(&app, json!({ "kind": "eligibleNewOwners", "listId": "l1" })).await;
    assert_eq!(refused["ok"], true, "a refusal is a state, not an error");
    assert_eq!(refused["value"]["availability"], "notPermitted");

    let undeployed = call(&app, json!({ "kind": "eligibleNewOwners", "listId": "l1" })).await;
    assert_eq!(undeployed["value"]["availability"], "unavailable");

    let done = call(
        &app,
        json!({ "kind": "transferListOwnership", "listId": "l1", "newOwnerId": "dana" }),
    )
    .await;
    assert_eq!(done["ok"], true, "{done}");
    let sent = transport.requests();
    let post = sent
        .iter()
        .find(|request| request.method.as_str() == "POST")
        .expect("a POST");
    assert!(post.url.ends_with("/api/v1/lists/l1/transfer-ownership"));
    assert!(app.store.list("l1").expect("reads").is_none());
}

/// A public list shown to somebody not on it answers an EMPTY roster rather than a refusal; the
/// server's `user_role` is what tells "not shown to you" from "nobody here" (Apple 4a338b53).
#[tokio::test]
async fn a_refreshed_roster_says_how_this_account_sees_the_list() {
    let transport = StubTransport::new().push_json(
        "/members",
        200,
        json!({ "members": [], "user_role": "viewer" }),
    );
    let app = app_with(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Public", "privacy": "PUBLIC" }))
                .expect("a list"),
        )
        .expect("stores");
    let roster = call(
        &app,
        json!({ "kind": "refreshListMembers", "listId": "l1" }),
    )
    .await;
    assert_eq!(roster["value"]["viewerRole"], "viewer", "{roster}");
}

/// Membership comes with what this account may do about it, decided by the permission rules
/// rather than by a screen — the failure mode is a control that 403s, or one that quietly is
/// not offered to somebody who should have it.
#[tokio::test]
async fn list_members_come_with_what_this_account_may_do() {
    let transport = StubTransport::new().push_json(
        "/members",
        200,
        json!({
            "members": [
                { "userId": "me", "role": "owner", "user": { "id": "me", "name": "Jon" } },
                { "userId": "dana", "role": "member", "user": { "id": "dana", "name": "Dana" } },
            ]
        }),
    );
    let app = app_with(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({
                "id": "l1", "name": "Work", "ownerId": "me", "privacy": "SHARED"
            }))
            .expect("a list"),
        )
        .expect("stores");
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    // The cache has no roster yet, and the answer says so at once rather than waiting.
    let cached = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
    assert_eq!(cached["ok"], true, "{cached}");
    assert_eq!(
        cached["value"]["members"]
            .as_array()
            .expect("members")
            .len(),
        0
    );
    assert_eq!(
        cached["value"]["name"], "Work",
        "the list's own facts need no network"
    );

    let answered = call(
        &app,
        json!({ "kind": "refreshListMembers", "listId": "l1" }),
    )
    .await;
    assert_eq!(
        answered["value"]["members"]
            .as_array()
            .expect("members")
            .len(),
        2
    );
    assert_eq!(answered["value"]["canManageMembers"], true);
    // The owner cannot leave their own list — that would strand it, which is what deleting is
    // for.
    assert_eq!(answered["value"]["canLeave"], false);

    // And the roster is kept: the next open draws it without asking again.
    let again = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
    assert_eq!(
        again["value"]["members"].as_array().expect("members").len(),
        2
    );
}

/// A GitHub-backed board keeps its roster in the project, not in Astrid
/// (`supports.membership: false`, astrid-web's GITHUB_PROJECTS_WHITELABEL §11.3): none of the
/// controls that would change it are offered, and it is the core that says so. The permission
/// rules alone would answer yes to all three here — the owner of a shared list may invite, hand
/// over and (on somebody else's) leave — so a shell left to combine the two answers itself would
/// be a fourth implementation of the rule, and the one that 403s (task 93ffa9ea).
#[tokio::test]
async fn a_board_that_does_not_do_membership_offers_nothing_that_would_change_it() {
    let app = app_with(StubTransport::new());
    let store = |value: serde_json::Value| {
        app.store
            .upsert_list(&serde_json::from_value(value).expect("a list"))
            .expect("stores");
    };
    // Mine, so the rules alone would offer inviting and handing over.
    store(json!({
        "id": "gh", "name": "Roadmap", "ownerId": "me", "privacy": "SHARED",
        "backend": "github_project",
        "supports": { "repeatMode": "spawn", "membership": false, "privateTasks": false },
    }));
    // Somebody else's, so the rules alone would offer leaving.
    store(json!({
        "id": "ghTheirs", "name": "Theirs", "ownerId": "dana", "privacy": "SHARED",
        "backend": "github_project",
        "supports": { "repeatMode": "spawn", "membership": false },
    }));
    // A `supports` block that simply does not mention membership is a classic list saying
    // nothing, not a board saying no.
    store(json!({
        "id": "quiet", "name": "Quiet", "ownerId": "me", "privacy": "SHARED",
        "supports": { "priority": false },
    }));
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    let mine = call(&app, json!({ "kind": "listMembers", "listId": "gh" })).await;
    assert_eq!(mine["ok"], true, "{mine}");
    assert_eq!(
        mine["value"]["canManageMembers"], false,
        "nobody is invited onto a GitHub board from here"
    );
    assert_eq!(
        mine["value"]["canTransferOwnership"], false,
        "a roster Astrid does not hold cannot be handed to anyone"
    );
    // What the list itself is — its name, colour, columns — is still Astrid's, and still the
    // owner's to change. Only the roster is the project's.
    assert_eq!(mine["value"]["canManageList"], true);
    assert_eq!(mine["value"]["canDeleteList"], true);

    let theirs = call(&app, json!({ "kind": "listMembers", "listId": "ghTheirs" })).await;
    assert_eq!(
        theirs["value"]["canLeave"], false,
        "there is no Astrid membership to leave"
    );

    let quiet = call(&app, json!({ "kind": "listMembers", "listId": "quiet" })).await;
    assert_eq!(
        quiet["value"]["canManageMembers"], true,
        "an absent key means what it means on a classic list"
    );
}

/// The flyout opens offline. One network call in here used to take the list's name, colour,
/// privacy and permissions — none of which need a connection — down with it.
#[tokio::test]
async fn list_settings_open_from_the_cache_when_the_server_is_away() {
    let app = app_with(StubTransport::new().fallback(Err(
        crate::api::TransportError::Unreachable("tunnel".into()),
    )));
    app.store
        .upsert_list(
            &serde_json::from_value(json!({
                "id": "l1", "name": "Work", "ownerId": "me", "color": "#ef4444",
                "listMembers": [
                    { "userId": "me", "role": "owner", "user": { "id": "me", "name": "Jon" } }
                ]
            }))
            .expect("a list"),
        )
        .expect("stores");

    let answered = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
    assert_eq!(answered["ok"], true, "{answered}");
    assert_eq!(answered["value"]["name"], "Work");
    assert_eq!(answered["value"]["color"], "#ef4444");
    assert_eq!(
        answered["value"]["members"]
            .as_array()
            .expect("members")
            .len(),
        1
    );

    let refreshed = call(
        &app,
        json!({ "kind": "refreshListMembers", "listId": "l1" }),
    )
    .await;
    assert_eq!(
        refreshed["ok"], false,
        "the refresh is the part that needs the server"
    );
    assert_eq!(refreshed["error"]["kind"], "offline");
}

/// The settings screen needs the look and the visibility of the list beside its members
/// (task 53780e75): the colour every screen draws, the web's palette to choose from,
/// privacy as stored, and whether it is a favourite.
#[tokio::test]
async fn list_settings_carry_colour_privacy_and_favourite_task_53780e75() {
    let transport = StubTransport::new().push_json("/members", 200, json!({ "members": [] }));
    let app = app_with(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({
                "id": "l1", "name": "Work", "ownerId": "me",
                "color": "#ef4444", "privacy": "PUBLIC", "isFavorite": true
            }))
            .expect("a list"),
        )
        .expect("stores");

    let answered = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;

    assert_eq!(answered["value"]["color"], "#ef4444");
    assert_eq!(answered["value"]["privacy"], "PUBLIC");
    assert_eq!(answered["value"]["isFavorite"], true);
    assert_eq!(
        answered["value"]["colorChoices"]
            .as_array()
            .expect("a palette")
            .len(),
        crate::rows::list_picks::LIST_COLOR_PALETTE.len()
    );

    // A list the server never said the privacy of: the answer says nothing rather than
    // "private", which a screen would then offer to change back to.
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l2", "name": "Thin", "ownerId": "me" }))
                .expect("a list"),
        )
        .expect("stores");
    let app = app_with(StubTransport::new().push_json("/members", 200, json!({ "members": [] })));
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l2", "name": "Thin", "ownerId": "me" }))
                .expect("a list"),
        )
        .expect("stores");
    let thin = call(&app, json!({ "kind": "listMembers", "listId": "l2" })).await;
    assert!(thin["value"]["privacy"].is_null());
    assert_eq!(
        thin["value"]["color"], "#3b82f6",
        "the default every client draws"
    );
}

#[tokio::test]
async fn members_of_a_list_that_is_not_there_are_a_not_found() {
    let app = app_with(StubTransport::new());
    let answered = call(&app, json!({ "kind": "listMembers", "listId": "nope" })).await;
    assert_eq!(answered["error"]["kind"], "notFound");
}

/// An invitation reaches the network rather than the Outbox: queuing one offline would show a
/// member who does not exist, and the optimistic row would be indistinguishable from a real one
/// to every permission check that read it afterwards.
#[tokio::test]
async fn an_invitation_goes_straight_to_the_server() {
    let transport = StubTransport::new().push_json("/members", 200, json!({ "ok": true }));
    let app = app_with(transport);

    let answered = call(
        &app,
        json!({
            "kind": "inviteToList",
            "listId": "l1",
            "email": "dana@example.test",
            "role": "member",
        }),
    )
    .await;
    assert_eq!(answered["ok"], true);
    assert_eq!(
        call(&app, json!({ "kind": "outboxStats" })).await["value"]["pending"],
        0,
        "an invitation is not a local fact and does not belong in the Outbox"
    );
}

/// A list's picture (task 3a913e52): chosen from disk it goes through the secure-upload route
/// with the list as context and its address is written on the list; asked for, a secure file is
/// fetched once into the cache and answered as a local path, a site path is made absolute, and
/// a list without one answers nothing.
/// AATD-39: an image stored under the LEGACY secure-file path (`/api/secure-files/<id>`, older
/// lists still carry it) is a secure file too — fetched with the session and answered locally, as
/// iOS's ListImageHelper accepts both. Made absolute instead, it needed a cookie no image control has.
#[tokio::test]
async fn aatd39_a_legacy_secure_file_list_image_is_fetched_with_the_session() {
    use crate::api::transport::HttpResponse;
    let transport = StubTransport::new().push(
        "/api/v1/secure-files/file_old",
        Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "image/png".into())],
            body: b"\x89PNG".to_vec(),
        }),
    );
    let (app, _transport) = app_and_transport(transport);
    call(&app, json!({ "kind": "createList", "name": "Bugs" })).await;
    let bugs = list_id_named(&app, "Bugs").await;
    call(
        &app,
        json!({ "kind": "updateList", "listId": bugs, "changes": { "imageUrl": "/api/secure-files/file_old" } }),
    )
    .await;
    let image = call(&app, json!({ "kind": "listImage", "listId": bugs })).await;
    assert_eq!(image["ok"], true, "{image}");
    assert_eq!(image["value"]["isLocal"], true, "{image}");
    let local = image["value"]["source"]
        .as_str()
        .expect("a path")
        .to_string();
    assert!(local.ends_with("file_old.png"), "{local}");
    let _ = std::fs::remove_file(&local);
}

/// AATD-39: a public list is not in this store, but its picture is a secure file all the same —
/// handed over by address it is fetched with the session, as the reader's own lists' are.
#[tokio::test]
async fn aatd39_a_public_lists_picture_is_fetched_by_its_address() {
    use crate::api::transport::HttpResponse;
    let transport = StubTransport::new().push(
        "/api/v1/secure-files/file_public",
        Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "image/png".into())],
            body: b"\x89PNG".to_vec(),
        }),
    );
    let (app, _transport) = app_and_transport(transport);
    let image = call(
        &app,
        json!({ "kind": "listImage", "listId": "not-in-the-store", "imageUrl": "/api/secure-files/file_public" }),
    )
    .await;
    assert_eq!(image["ok"], true, "{image}");
    assert_eq!(image["value"]["isLocal"], true, "{image}");
    let local = image["value"]["source"]
        .as_str()
        .expect("a path")
        .to_string();
    assert!(local.ends_with("file_public.png"), "{local}");
    let _ = std::fs::remove_file(&local);
    let none = call(
        &app,
        json!({ "kind": "listImage", "listId": "x", "imageUrl": " " }),
    )
    .await;
    assert_eq!(none["value"], serde_json::Value::Null, "{none}");
}

#[tokio::test]
async fn a_list_image_is_uploaded_stored_and_fetched_task_3a913e52() {
    use crate::api::transport::HttpResponse;
    let transport = StubTransport::new()
        .push_json(
            "/api/v1/secure-upload/request-upload",
            200,
            json!({ "fileId": "file_pic" }),
        )
        .push(
            "/api/v1/secure-files/file_pic",
            Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "image/png".into())],
                body: b"\x89PNG".to_vec(),
            }),
        );
    let (app, transport) = app_and_transport(transport);
    call(&app, json!({ "kind": "createList", "name": "Garden" })).await;
    let garden = list_id_named(&app, "Garden").await;

    let picture = std::env::temp_dir().join(format!(
        "astrid-list-image-{}.png",
        crate::outbox::new_temp_id()
    ));
    std::fs::write(&picture, b"\x89PNG").expect("writes");
    let set = call(
        &app,
        json!({ "kind": "setListImage", "listId": garden, "path": picture.to_string_lossy() }),
    )
    .await;
    let _ = std::fs::remove_file(&picture);
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["value"]["imageUrl"], "/api/v1/secure-files/file_pic");

    let upload = transport
        .requests()
        .into_iter()
        .find(|request| {
            request
                .url
                .ends_with("/api/v1/secure-upload/request-upload")
        })
        .expect("an upload");
    let body = String::from_utf8_lossy(upload.body.as_deref().expect("bytes")).into_owned();
    assert!(body.contains("name=\"file\""), "{body}");
    assert!(body.contains("image/png"), "{body}");
    assert!(
        body.contains(&garden),
        "the list travels as the upload's context: {body}"
    );

    let settings = call(&app, json!({ "kind": "listMembers", "listId": garden })).await;
    assert_eq!(
        settings["value"]["imageUrl"],
        "/api/v1/secure-files/file_pic"
    );

    // The picture, fetched once with the signed-in client and kept.
    let first = call(&app, json!({ "kind": "listImage", "listId": garden })).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["value"]["isLocal"], true);
    let local = first["value"]["source"]
        .as_str()
        .expect("a path")
        .to_string();
    assert!(local.ends_with("file_pic.png"), "{local}");
    assert_eq!(std::fs::read(&local).expect("cached"), b"\x89PNG");
    let again = call(&app, json!({ "kind": "listImage", "listId": garden })).await;
    assert_eq!(again["value"]["source"], local);
    let fetches = transport
        .requests()
        .iter()
        .filter(|request| request.url.contains("/secure-files/file_pic"))
        .count();
    assert_eq!(fetches, 1, "the second answer came from the cache");
    let _ = std::fs::remove_file(&local);

    // A site path — the web's placeholders — is made absolute; an address is passed through.
    call(
        &app,
        json!({ "kind": "updateList", "listId": garden, "changes": { "imageUrl": "/placeholders/blue.svg" } }),
    )
    .await;
    let placeholder = call(&app, json!({ "kind": "listImage", "listId": garden })).await;
    assert_eq!(
        placeholder["value"]["source"],
        "https://astrid.cc/placeholders/blue.svg"
    );
    assert_eq!(placeholder["value"]["isLocal"], false);

    // Removed: nothing to draw.
    call(
        &app,
        json!({ "kind": "updateList", "listId": garden, "changes": { "imageUrl": null } }),
    )
    .await;
    let none = call(&app, json!({ "kind": "listImage", "listId": garden })).await;
    assert_eq!(none["ok"], true, "{none}");
    assert_eq!(none["value"], serde_json::Value::Null);
}

/// Public lists are browsed from the server and one is copied into the account (task f6bc59e8):
/// the copy comes back cached with its tasks, so the sidebar and the list draw at once.
#[tokio::test]
async fn public_lists_are_browsed_and_one_is_copied_task_f6bc59e8() {
    let transport = StubTransport::new()
        .push_json(
            "/api/v1/public/lists",
            200,
            json!({ "lists": [{
                "id": "pub1", "name": "Recipes", "description": "Weeknight dinners",
                "privacy": "PUBLIC", "owner": { "id": "them", "name": "Dana", "email": "dana@x.io" },
                "taskCount": 3, "memberCount": 2,
            }] }),
        )
        .push_json(
            "/api/v1/lists/pub1/copy",
            200,
            json!({ "list": {
                "id": "mine1", "name": "Recipes", "privacy": "PRIVATE", "ownerId": "me",
                "tasks": [{ "id": "t9", "title": "Bake bread", "listIds": ["mine1"] }],
            } }),
        );
    let (app, transport) = app_and_transport(transport);

    let browsed = call(&app, json!({ "kind": "publicLists" })).await;
    assert_eq!(browsed["ok"], true, "{browsed}");
    assert_eq!(browsed["value"][0]["name"], "Recipes");
    assert_eq!(browsed["value"][0]["owner"]["name"], "Dana");
    assert_eq!(browsed["value"][0]["taskCount"], 3);
    assert_eq!(browsed["value"][0]["memberCount"], 2);
    let listing = transport
        .requests()
        .into_iter()
        .find(|request| request.url.contains("/api/v1/public/lists"))
        .expect("a listing");
    assert!(listing.url.contains("sortBy=popular"), "{}", listing.url);

    let copied = call(&app, json!({ "kind": "copyList", "listId": "pub1" })).await;
    assert_eq!(copied["ok"], true, "{copied}");
    assert_eq!(copied["value"]["id"], "mine1");
    let post = transport
        .requests()
        .into_iter()
        .find(|request| request.url.ends_with("/api/v1/lists/pub1/copy"))
        .expect("a copy");
    let body: serde_json::Value =
        serde_json::from_slice(post.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["includeTasks"], true);

    // Cached at once, tasks and all.
    let lists = call(&app, json!({ "kind": "lists" })).await;
    assert!(lists["value"]
        .as_array()
        .unwrap()
        .iter()
        .any(|list| list["id"] == "mine1"));
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": "mine1" })).await;
    assert_eq!(rows["value"]["rows"][0]["title"], "Bake bread");
}

/// One public list is opened by its id, for somebody who is not a member of it (task c5f7d870) —
/// the read Report an Issue needs and `publicLists` (a catalogue) and `copyList` (which leaves a
/// copy in the sidebar) could not give it.
///
/// Three things are asserted, and the third is the point: the answer carries the reader's
/// permissions beside the list, those permissions follow `publicListType`, and the list **does not
/// enter the cache**. Filing a bug must not put the Bugs & Feedback list in somebody's sidebar.
#[tokio::test]
async fn one_public_list_is_opened_by_id_and_not_cached_task_c5f7d870() {
    let transport = StubTransport::new()
        .push_json(
            "/api/v1/lists/collab",
            200,
            json!({ "list": {
                "id": "collab", "name": "Bugs & Feedback", "ownerId": "them",
                "privacy": "PUBLIC", "publicListType": "collaborative",
                "owner": { "id": "them", "name": "Dana" },
            } }),
        )
        .push_json(
            "/api/v1/lists/readonly",
            200,
            json!({ "list": {
                "id": "readonly", "name": "Recipes", "ownerId": "them",
                "privacy": "PUBLIC", "publicListType": "copy_only",
            } }),
        );
    let (app, transport) = app_and_transport(transport);
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    // A collaborative public list: a stranger is a viewer, and that viewer may write.
    let collab = call(&app, json!({ "kind": "publicList", "listId": "collab" })).await;
    assert_eq!(collab["ok"], true, "{collab}");
    assert_eq!(collab["value"]["list"]["id"], "collab");
    assert_eq!(collab["value"]["list"]["name"], "Bugs & Feedback");
    assert_eq!(collab["value"]["role"], "viewer");
    assert_eq!(collab["value"]["canAddTask"], true, "{collab}");
    assert_eq!(collab["value"]["canComment"], true, "{collab}");

    // It asked the by-id route, not the catalogue.
    let read = transport
        .requests()
        .into_iter()
        .find(|request| request.url.contains("/api/v1/lists/collab"))
        .expect("a read of the list");
    assert!(read.url.ends_with("/api/v1/lists/collab"), "{}", read.url);

    // A copy_only public list: still a viewer, but read-only — the point of the type.
    let readonly = call(&app, json!({ "kind": "publicList", "listId": "readonly" })).await;
    assert_eq!(readonly["ok"], true, "{readonly}");
    assert_eq!(readonly["value"]["role"], "viewer");
    assert_eq!(readonly["value"]["canAddTask"], false, "{readonly}");
    assert_eq!(readonly["value"]["canComment"], false, "{readonly}");

    // Neither one is in the cache: opening somebody else's list is not joining it.
    let lists = call(&app, json!({ "kind": "lists" })).await;
    let cached = lists["value"].as_array().expect("an array");
    assert!(
        !cached
            .iter()
            .any(|list| list["id"] == "collab" || list["id"] == "readonly"),
        "a public list opened by id must not be cached, got {lists}"
    );
}

/// Dragging rows into a new order writes the whole list's arrangement, locally first and then
/// to the server's route for it, whose reconciled answer wins (task 7883f710).
///
/// The shell hands over only the rows it showed. The core completes the order — the rest of the
/// arrangement keeps its place, anything never arranged comes last by creation — so the list
/// redraws in the new order offline and does not jump when the server answers.
#[tokio::test]
async fn a_manual_reorder_redraws_at_once_and_reaches_the_route_task_7883f710() {
    let transport = StubTransport::new().push_json(
        "/manual-order",
        200,
        json!({
            "list": { "id": "l1", "name": "Work", "sortBy": "manual",
                      "manualSortOrder": ["t2", "t3", "t1"] },
            "order": ["t2", "t3", "t1"],
            "meta": { "apiVersion": "v1" }
        }),
    );
    let (app, transport) = app_and_transport(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({ "id": "l1", "name": "Work", "sortBy": "manual" }))
                .expect("a list"),
        )
        .expect("stores");
    for (id, created) in [
        ("t1", "2026-01-01T00:00:00Z"),
        ("t2", "2026-01-02T00:00:00Z"),
        ("t3", "2026-01-03T00:00:00Z"),
    ] {
        app.store
            .upsert_task(
                &serde_json::from_value(json!({
                    "id": id, "title": id, "createdAt": created,
                    "lists": [{ "id": "l1", "name": "Work" }]
                }))
                .expect("a task"),
            )
            .expect("stores");
    }

    let answered = call(
        &app,
        json!({ "kind": "setManualOrder", "listId": "l1", "order": ["t3", "t1"] }),
    )
    .await;
    assert_eq!(answered["ok"], true, "{answered}");

    // On screen at once, offline: the dragged rows lead, the rest follows by creation. The
    // answer says the rows are sorted by hand, which is what lets the shell offer the drag.
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": "l1" })).await;
    assert_eq!(rows["value"]["sortBy"], "manual");
    let shown: Vec<&str> = rows["value"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(shown, vec!["t3", "t1", "t2"]);

    // Journalled, then sent whole to the list's own route.
    call(&app, json!({ "kind": "drain" })).await;
    let post = transport
        .requests()
        .into_iter()
        .find(|request| request.method.as_str() == "POST")
        .expect("a POST");
    assert!(
        post.url.ends_with("/api/v1/lists/l1/manual-order"),
        "{}",
        post.url
    );
    let body: serde_json::Value =
        serde_json::from_slice(post.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body, json!({ "order": ["t3", "t1", "t2"] }));

    // The server's reconciled order is what the cache keeps.
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": "l1" })).await;
    let shown: Vec<&str> = rows["value"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(shown, vec!["t2", "t3", "t1"]);
}

/// Settings the Apple apps write that this door used to drop without a word: a list's private
/// default, its public type, its project and its agent settings reach the cache and the journal.
#[tokio::test]
async fn every_list_setting_a_shell_writes_is_kept() {
    let app = app_with(StubTransport::new());
    let made = call(
        &app,
        json!({ "kind": "createList", "name": "Shared work", "privacy": "PUBLIC",
                "description": "For the team" }),
    )
    .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["value"]["privacy"], "PUBLIC");
    assert_eq!(made["value"]["description"], "For the team");
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let updated = call(
        &app,
        json!({ "kind": "updateList", "listId": id, "changes": {
            "defaultIsPrivate": false, "publicListType": "collaborative", "projectId": "p1",
            "aiAgentConfig": { "enabledTypes": ["claude"], "defaultAgentId": "ai-agent-claude" }
        } }),
    )
    .await;
    assert_eq!(updated["ok"], true, "{updated}");
    assert_eq!(updated["value"]["defaultIsPrivate"], false);
    assert_eq!(updated["value"]["publicListType"], "collaborative");
    assert_eq!(updated["value"]["projectId"], "p1");
    assert_eq!(
        updated["value"]["aiAgentConfig"]["defaultAgentId"],
        "ai-agent-claude"
    );

    let journal = crate::outbox::journal::all(&app.store).expect("reads");
    let edit = journal
        .iter()
        .rev()
        .find(|entry| entry.kind == crate::outbox::kind::UPDATE_LIST)
        .expect("an update is journaled");
    let body = &edit.payload["body"];
    assert_eq!(body["publicListType"], "collaborative");
    assert_eq!(body["projectId"], "p1");
    assert_eq!(body["aiAgentConfig"]["enabledTypes"], json!(["claude"]));
}

/// AATD-44 and AATD-59: a list's share link is minted as a task's is, and a share link's code
/// resolves to what it opens.
#[tokio::test]
async fn aatd59_share_links_for_lists_and_resolving_a_code() {
    let transport = StubTransport::new()
        .push_json(
            "/api/v1/shortcodes",
            200,
            json!({ "url": "https://astrid.cc/s/lst1" }),
        )
        .push_json(
            "/api/v1/shortcodes/abc123",
            200,
            json!({ "targetType": "task", "targetId": "t9", "meta": {} }),
        );
    let (app, transport) = app_and_transport(transport);
    let shared = call(&app, json!({ "kind": "shareList", "listId": "l1" })).await;
    assert_eq!(
        shared["value"]["url"], "https://astrid.cc/s/lst1",
        "{shared}"
    );
    let minted = transport
        .requests()
        .into_iter()
        .find(|request| request.url.ends_with("/api/v1/shortcodes"))
        .expect("a shortcode request");
    let body: serde_json::Value =
        serde_json::from_slice(minted.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["targetType"], "list");
    let resolved = call(
        &app,
        json!({ "kind": "resolveShortcode", "code": "abc123" }),
    )
    .await;
    assert_eq!(
        resolved["value"],
        json!({ "targetType": "task", "targetId": "t9" }),
        "{resolved}"
    );
}

/// AATD-44: the roster as iOS draws it comes from the core — the owner as a person, the
/// description, the recently-completed window, and pending invitations minus anyone already a
/// member. An agent offered to a list carries its address, face and kind.
#[tokio::test]
async fn aatd44_the_roster_and_agents_come_from_the_core() {
    let transport = StubTransport::new().push_json(
        "/available-agents",
        200,
        json!({ "agents": [
            { "id": "ai-agent-claude", "name": "Claude Agent", "email": "claude@astrid.cc",
              "image": "/ai-claude.png", "aiAgentType": "claude_agent" }
        ] }),
    );
    let app = app_with(transport);
    app.store
        .upsert_list(
            &serde_json::from_value(json!({
                "id": "l1", "name": "Family", "description": "Groceries and chores",
                "ownerId": "me", "owner": { "id": "me", "name": "Jon", "email": "jon@example.com" },
                "listMembers": [
                    { "userId": "mum", "role": "member", "user": { "id": "mum", "email": "Mum@example.com" } }
                ],
                "invitations": [
                    { "id": "i1", "listId": "l1", "email": "mum@example.com", "role": "member", "token": "a" },
                    { "id": "i2", "listId": "l1", "email": "dad@example.com", "role": "admin", "token": "b" }
                ],
                "recentlyCompletedWindow": { "kind": "duration", "amount": 3, "unit": "day" }
            }))
            .expect("a list"),
        )
        .expect("stores");
    let roster = call(&app, json!({ "kind": "listMembers", "listId": "l1" })).await;
    assert_eq!(roster["ok"], true, "{roster}");
    assert_eq!(roster["value"]["description"], "Groceries and chores");
    assert_eq!(roster["value"]["owner"]["name"], "Jon");
    let invited: Vec<&str> = roster["value"]["invitations"]
        .as_array()
        .expect("invitations")
        .iter()
        .map(|invite| invite["email"].as_str().expect("an email"))
        .collect();
    assert_eq!(
        invited,
        vec!["dad@example.com"],
        "mum already joined: {roster}"
    );
    assert!(
        !roster["value"]["recentlyCompletedWindow"].is_null(),
        "{roster}"
    );

    let options = call(&app, json!({ "kind": "listAgentOptions", "listId": "l1" })).await;
    assert_eq!(
        options["value"]["agents"][0]["email"], "claude@astrid.cc",
        "{options}"
    );
    assert_eq!(options["value"]["agents"][0]["aiAgentType"], "claude_agent");
}

/// The command reaches the route with the window and the zone it was given, and hands the
/// server's object back untouched. A shell that had to unwrap `insights` itself, or that
/// assembled the query, would be a second place the contract lived.
#[tokio::test]
async fn list_insights_asks_the_route_and_answers_what_it_said() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "/insights",
        200,
        json!({ "insights": {
            "window": { "days": 90, "timeZone": "Europe/Berlin",
                        "start": "2026-06-10", "end": "2026-09-07" },
            "series": [ { "date": "2026-09-07", "created": 1, "completed": 0 } ],
            "totals": { "created": 1, "completed": 0 },
            "createToComplete": { "medianMs": null, "sampleSize": 0 },
            "lanes": [ { "role": "doing", "name": "Doing", "medianMs": 3600000, "sampleSize": 2 } ],
            "contributors": [ { "userId": "u1", "name": "Dana", "image": null, "isAIAgent": false,
                                "completed": 3, "inspired": 1, "supported": 2,
                                "created": 1, "edited": 4, "commented": 2, "total": 6 } ]
        }}),
    ));

    let answer = call(
        &app,
        json!({ "kind": "listInsights", "listId": "l1", "days": 90, "timeZone": "Europe/Berlin" }),
    )
    .await;

    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["value"]["window"]["days"], 90);
    assert_eq!(answer["value"]["lanes"][0]["name"], "Doing");
    assert_eq!(answer["value"]["contributors"][0]["inspired"], 1);
    assert!(
        answer["value"]["createToComplete"]["medianMs"].is_null(),
        "a null median is 'nothing to take a median of', and a zero would read as instant"
    );

    let url = &transport.requests()[0].url;
    assert!(url.contains("/api/v1/lists/l1/insights"), "{url}");
    assert!(url.contains("days=90"), "{url}");
    assert!(url.contains("tz=Europe%2FBerlin"), "{url}");
}

/// Asked for nothing, the command asks for what the route would have assumed anyway — so a
/// caller that has not resolved a zone yet still gets an answer rather than a 400.
#[tokio::test]
async fn list_insights_defaults_to_thirty_days_of_utc() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "/insights",
        200,
        json!({ "insights": { "totals": { "created": 0, "completed": 0 } } }),
    ));

    call(&app, json!({ "kind": "listInsights", "listId": "l1" })).await;

    let url = &transport.requests()[0].url;
    assert!(url.contains("days=30"), "{url}");
    assert!(url.contains("tz=UTC"), "{url}");
}
