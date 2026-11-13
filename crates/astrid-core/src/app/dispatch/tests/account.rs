//! The account: signing in and out, the profile, settings, agents and the external links.

use super::*;

/// The sign-in commands, end to end through the door the shell uses.
#[tokio::test]
async fn signing_in_goes_out_through_the_browser_and_comes_back_through_a_callback() {
    let app = app_with(StubTransport::new().push_json(
        "/api/v1/auth/desktop/exchange",
        200,
        json!({
            "sessionToken": "eyJhbGciOi.token",
            "expiresAt": "2026-10-07T12:00:00Z",
            "sessionCookieName": "next-auth.session-token",
            "user": { "id": "u1", "email": "ada@example.com", "name": "Ada" }
        }),
    ));
    assert_eq!(
        call(&app, json!({ "kind": "isSignedIn" })).await["value"]["signedIn"],
        false
    );

    let began = call(&app, json!({ "kind": "beginSignIn" })).await;
    let url = began["value"]["authorizeUrl"].as_str().expect("a URL");
    let state = url::Url::parse(url)
        .expect("a URL")
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("a state");

    let completed = call(
        &app,
        json!({
            "kind": "completeSignIn",
            "callbackUrl": format!("astrid://auth/callback?code=abc&state={state}")
        }),
    )
    .await;
    assert_eq!(completed["value"]["id"], "u1");
    assert_eq!(
        call(&app, json!({ "kind": "isSignedIn" })).await["value"]["signedIn"],
        true
    );
    assert_eq!(
        call(&app, json!({ "kind": "currentUser" })).await["value"]["email"],
        "ada@example.com"
    );
}

/// Any web page can open the callback scheme. Without a flow in progress there is nothing to
/// check it against, so it is refused.
#[tokio::test]
async fn a_callback_nobody_asked_for_is_refused() {
    let app = app_with(StubTransport::new());
    let answer = call(
            &app,
            json!({ "kind": "completeSignIn", "callbackUrl": "astrid://auth/callback?code=abc&state=x" }),
        )
        .await;
    assert_eq!(answer["ok"], false);
}

/// A pass already on its way back holds the departing account's data: sign-out waits for it
/// before clearing, so nothing it writes lands in the next person's cache. And every file this
/// installation holds goes — downloads too, not only the uploads still waiting.
#[tokio::test]
async fn signing_out_waits_for_a_pass_in_flight_and_takes_every_file_with_it() {
    let app = std::sync::Arc::new(app_with(StubTransport::new()));
    std::fs::create_dir_all(app.attachment_cache()).expect("makes the cache");
    let download = app.attachment_cache().join("cm3file.png");
    std::fs::write(&download, b"a picture").expect("writes");

    let pass = app
        .store
        .session()
        .enter()
        .expect("a pass may run before sign-out");
    let signing_out = {
        let app = app.clone();
        tokio::spawn(async move { call(&app, json!({ "kind": "signOut" })).await })
    };
    tokio::task::yield_now().await;
    assert!(
        !signing_out.is_finished(),
        "sign-out waits for the pass in flight"
    );
    assert!(
        app.store.session().enter().is_none(),
        "and nothing new starts meanwhile"
    );

    drop(pass);
    signing_out.await.expect("joins");
    assert!(
        !download.exists(),
        "downloaded files belong to the departing account too"
    );
    assert!(
        app.store.session().enter().is_some(),
        "and the session opens for whoever is next"
    );
}

/// The credentials route takes PUT (it has no POST) and reads a delete's service from the body.
#[tokio::test]
async fn an_ai_key_is_saved_with_put_and_deleted_by_body() {
    let transport = StubTransport::new()
        .push_json("/ai-credentials", 200, json!({ "success": true }))
        .push_json("/ai-credentials", 200, json!({ "success": true }));
    let sent = transport.recorded.clone();
    let app = app_with(transport);
    call(
        &app,
        json!({ "kind": "saveAgentCredential", "serviceId": "openai", "key": "sk-x" }),
    )
    .await;
    call(
        &app,
        json!({ "kind": "deleteAgentCredential", "serviceId": "openai" }),
    )
    .await;
    let sent = sent.lock().expect("lock");
    let credential_requests: Vec<_> = sent
        .iter()
        .filter(|request| request.url.contains("/ai-credentials"))
        .collect();
    assert_eq!(credential_requests[0].method, crate::api::Method::Put);
    assert_eq!(credential_requests[1].method, crate::api::Method::Delete);
    let body = String::from_utf8_lossy(credential_requests[1].body.as_deref().expect("a body"))
        .to_string();
    assert!(body.contains("openai"), "{body}");
}

/// An open live stream is authenticated with the departing account's cookie, and would keep
/// delivering that account's events into the cache the next person sees. Signing out drops it; the
/// loop then waits for a new session.
#[tokio::test]
async fn signing_out_drops_the_live_stream() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "signOut" })).await;
    tokio::time::timeout(std::time::Duration::from_millis(50), app.realtime().woken())
        .await
        .expect("the stream was told to start over");
}

#[tokio::test]
async fn signing_out_abandons_a_sign_in_in_progress() {
    let app = app_with(StubTransport::new());
    let began = call(&app, json!({ "kind": "beginSignIn" })).await;
    let url = began["value"]["authorizeUrl"]
        .as_str()
        .expect("a URL")
        .to_string();
    let state = url::Url::parse(&url)
        .expect("a URL")
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("a state");

    call(&app, json!({ "kind": "signOut" })).await;
    let late = call(
        &app,
        json!({
            "kind": "completeSignIn",
            "callbackUrl": format!("astrid://auth/callback?code=abc&state={state}")
        }),
    )
    .await;
    assert_eq!(late["ok"], false);
}

/// An account that has never configured a webhook still needs the event and agent lists: the
/// screen builds its pickers from them, so this is not a 404 on the server either.
#[tokio::test]
async fn the_webhook_settings_answer_before_anything_is_configured() {
    let app = app_with(StubTransport::new().push_json(
        "webhook-settings",
        200,
        json!({
            "configured": false,
            "availableEvents": ["task.created"],
            "availableAgents": ["astrid"],
        }),
    ));

    let settings = call(&app, json!({ "kind": "webhookSettings" })).await;

    assert_eq!(settings["ok"], true, "{settings}");
    assert_eq!(settings["value"]["configured"], false);
    assert_eq!(settings["value"]["availableEvents"][0], "task.created");
}

/// Omitting `enabled` must not turn somebody's webhook off. It is the field a screen leaves
/// out when it is only changing the URL.
#[tokio::test]
async fn saving_a_webhook_without_saying_enabled_leaves_it_on() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "webhook-settings",
        200,
        json!({ "configured": true, "enabled": true }),
    ));

    call(
        &app,
        json!({
            "kind": "saveWebhook",
            "url": "https://example.com/hook",
            "events": ["task.created"],
        }),
    )
    .await;

    let sent = transport.requests();
    let body: serde_json::Value =
        serde_json::from_slice(&sent[0].body.clone().unwrap_or_default()).expect("a body");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["webhookUrl"], "https://example.com/hook");
    assert_eq!(body["regenerateSecret"], false, "not unless asked");
}

/// The credentials come back once and never again, so the answer has to reach the screen
/// rather than being reduced to "it worked".
#[tokio::test]
async fn registering_an_agent_hands_back_what_the_server_made() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "custom-agents/register",
        200,
        json!({ "agent": { "id": "a1" }, "clientSecret": "shown-once" }),
    ));

    let made = call(
        &app,
        json!({ "kind": "registerCustomAgent", "name": "builder", "listIds": ["l1"] }),
    )
    .await;

    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["value"]["clientSecret"], "shown-once");
    let sent = transport.requests();
    let body: serde_json::Value =
        serde_json::from_slice(&sent[0].body.clone().unwrap_or_default()).expect("a body");
    assert_eq!(body["agentName"], "builder");
    assert_eq!(body["listIds"][0], "l1");
}

/// Absent means every list this account has, which is a bigger grant than most people mean —
/// so it must be absent rather than an empty array, which would mean "nothing".
#[tokio::test]
async fn registering_without_naming_lists_sends_no_list_field_at_all() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "custom-agents/register",
        200,
        json!({ "agent": { "id": "a1" } }),
    ));

    call(
        &app,
        json!({ "kind": "registerCustomAgent", "name": "builder" }),
    )
    .await;

    let sent = transport.requests();
    let body: serde_json::Value =
        serde_json::from_slice(&sent[0].body.clone().unwrap_or_default()).expect("a body");
    assert!(body.get("listIds").is_none(), "{body}");
}

/// The route describes agents as mailbox rows (`{ mailbox, email, mode, locked }`), and the
/// shell was reading `id` and `name` off them: every agent drew nameless and, joined on an empty
/// id, off. The hub answers with named rows, each saying which modes it may be offered — a locked
/// harness (Muse, Codex's polling identity) only polling or off, which is all the server stores
/// for one (task 42349da6).
#[tokio::test]
async fn the_agent_hub_names_the_servers_mailbox_rows_and_locks_the_harness_ones() {
    let app = app_with(StubTransport::new().push_json(
        "agent-modes",
        200,
        json!({
            "agents": [
                { "mailbox": "claude", "email": "claude@astrid.cc", "mode": "api", "locked": false },
                { "mailbox": "muse", "email": "muse@astrid.cc", "mode": "polling", "locked": true },
            ],
            "modes": { "claude": "api", "muse": "polling" },
        }),
    ));

    let hub = call(&app, json!({ "kind": "agents" })).await;
    assert_eq!(hub["ok"], true, "{hub}");
    let agents = hub["value"]["agents"].as_array().expect("agents");
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0]["id"], "claude");
    assert_eq!(agents[0]["name"], "Claude");
    assert_eq!(agents[0]["mode"], "api");
    assert_eq!(
        agents[0]["modes"],
        json!(["api", "polling", "webhook", "off"])
    );
    assert_eq!(agents[1]["id"], "muse");
    assert_eq!(agents[1]["name"], "Muse");
    assert_eq!(agents[1]["locked"], true);
    assert_eq!(agents[1]["modes"], json!(["polling", "off"]));
}

/// A hub that refused to draw because one of its four requests failed would be a screen
/// nobody could use to fix the thing that failed.
#[tokio::test]
async fn the_agent_hub_draws_even_when_parts_of_it_are_missing() {
    let app = app_with(StubTransport::new());

    let hub = call(&app, json!({ "kind": "agents" })).await;
    assert_eq!(hub["ok"], true);
    assert!(hub["value"]["agents"]
        .as_array()
        .expect("agents")
        .is_empty());
    assert_eq!(hub["value"]["copilot"]["connected"], false);
}

/// Nothing connected is a state the panel exists to show, not an error to report.
#[tokio::test]
async fn the_external_panel_answers_even_with_nothing_connected() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let panel = call(&app, json!({ "kind": "externalSync", "listId": id })).await;
    assert_eq!(panel["ok"], true);
    let providers = panel["value"]["providers"].as_array().expect("providers");
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0]["connected"], false);
    assert!(providers[0]["containers"]
        .as_array()
        .expect("containers")
        .is_empty());
}

/// A file waiting to be uploaded belongs to whoever queued it. The journal that would have
/// sent it is wiped on sign-out, so leaving the bytes would hand the next person on this
/// machine somebody else's file with nothing left to send it.
#[tokio::test]
async fn signing_out_takes_the_files_waiting_to_be_uploaded_with_it() {
    let app = app_with(StubTransport::new());
    let made = call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    let task_id = made["value"]["id"].as_str().expect("an id").to_string();
    let scratch = std::env::temp_dir().join(format!("astrid-{}", crate::outbox::new_temp_id()));
    std::fs::write(&scratch, b"a receipt").expect("writes");
    call(
        &app,
        json!({
            "kind": "attachFile",
            "taskId": task_id,
            "path": scratch.to_string_lossy(),
        }),
    )
    .await;
    let _ = std::fs::remove_file(&scratch);
    let pending = app
        .context
        .attachments(app.attachment_cache())
        .pending_dir();
    assert!(pending.exists());

    call(&app, json!({ "kind": "signOut" })).await;

    assert!(!pending.exists());
}

/// The filters are the account's. Set here, they are what the next screen on the next machine
/// draws — and they are remembered locally either way, so being offline does not lose them.
#[tokio::test]
async fn my_tasks_filters_are_remembered_here_even_when_the_account_cannot_be_told() {
    let app = app_with(StubTransport::new());

    let set = call(
        &app,
        json!({
            "kind": "setMyTasksFilters",
            "filterPriority": [1],
            "sortBy": "when",
        }),
    )
    .await;
    assert_eq!(set["ok"], false, "the account was not reachable");

    let held = call(&app, json!({ "kind": "myTasksFilters" })).await;
    assert_eq!(held["value"]["sortBy"], "when");
    assert_eq!(held["value"]["filterPriority"][0], 1);
}

/// Filters chosen on another machine arrive with the account, not with this one.
#[tokio::test]
async fn my_tasks_filters_come_back_from_the_account() {
    let app = app_with(StubTransport::new().push_json(
        "my-tasks-preferences",
        200,
        json!({ "filterCompletion": "all", "sortBy": "priority" }),
    ));

    let fetched = call(&app, json!({ "kind": "refreshMyTasks" })).await;
    assert_eq!(fetched["ok"], true);
    assert_eq!(fetched["value"]["filterCompletion"], "all");

    let held = call(&app, json!({ "kind": "myTasksFilters" })).await;
    assert_eq!(held["value"]["filterCompletion"], "all", "and cached");
}

/// A filter that hides something has to actually hide it, which is the point of the whole
/// round trip.
#[tokio::test]
async fn a_priority_filter_narrows_what_my_tasks_shows() {
    let app = app_with(StubTransport::new());
    // My Tasks is what is assigned to the reader (D25), so somebody has to be signed in.
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");
    call(
        &app,
        json!({ "kind": "createTask", "title": "Buy milk", "priority": 1, "assigneeId": "me" }),
    )
    .await;
    call(
        &app,
        json!({ "kind": "createTask", "title": "Ring the dentist", "assigneeId": "me" }),
    )
    .await;

    call(
        &app,
        json!({ "kind": "setMyTasksFilters", "filterPriority": [1] }),
    )
    .await;
    let rows = call(
        &app,
        json!({ "kind": "rowsForList", "listId": "virtual:my-tasks" }),
    )
    .await;

    assert_eq!(rows["value"]["total"], 1);
}

/// Where a filter is written depends on what is being filtered — a list's on the list, My
/// Tasks' on the account — and that is a decision the shell must not be making.
#[tokio::test]
async fn setting_a_my_tasks_filter_writes_it_to_the_account() {
    let app = app_with(StubTransport::new());

    call(
        &app,
        json!({
            "kind": "setFilter",
            "listId": "virtual:my-tasks",
            "field": "filterCompletion",
            "value": "all",
        }),
    )
    .await;

    let held = call(&app, json!({ "kind": "myTasksFilters" })).await;
    assert_eq!(held["value"]["filterCompletion"], "all");
}

/// The mode is the account's, so the screen has to read it back rather than remember what it
/// last set — somebody who turned on "every list" at a desk sees that on their laptop.
#[tokio::test]
async fn the_google_sync_mode_is_read_from_the_account() {
    let app = app_with(StubTransport::new().push_json(
        "/api/v1/integrations",
        200,
        json!({
            "integrations": [{
                "provider": "GOOGLE_TASKS",
                "metadata": { "googleSyncMode": "all_bidirectional", "listSuffix": "(G)" },
            }],
        }),
    ));

    let answer = call(&app, json!({ "kind": "googleSyncMode" })).await;
    assert_eq!(answer["value"]["mode"], "all_bidirectional");
    assert_eq!(answer["value"]["suffix"], "(G)");
}

/// The panel reads "connected" by matching the server's own provider name, so a name we
/// invented shows a connected account as disconnected and offers a Connect button instead.
#[tokio::test]
async fn a_connected_provider_reads_as_connected_under_the_servers_name_for_it() {
    let app = app_with(
        StubTransport::new()
            .push_json(
                "/api/v1/integrations",
                200,
                json!({ "integrations": [{ "provider": "GITHUB_ISSUES" }] }),
            )
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );
    let made = call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let id = made["value"]["id"].as_str().expect("an id").to_string();

    let panel = call(&app, json!({ "kind": "externalSync", "listId": id })).await;
    let providers = panel["value"]["providers"].as_array().expect("providers");
    let github = providers
        .iter()
        .find(|provider| provider["provider"] == "git_hub")
        .expect("GitHub is one of the two");
    assert_eq!(github["connected"], true);
}

/// Task 6db679c3. Which account, not just whether: somebody with a work Google account and a
/// personal one cannot tell from a tick which of them their lists are being mirrored into.
#[tokio::test]
async fn the_account_panel_says_which_account_each_provider_is_connected_as() {
    let app = app_with(
        StubTransport::new()
            .push_json(
                "/api/v1/integrations",
                200,
                json!({
                    "integrations": [
                        { "provider": "GOOGLE_TASKS", "externalAccountId": "jon@gmail.com" },
                        { "provider": "GITHUB_ISSUES", "externalAccountId": "jonparis" },
                    ],
                }),
            )
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );

    let panel = call(&app, json!({ "kind": "externalAccounts" })).await;
    let providers = panel["value"]["providers"].as_array().expect("providers");
    let google = providers
        .iter()
        .find(|provider| provider["provider"] == "google_tasks")
        .expect("Google is one of the two");
    assert_eq!(google["connected"], true);
    assert_eq!(google["connectedAs"], "jon@gmail.com");
    let github = providers
        .iter()
        .find(|provider| provider["provider"] == "git_hub")
        .expect("GitHub is the other");
    assert_eq!(github["connectedAs"], "jonparis");
}

/// Task 6db679c3. The whole account's links in one answer, each naming both ends — the screen
/// draws "Work → Work (Google)", and neither name may be an opaque id it has to go and resolve.
#[tokio::test]
async fn the_account_panel_names_both_ends_of_every_link_it_has() {
    let app = app_with(
        StubTransport::new()
            .push_json(
                "/api/v1/integrations",
                200,
                json!({ "integrations": [{ "provider": "GOOGLE_TASKS" }] }),
            )
            .push_json(
                "/sync/google/links",
                200,
                json!({
                    "links": [{
                        "id": "link-1",
                        "astridListId": "l1",
                        "remoteContainerId": "tasklist-1",
                        "remoteContainerName": "Work (Google)",
                    }],
                }),
            )
            .fallback(Ok(crate::api::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: b"{}".to_vec(),
            })),
    );
    app.store
        .upsert_list(&crate::model::TaskList::new("l1", "Work"))
        .expect("stores");

    let panel = call(&app, json!({ "kind": "externalAccounts" })).await;
    let google = panel["value"]["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["provider"] == "google_tasks")
        .cloned()
        .expect("Google");
    let links = google["links"].as_array().expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["id"], "link-1");
    assert_eq!(links[0]["listId"], "l1");
    assert_eq!(links[0]["listName"], "Work");
    assert_eq!(links[0]["containerId"], "tasklist-1");
    assert_eq!(links[0]["containerName"], "Work (Google)");
}

/// Task 6db679c3. "New List" is one gesture, so it is one command: make the list on Google and
/// bind it, rather than leaving the client to do the second half and get it wrong.
#[tokio::test]
async fn new_list_creates_the_google_list_and_links_it() {
    let transport = StubTransport::new()
        .push_json("/sync/google/tasklists", 200, json!({ "tasklists": [] }))
        .push_json("/sync/google/links", 200, json!({ "links": [] }))
        .push_json(
            "/sync/google/tasklists",
            200,
            json!({ "tasklist": { "id": "tasklist-9", "name": "Garden" } }),
        )
        .push_json(
            "/sync/google/links",
            200,
            json!({ "link": { "id": "link-9", "astridListId": "l1", "remoteContainerId": "tasklist-9" } }),
        )
        .fallback(Ok(crate::api::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        }));
    let app = app_with(transport);
    app.store
        .upsert_list(&crate::model::TaskList::new("l1", "Garden"))
        .expect("stores");

    let made = call(
        &app,
        json!({ "kind": "createAndLinkGoogleList", "listId": "l1", "name": "Garden" }),
    )
    .await;

    assert_eq!(made["ok"], true);
    assert_eq!(made["value"]["containerId"], "tasklist-9");
    assert_eq!(made["value"]["created"], true);
    assert_eq!(made["value"]["link"]["id"], "link-9");
}

/// A pull applies what came back and commits the cursor only after it has — a client killed
/// mid-pass re-pulls rather than skipping what it never wrote down.""
#[tokio::test]
async fn a_google_pass_applies_what_it_pulled_and_then_commits() {
    let transport = StubTransport::new()
        .push_json(
            "/sync/google/links",
            200,
            json!({
                "links": [{
                    "id": "link-1",
                    "astridListId": "l1",
                    "remoteContainerId": "tasklist-1",
                }],
            }),
        )
        .push_json(
            "/sync/google/tasks?",
            200,
            json!({
                "items": [{
                    "remoteId": "tasklist-1:abc",
                    "title": "Buy oat milk",
                    "completed": false,
                    "dueDate": "2026-09-20T00:00:00Z",
                }],
                "cursor": "2026-09-07T12:00:00Z",
            }),
        )
        .push_json("/sync/google/task-links", 200, json!({ "taskLinks": [] }))
        .fallback(Ok(crate::api::transport::HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"{}".to_vec(),
        }));
    let app = app_with(transport);
    app.store
        .upsert_list(&crate::model::TaskList::new("l1", "Work"))
        .expect("stores");

    let answered = call(&app, json!({ "kind": "syncExternal" })).await;
    let passes = answered["value"]["passes"].as_array().expect("passes");
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0]["report"]["applied"], 1);

    // And the pulled task is in the list it was linked to.
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": "l1" })).await;
    assert_eq!(rows["value"]["rows"][0]["title"], "Buy oat milk");
}

/// The settings screen draws from the cache and offers the same reminder offsets the per-task
/// picker does, so "15 minutes before" means one thing in this app rather than two.
#[tokio::test]
async fn the_settings_screen_reads_from_the_cache() {
    let app = app_with(StubTransport::new());
    app.store
        .set_metadata(
            "account.settings",
            r#"{"reminderSettings":{"enablePushReminders":true,"defaultReminderTime":15}}"#,
        )
        .expect("stores");

    let answered = call(&app, json!({ "kind": "settings" })).await;
    assert_eq!(
        answered["value"]["reminderSettings"]["enablePushReminders"],
        true
    );
    let offsets = answered["value"]["offsets"].as_array().expect("offsets");
    assert_eq!(offsets[0]["titleKey"], "reminder.at_due_time");
    assert!(offsets.iter().any(|offset| offset["minutes"] == 15));
}

/// One toggle at a time: a screen sending a single field must not clear the rest.
#[tokio::test]
async fn changing_one_reminder_setting_keeps_the_others() {
    let transport = StubTransport::new().push_json("/settings", 200, json!({ "ok": true }));
    let app = app_with(transport);
    app.store
        .set_metadata(
            "account.settings",
            r#"{"reminderSettings":{"enablePushReminders":true,"enableEmailReminders":true}}"#,
        )
        .expect("stores");

    let answered = call(
        &app,
        json!({
            "kind": "updateReminderSettings",
            "changes": { "enablePushReminders": false },
        }),
    )
    .await;
    assert_eq!(
        answered["value"]["reminderSettings"]["enablePushReminders"],
        false
    );
    assert_eq!(
        answered["value"]["reminderSettings"]["enableEmailReminders"], true,
        "the setting nobody touched is still there"
    );
}

/// The account page's sections work through the core (task 19fd9289): a name or a photo is
/// saved and the account fetched back, the verification email is re-sent where the v1 route
/// reads the action, and deleting the account needs the phrase typed exactly — then leaves
/// nothing of the account on this machine.
#[tokio::test]
async fn the_account_page_edits_the_profile_resends_verification_and_deletes_the_account_task_19fd9289(
) {
    let me = json!({ "user": {
        "id": "me", "name": "Jon", "email": "jon@x.io", "image": null,
        "verified": false, "hasPendingChange": true, "pendingEmail": "new@x.io",
        "createdAt": "2026-01-02T03:04:05Z", "updatedAt": "2026-09-01T00:00:00Z"
    }});
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "v1/upload",
                200,
                json!({ "url": "https://blob.test/photo.png" }),
            )
            .push_json(
                "verify-email",
                200,
                json!({ "success": true, "message": "Verification email sent" }),
            )
            .push_json("me/delete", 200, json!({ "success": true }))
            // Everything else — the GET and the PUT of /users/me — answers with the account.
            .fallback(Ok(crate::api::transport::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: me.to_string().into_bytes(),
            })),
    );
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    // A new name: sent, then the account fetched back so the screen says what the server says.
    let renamed = call(&app, json!({ "kind": "updateProfile", "name": " Jon P " })).await;
    assert_eq!(renamed["ok"], true, "{renamed}");
    assert_eq!(renamed["value"]["user"]["verified"], false);
    assert_eq!(renamed["value"]["user"]["hasPendingChange"], true);
    assert_eq!(renamed["value"]["user"]["pendingEmail"], "new@x.io");
    let put = transport
        .requests()
        .into_iter()
        .find(|request| request.method.as_str() == "PUT")
        .expect("a PUT");
    assert!(put.url.ends_with("/api/v1/users/me"), "{}", put.url);
    let body: serde_json::Value =
        serde_json::from_slice(put.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["name"], "Jon P", "trimmed");
    assert!(
        body.get("image").is_none(),
        "a photo that was not chosen is left alone"
    );

    // A photo: uploaded as a file, then its address put on the profile.
    let photo =
        std::env::temp_dir().join(format!("astrid-photo-{}.png", crate::outbox::new_temp_id()));
    std::fs::write(&photo, b"\x89PNG").expect("writes");
    let pictured = call(
        &app,
        json!({ "kind": "updateProfile", "photoPath": photo.to_string_lossy() }),
    )
    .await;
    let _ = std::fs::remove_file(&photo);
    assert_eq!(pictured["ok"], true, "{pictured}");
    let requests = transport.requests();
    let upload = requests
        .iter()
        .find(|request| request.url.ends_with("/api/v1/upload"))
        .expect("an upload");
    let upload_body = String::from_utf8_lossy(upload.body.as_deref().expect("bytes"));
    assert!(upload_body.contains("name=\"file\""));
    assert!(upload_body.contains("image/png"));
    let put = requests
        .iter()
        .rfind(|request| request.method.as_str() == "PUT")
        .expect("a PUT");
    let body: serde_json::Value =
        serde_json::from_slice(put.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["image"], "https://blob.test/photo.png");
    assert!(body.get("name").is_none(), "the name was not touched");

    // Resend goes where the v1 route reads the action.
    let resent = call(&app, json!({ "kind": "resendVerification" })).await;
    assert_eq!(resent["ok"], true, "{resent}");
    assert_eq!(resent["value"]["message"], "Verification email sent");
    assert!(transport.requests().iter().any(|request| {
        request.url.contains("verify-email") && request.url.contains("action=resend")
    }));

    // Deleting needs the phrase, typed exactly; a near miss sends nothing.
    let refused = call(
        &app,
        json!({ "kind": "deleteAccount", "confirmation": "delete my account" }),
    )
    .await;
    assert_eq!(refused["ok"], false);
    assert!(!transport
        .requests()
        .iter()
        .any(|request| request.url.contains("me/delete")));
    let deleted = call(
        &app,
        json!({ "kind": "deleteAccount", "confirmation": "DELETE MY ACCOUNT" }),
    )
    .await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    let sent = transport
        .requests()
        .into_iter()
        .find(|request| request.url.contains("me/delete"))
        .expect("the deletion");
    let body: serde_json::Value =
        serde_json::from_slice(sent.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["confirmationText"], "DELETE MY ACCOUNT");
    assert!(
        app.context
            .account()
            .current_user()
            .expect("reads")
            .is_none(),
        "nothing of the account is left here"
    );
}

/// The Task Settings page (task c0f3db19): the defaults come shaped from the cache, the server's
/// answer replaces them on refresh, one field is written with a PATCH — refused before any
/// request when the server would refuse it — and the layout the account chose is what the rows
/// draw with, at once.
#[tokio::test]
async fn task_settings_are_read_written_and_drive_the_rows_layout_task_c0f3db19() {
    let account = json!({ "user": { "id": "me", "name": "Jon" }, "settings": {} });
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "smart-tasks",
                200,
                json!({
                    "emailToTaskEnabled": false, "defaultTaskDueOffset": "3_days",
                    "defaultDueTime": "09:00", "taskDisplayMode": "project",
                    "subtaskDisplay": "under_parent", "meta": { "apiVersion": "v1" }
                }),
            )
            .push_json("smart-tasks", 200, json!({ "taskDisplayMode": "list" }))
            // The user and the settings: enough for a refresh to be a refresh.
            .fallback(Ok(crate::api::transport::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: account.to_string().into_bytes(),
            })),
    );

    // Nothing fetched yet: the web's defaults, shaped, with the choices beside them.
    let cached = call(&app, json!({ "kind": "settings" })).await;
    assert_eq!(cached["value"]["smartTasks"]["emailToTaskEnabled"], true);
    assert_eq!(
        cached["value"]["smartTasks"]["defaultTaskDueOffset"],
        "1_week"
    );
    assert_eq!(cached["value"]["smartTasks"]["defaultDueTime"], "17:00");
    assert_eq!(cached["value"]["smartTasks"]["taskDisplayMode"], "list");
    assert_eq!(cached["value"]["dueOffsetChoices"][0]["value"], "none");
    assert_eq!(
        cached["value"]["dueTimeChoices"][2]["titleKey"],
        "smart.time.17_00"
    );
    assert_eq!(
        cached["value"]["layoutChoices"]
            .as_array()
            .expect("layouts")
            .len(),
        2
    );

    // The server's answer replaces them, without its envelope.
    let fresh = call(&app, json!({ "kind": "refreshSettings" })).await;
    assert_eq!(fresh["ok"], true, "{fresh}");
    assert_eq!(fresh["value"]["smartTasks"]["emailToTaskEnabled"], false);
    assert_eq!(
        fresh["value"]["smartTasks"]["defaultTaskDueOffset"],
        "3_days"
    );
    assert_eq!(fresh["value"]["smartTasks"]["defaultDueTime"], "09:00");
    assert_eq!(fresh["value"]["smartTasks"]["taskDisplayMode"], "project");
    assert_eq!(
        fresh["value"]["smartTasks"]["subtaskDisplay"],
        "under_parent"
    );
    assert!(fresh["value"]["smartTasks"].get("meta").is_none());

    // Project layout, chosen on the server: a list row's checkbox opens the picker.
    call(&app, json!({ "kind": "createList", "name": "Work" })).await;
    let list_id = list_id_named(&app, "Work").await;
    call(
        &app,
        json!({ "kind": "createTask", "title": "Write it down", "listIds": [list_id] }),
    )
    .await;
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": list_id })).await;
    assert_eq!(rows["value"]["rows"][0]["action"], "openPicker");

    // A value the server would refuse sends nothing.
    let refused = call(
        &app,
        json!({
            "kind": "updateSmartTaskSettings",
            "changes": { "defaultTaskDueOffset": "2_weeks" }
        }),
    )
    .await;
    assert_eq!(refused["ok"], false);
    assert!(!transport
        .requests()
        .iter()
        .any(|request| request.method.as_str() == "PATCH"));

    // Back to the list layout: one PATCH carrying just that field, and the rows follow at once.
    let written = call(
        &app,
        json!({ "kind": "updateSmartTaskSettings", "changes": { "taskDisplayMode": "list" } }),
    )
    .await;
    assert_eq!(written["ok"], true, "{written}");
    assert_eq!(written["value"]["smartTasks"]["taskDisplayMode"], "list");
    assert_eq!(
        written["value"]["smartTasks"]["defaultDueTime"], "09:00",
        "the rest is kept"
    );
    // Journalled rather than sent: it goes out with the Outbox, carrying just that field.
    call(&app, json!({ "kind": "drain" })).await;
    let patch = transport
        .requests()
        .into_iter()
        .find(|request| request.method.as_str() == "PATCH")
        .expect("a PATCH");
    assert!(
        patch.url.ends_with("/api/v1/users/me/smart-tasks"),
        "{}",
        patch.url
    );
    let body: serde_json::Value =
        serde_json::from_slice(patch.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body, json!({ "taskDisplayMode": "list" }));
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": list_id })).await;
    assert_eq!(rows["value"]["rows"][0]["action"], "complete");
}

/// Appearance's two rules (task 6ac2639a): with smart parsing on, the quick-add box's `#list`
/// files the task and leaves the title, and off it stays in the title as typed; and "inside
/// parent task only" keeps subtasks out of the top-level list while the detail keeps them.
#[tokio::test]
async fn smart_parsing_and_subtask_display_follow_the_account_task_6ac2639a() {
    let (app, _transport) = app_and_transport(
        StubTransport::new()
            .push_json("smart-tasks", 200, json!({}))
            .push_json("smart-tasks", 200, json!({})),
    );
    call(&app, json!({ "kind": "createList", "name": "Health" })).await;
    call(
        &app,
        json!({ "kind": "createList", "name": "Side Projects" }),
    )
    .await;
    let health = list_id_named(&app, "Health").await;
    let side = list_id_named(&app, "Side Projects").await;

    // On by default, as on web: the tag files the task and leaves the title. The tagged list
    // comes first, the open list after.
    let filed = call(
        &app,
        json!({
            "kind": "createTask", "title": "Pushups #health", "listIds": [side], "quickAdd": true
        }),
    )
    .await;
    assert_eq!(filed["ok"], true, "{filed}");
    assert_eq!(filed["value"]["title"], "Pushups");
    assert_eq!(filed["value"]["listIds"], json!([health, side]));

    // A two-word list, typed the way people type it after a `#`.
    let dashed = call(
        &app,
        json!({ "kind": "createTask", "title": "Read #side-projects docs", "quickAdd": true }),
    )
    .await;
    assert_eq!(dashed["value"]["title"], "Read docs");
    assert_eq!(dashed["value"]["listIds"], json!([side]));

    // Not from the quick-add box: a title is a title.
    let typed = call(
        &app,
        json!({ "kind": "createTask", "title": "Fix #health", "listIds": [side] }),
    )
    .await;
    assert_eq!(typed["value"]["title"], "Fix #health");
    assert_eq!(typed["value"]["listIds"], json!([side]));

    // Off: the tag stays in the title and the task lands in the open list only.
    let off = call(
        &app,
        json!({
            "kind": "updateSmartTaskSettings",
            "changes": { "smartTaskCreationEnabled": false }
        }),
    )
    .await;
    assert_eq!(off["ok"], true, "{off}");
    let plain = call(
        &app,
        json!({
            "kind": "createTask", "title": "Walk #health", "listIds": [side], "quickAdd": true
        }),
    )
    .await;
    assert_eq!(plain["value"]["title"], "Walk #health");
    assert_eq!(plain["value"]["listIds"], json!([side]));

    // Subtasks: indented in the list by default...
    let parent = call(
        &app,
        json!({ "kind": "createTask", "title": "Plan the trip", "listIds": [health] }),
    )
    .await;
    let parent_id = parent["value"]["id"].as_str().expect("an id").to_string();
    call(
        &app,
        json!({
            "kind": "createTask", "title": "Book flights",
            "listIds": [health], "parentTaskId": parent_id
        }),
    )
    .await;
    let titles = |rows: &serde_json::Value| -> Vec<String> {
        rows["value"]["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["title"].as_str().expect("a title").to_string())
            .collect()
    };
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": health })).await;
    assert!(
        titles(&rows).contains(&"Book flights".to_string()),
        "{:?}",
        titles(&rows)
    );

    // ...and inside the parent only when the account says so; the detail still has it.
    let under = call(
            &app,
            json!({ "kind": "updateSmartTaskSettings", "changes": { "subtaskDisplay": "under_parent" } }),
        )
        .await;
    assert_eq!(under["ok"], true, "{under}");
    let rows = call(&app, json!({ "kind": "rowsForList", "listId": health })).await;
    assert!(
        !titles(&rows).contains(&"Book flights".to_string()),
        "{:?}",
        titles(&rows)
    );
    assert!(titles(&rows).contains(&"Plan the trip".to_string()));
    let detail = call(&app, json!({ "kind": "taskDetail", "taskId": parent_id })).await;
    assert_eq!(detail["value"]["subtasks"][0]["title"], "Book flights");
}

/// Passkeys on the Account page (task 19fd9289): listed as the page draws them, renamed with a
/// PATCH to the one path, revoked with a DELETE to it, and a server without the route reads as
/// a sentence rather than a missing account.
#[tokio::test]
async fn passkeys_are_listed_renamed_and_revoked_through_the_core_task_19fd9289() {
    let (app, transport) = app_and_transport(
            StubTransport::new()
                .push_json(
                    "me/passkeys",
                    200,
                    json!({ "passkeys": [
                        { "id": "k1", "name": "MacBook", "credentialDeviceType": "multiDevice", "credentialBackedUp": true, "createdAt": "2026-08-01T00:00:00Z" },
                        { "id": "k2", "name": "YubiKey", "credentialDeviceType": "singleDevice", "credentialBackedUp": false, "createdAt": "2026-07-01T00:00:00Z" }
                    ], "meta": { "apiVersion": "v1" } }),
                )
                .push_json("me/passkeys", 200, json!({ "success": true }))
                .push_json("me/passkeys", 200, json!({ "success": true }))
                .push_json("me/passkeys", 404, json!({ "error": "Not found" })),
        );

    let listed = call(&app, json!({ "kind": "passkeys" })).await;
    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["value"]["passkeys"][0]["name"], "MacBook");
    assert_eq!(listed["value"]["passkeys"][0]["isSynced"], true);
    assert_eq!(listed["value"]["passkeys"][1]["isSynced"], false);
    assert!(listed["value"]["passkeys"][0]
        .get("credentialDeviceType")
        .is_none());

    let blank = call(
        &app,
        json!({ "kind": "renamePasskey", "id": "k1", "name": "  " }),
    )
    .await;
    assert_eq!(blank["ok"], false, "refused before any request");

    let renamed = call(
        &app,
        json!({ "kind": "renamePasskey", "id": "k1", "name": " Laptop " }),
    )
    .await;
    assert_eq!(renamed["ok"], true, "{renamed}");
    let revoked = call(&app, json!({ "kind": "revokePasskey", "id": "k2" })).await;
    assert_eq!(revoked["ok"], true, "{revoked}");

    let sent: Vec<(String, String)> = transport
        .requests()
        .iter()
        .filter(|request| request.url.contains("me/passkeys"))
        .map(|request| (request.method.as_str().to_string(), request.url.clone()))
        .collect();
    assert_eq!(sent.len(), 3, "list, rename, revoke");
    assert_eq!(sent[1].0, "PATCH");
    assert!(sent[1].1.ends_with("/api/v1/users/me/passkeys/k1"));
    let body: serde_json::Value = serde_json::from_slice(
        transport.requests()[transport.requests().len() - 2]
            .body
            .as_deref()
            .expect("a body"),
    )
    .expect("json");
    assert_eq!(body["name"], "Laptop", "trimmed");
    assert_eq!(sent[2].0, "DELETE");
    assert!(sent[2].1.ends_with("/api/v1/users/me/passkeys/k2"));

    // A deployment without the route: one sentence, not a missing account.
    let missing = call(&app, json!({ "kind": "passkeys" })).await;
    assert_eq!(missing["ok"], false);
    assert!(missing["error"]["message"]
        .as_str()
        .expect("a message")
        .contains("does not offer passkeys"));
}

/// Task 4c285039: a passkey is registered through the core, so Android's Credential Manager and
/// Windows Hello each only have to run the ceremony. Two commands, one server-side ceremony — the
/// `sessionId` the challenge arrived with has to go back out with the attestation.
#[tokio::test]
async fn a_passkey_is_registered_through_the_core_task_4c285039() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "webauthn/register/options",
                200,
                json!({
                    "options": { "challenge": "Y2hhbGxlbmdl", "rp": { "id": "astrid.cc" } },
                    "sessionId": "s1",
                }),
            )
            .push_json(
                "webauthn/register/verify",
                200,
                json!({ "verified": true, "isNewUser": false }),
            ),
    );

    let begun = call(&app, json!({ "kind": "passkeyRegistrationOptions" })).await;
    assert_eq!(begun["ok"], true, "{begun}");
    assert_eq!(begun["value"]["sessionId"], "s1");
    assert_eq!(begun["value"]["options"]["challenge"], "Y2hhbGxlbmdl");

    let no_session = call(
        &app,
        json!({ "kind": "registerPasskey", "sessionId": " ", "attestation": { "id": "new" } }),
    )
    .await;
    assert_eq!(
        no_session["ok"], false,
        "refused before any request: the server would only fail it later"
    );

    let registered = call(
        &app,
        json!({
            "kind": "registerPasskey",
            "sessionId": "s1",
            "attestation": { "id": "new", "rawId": "new", "type": "public-key" },
            "name": "Surface",
        }),
    )
    .await;
    assert_eq!(registered["ok"], true, "{registered}");

    let sent: Vec<(String, String)> = transport
        .requests()
        .iter()
        .filter(|request| request.url.contains("webauthn"))
        .map(|request| (request.method.as_str().to_string(), request.url.clone()))
        .collect();
    assert_eq!(sent.len(), 2, "one challenge, one verification");
    assert_eq!(sent[0].0, "POST");
    assert!(sent[0].1.ends_with("/api/auth/webauthn/register/options"));
    assert!(sent[1].1.ends_with("/api/auth/webauthn/register/verify"));

    let body: serde_json::Value = serde_json::from_slice(
        transport.requests()[transport.requests().len() - 1]
            .body
            .as_deref()
            .expect("a body"),
    )
    .expect("json");
    assert_eq!(body["sessionId"], "s1", "the ceremony is one exchange");
    assert_eq!(body["response"]["rawId"], "new", "passed through unread");
    assert_eq!(body["name"], "Surface");
}

/// Task 4c285039: the address changes when the mail is answered, so the core reports that it asked
/// rather than claiming it changed.
#[tokio::test]
async fn changing_the_email_reports_the_verification_it_asked_for_task_4c285039() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "users/me",
                200,
                json!({
                    "success": true,
                    "message": "Account updated successfully. Please check your email to verify the new address.",
                    "emailVerificationRequired": true,
                }),
            )
            .push_json(
                "users/me",
                409,
                json!({ "error": "Email address is already in use" }),
            ),
    );

    let blank = call(&app, json!({ "kind": "changeEmail", "email": "  " })).await;
    assert_eq!(blank["ok"], false, "refused before any request");

    let asked = call(
        &app,
        json!({ "kind": "changeEmail", "email": "grace@example.com" }),
    )
    .await;
    assert_eq!(asked["ok"], true, "{asked}");
    assert_eq!(asked["value"]["verificationRequired"], true);

    let taken = call(
        &app,
        json!({ "kind": "changeEmail", "email": "taken@example.com" }),
    )
    .await;
    assert_eq!(taken["ok"], false);
    assert!(
        taken["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("already in use"),
        "a 409 belongs on screen as a sentence: {taken}"
    );

    let sent: Vec<String> = transport
        .requests()
        .iter()
        .filter(|request| request.url.ends_with("/api/v1/users/me"))
        .map(|request| request.method.as_str().to_string())
        .collect();
    assert_eq!(sent, vec!["PUT", "PUT"], "the blank one never went");
}

/// The Contacts page (task 438494c7): the server's list, shaped to what the page draws, and
/// clearing it answers with the count.
#[tokio::test]
async fn contacts_are_listed_and_cleared_through_the_core_task_438494c7() {
    let (app, transport) = app_and_transport(
            StubTransport::new()
                .push_json(
                    "v1/contacts",
                    200,
                    json!({
                        "contacts": [
                            { "id": "c1", "email": "ann@x.io", "name": "Ann", "phoneNumber": null, "uploadedAt": "2026-09-01T00:00:00Z" },
                            { "id": "c2", "email": "bo@x.io", "name": null, "phoneNumber": null, "uploadedAt": "2026-09-01T00:00:00Z" }
                        ],
                        "pagination": { "total": 2, "limit": 500, "offset": 0, "hasMore": false },
                        "meta": { "apiVersion": "v1" }
                    }),
                )
                .push_json("v1/contacts", 200, json!({ "message": "All contacts deleted", "deleted": 2 })),
        );

    let listed = call(&app, json!({ "kind": "contacts" })).await;
    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["value"]["total"], 2);
    assert_eq!(listed["value"]["contacts"][0]["email"], "ann@x.io");
    assert_eq!(listed["value"]["contacts"][0]["name"], "Ann");
    assert!(listed["value"]["contacts"][1]["name"].is_null());
    assert!(
        listed["value"]["contacts"][0].get("phoneNumber").is_none(),
        "only what the page draws"
    );

    let cleared = call(&app, json!({ "kind": "clearContacts" })).await;
    assert_eq!(cleared["ok"], true, "{cleared}");
    assert_eq!(cleared["value"]["deleted"], 2);
    let methods: Vec<&str> = transport
        .requests()
        .iter()
        .filter(|request| {
            request.url.ends_with("/api/v1/contacts") || request.url.contains("/api/v1/contacts?")
        })
        .map(|request| request.method.as_str())
        .collect();
    assert_eq!(methods, ["GET", "DELETE"]);
}

/// AATD-68 (astrid-android). Uploading the device's contacts is iOS's ContactsService.syncContacts:
/// batches of 500, the first replacing what the server had and the rest appending to it, and the
/// answer is the last batch's stats (its `total` is the account's whole count).
#[tokio::test]
async fn aatd68_contacts_upload_in_batches_the_first_replacing_the_rest_appending() {
    let stats = |total: u64| {
        json!({ "message": "Contacts synced successfully",
        "stats": { "received": 0, "valid": 0, "created": 0, "updated": 0, "total": total, "errors": 0 } })
    };
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json("v1/contacts", 200, stats(500))
            .push_json("v1/contacts", 200, stats(1001))
            .push_json("v1/contacts", 200, stats(1201)),
    );
    let contacts: Vec<serde_json::Value> = (0..1201)
        .map(|i| json!({ "email": format!("p{i}@x.io"), "name": format!("P {i}"), "phoneNumber": null }))
        .collect();

    let uploaded = call(
        &app,
        json!({ "kind": "uploadContacts", "contacts": contacts }),
    )
    .await;

    assert_eq!(uploaded["ok"], true, "{uploaded}");
    assert_eq!(uploaded["value"]["total"], 1201);
    let sent: Vec<serde_json::Value> = transport
        .requests()
        .iter()
        .filter(|request| request.url.ends_with("/api/v1/contacts"))
        .map(|request| {
            assert_eq!(request.method.as_str(), "POST");
            serde_json::from_slice(&request.body.clone().unwrap_or_default()).expect("a body")
        })
        .collect();
    let sizes: Vec<usize> = sent
        .iter()
        .map(|body| body["contacts"].as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(sizes, [500, 500, 201]);
    let replace: Vec<bool> = sent.iter().map(|body| body["replaceAll"] == true).collect();
    assert_eq!(replace, [true, false, false]);
}

/// AATD-68. A device with no contacts that have an email still says so: one empty upload that
/// replaces, so a person who deleted everyone is not still suggested.
#[tokio::test]
async fn aatd68_no_contacts_still_clears_what_the_server_had() {
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "v1/contacts",
        200,
        json!({ "stats": { "total": 0 } }),
    ));

    let uploaded = call(&app, json!({ "kind": "uploadContacts", "contacts": [] })).await;

    assert_eq!(uploaded["ok"], true, "{uploaded}");
    assert_eq!(uploaded["value"]["total"], 0);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body.clone().unwrap_or_default()).expect("a body");
    assert_eq!(body["replaceAll"], true);
    assert_eq!(body["contacts"], json!([]));
}

#[tokio::test]
async fn signing_out_empties_everything() {
    let app = app_with(StubTransport::new());
    call(&app, json!({ "kind": "createTask", "title": "Buy milk" })).await;
    call(&app, json!({ "kind": "signOut" })).await;
    let stats = call(&app, json!({ "kind": "outboxStats" })).await;
    assert_eq!(stats["value"]["hasUnsentWork"], false);
}

/// The model behind @astrid (task 810e1876): the options are the agents the server can run,
/// without Astrid itself, the choice is the account's default agent, and choosing patches it —
/// or clears it, which leaves the server to pick.
#[tokio::test]
async fn the_model_behind_astrid_is_listed_chosen_and_cleared_task_810e1876() {
    let agents = json!({ "agents": [
        { "id": "astrid-user", "name": "Astrid", "email": "astrid@astrid.cc", "service": "claude" },
        { "id": "claude-agent", "name": "Claude", "email": "claude@astrid.cc", "service": "claude" },
        { "id": "own", "name": "Mine", "email": "mine@custom.test", "service": "openclaw" },
    ] });
    let transport = StubTransport::new()
        .push_json("/available-agents", 200, agents.clone())
        .push_json(
            "/ai-preferences",
            200,
            json!({ "defaultAgentId": "claude-agent" }),
        )
        // Choosing: the PATCH's own answer, then the selector read back.
        .push_json("/ai-preferences", 200, json!({ "defaultAgentId": "own" }))
        .push_json("/available-agents", 200, agents.clone())
        .push_json("/ai-preferences", 200, json!({ "defaultAgentId": "own" }))
        // Clearing.
        .push_json("/ai-preferences", 200, json!({ "defaultAgentId": null }))
        .push_json("/available-agents", 200, agents)
        .push_json("/ai-preferences", 200, json!({ "defaultAgentId": null }));
    let (app, transport) = app_and_transport(transport);

    let read = call(&app, json!({ "kind": "astridModel" })).await;
    assert_eq!(read["ok"], true, "{read}");
    let options = read["value"]["options"].as_array().expect("options");
    assert_eq!(options.len(), 2, "Astrid itself is not an option: {read}");
    assert_eq!(options[0]["id"], "claude-agent");
    assert_eq!(options[0]["isSelected"], true);
    assert_eq!(options[1]["serviceLabel"], "Custom Agent");
    assert_eq!(read["value"]["selected"], "claude-agent");
    assert!(transport
        .requests()
        .iter()
        .any(|request| request.url.contains("available-agents")
            && request.url.contains("serverRun=true")));

    let chosen = call(&app, json!({ "kind": "setAstridModel", "agentId": "own" })).await;
    assert_eq!(chosen["ok"], true, "{chosen}");
    assert_eq!(chosen["value"]["selected"], "own");
    let patch = transport
        .requests()
        .into_iter()
        .find(|request| request.method.as_str() == "PATCH")
        .expect("a PATCH");
    assert!(
        patch.url.ends_with("/api/v1/users/me/ai-preferences"),
        "{}",
        patch.url
    );
    let body: serde_json::Value =
        serde_json::from_slice(patch.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["defaultAgentId"], "own");

    let cleared = call(&app, json!({ "kind": "setAstridModel" })).await;
    assert_eq!(cleared["ok"], true, "{cleared}");
    assert_eq!(cleared["value"]["selected"], serde_json::Value::Null);
    let last = transport
        .requests()
        .into_iter()
        .rfind(|request| request.method.as_str() == "PATCH")
        .expect("a second PATCH");
    let body: serde_json::Value =
        serde_json::from_slice(last.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["defaultAgentId"], serde_json::Value::Null);
}

/// The calendar feed (task 28c5c6a9): the account answers the address a calendar app subscribes
/// to, and the two calendar fields ride the reminder-settings blob like every other field.
#[tokio::test]
async fn the_calendar_feed_is_addressed_and_its_settings_ride_the_reminder_blob_task_28c5c6a9() {
    let app = app_with(StubTransport::new());

    let before = call(&app, json!({ "kind": "settings" })).await;
    assert_eq!(before["ok"], true, "{before}");
    assert_eq!(
        before["value"]["calendarFeedUrl"],
        "https://astrid.cc/api/calendar/tasks.ics"
    );

    let changed = call(
        &app,
        json!({
            "kind": "updateReminderSettings",
            "changes": { "enableCalendarSync": true, "calendarSyncType": "with_due_times" }
        }),
    )
    .await;
    assert_eq!(changed["ok"], true, "{changed}");
    assert_eq!(
        changed["value"]["reminderSettings"]["enableCalendarSync"],
        true
    );
    assert_eq!(
        changed["value"]["reminderSettings"]["calendarSyncType"],
        "with_due_times"
    );

    let narrowed = call(
        &app,
        json!({ "kind": "updateReminderSettings", "changes": { "calendarSyncType": "none" } }),
    )
    .await;
    assert_eq!(
        narrowed["value"]["reminderSettings"]["calendarSyncType"],
        "none"
    );
    assert_eq!(
        narrowed["value"]["reminderSettings"]["enableCalendarSync"], true,
        "one field at a time: the other stays"
    );
}

/// AITD-463: an Apple client hands its old UserDefaults sync ledger to the core in one command,
/// and running it twice is harmless.
#[tokio::test]
async fn aitd_463_an_apple_ledger_imports_in_one_command() {
    let app = app_with(StubTransport::new());
    let command = json!({
        "kind": "importExternalLedger",
        "provider": "google_tasks",
        "pending": [{ "remoteId": "p1", "containerId": "c1" }],
        "tombstones": ["t1"],
        "serverTombstones": ["s1"],
        "excluded": ["x1"],
        "links": [{ "taskId": "task-1", "remoteId": "r1", "containerId": "c1" }],
    });
    for _ in 0..2 {
        let answered = call(&app, command.clone()).await;
        assert_eq!(answered["ok"], true, "{answered}");
    }

    let store = &app.store;
    use crate::external::ledger;
    assert_eq!(
        ledger::pending(store, "google"),
        vec![("p1".to_string(), "c1".to_string())]
    );
    assert_eq!(
        ledger::tombstoned(store, "google"),
        vec!["t1".to_string(), "p1".to_string(), "s1".to_string()]
    );
    assert_eq!(ledger::excluded(store, "google"), vec!["x1".to_string()]);
    assert_eq!(
        ledger::twin(store, "google", "task-1"),
        Some(("r1".to_string(), "c1".to_string()))
    );

    // Only the provider is required: every part may be absent.
    let bare = call(
        &app,
        json!({ "kind": "importExternalLedger", "provider": "google_tasks" }),
    )
    .await;
    assert_eq!(bare["ok"], true, "{bare}");
}

// ─── AWTD-1610: the commands iOS needs to move off AstridAPIClient ──────────────────────────────

fn json_response(status: u16, body: serde_json::Value) -> crate::api::transport::HttpResponse {
    crate::api::transport::HttpResponse {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.to_string().into_bytes(),
    }
}

/// The key rows show why a test failed. The route answers 200 with `{ success: false, error }`
/// for a key the provider refused, and `testAgentCredential` used to keep only `works`.
#[tokio::test]
async fn testing_a_key_keeps_the_servers_reason_awtd_1610() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "ai-credentials/test",
                200,
                json!({ "success": false, "error": "HTTP 401: invalid x-api-key",
                        "meta": { "apiVersion": "v1" } }),
            )
            .push_json("ai-credentials/test", 200, json!({ "success": true })),
    );

    let refused = call(
        &app,
        json!({ "kind": "testAgentCredential", "serviceId": "claude" }),
    )
    .await;
    assert_eq!(refused["ok"], true, "{refused}");
    assert_eq!(refused["value"]["works"], false);
    assert_eq!(refused["value"]["error"], "HTTP 401: invalid x-api-key");

    let works = call(
        &app,
        json!({ "kind": "testAgentCredential", "serviceId": "claude" }),
    )
    .await;
    assert_eq!(works["value"]["works"], true);
    assert!(works["value"]["error"].is_null(), "{works}");

    let body: serde_json::Value =
        serde_json::from_slice(transport.requests()[0].body.as_deref().expect("a body"))
            .expect("json");
    assert_eq!(body["serviceId"], "claude");
}

/// The AI-keys screen reads the stored keys on its own, not as one part of the whole hub.
#[tokio::test]
async fn the_stored_keys_are_read_on_their_own_awtd_1610() {
    let keys = json!({
        "claude": { "hasKey": true, "keyPreview": "sk-ant...abcd", "isValid": false,
                    "lastTested": "2026-11-01T00:00:00Z", "error": "HTTP 401: nope" },
    });
    let (app, transport) = app_and_transport(StubTransport::new().push_json(
        "/users/me/ai-credentials",
        200,
        json!({ "keys": keys.clone(), "meta": { "apiVersion": "v1" } }),
    ));

    let read = call(&app, json!({ "kind": "agentCredentials" })).await;
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["value"], json!({ "keys": keys }));
    let requests = transport.requests();
    assert_eq!(requests.len(), 1, "one read, not the whole hub");
    assert_eq!(requests[0].method.as_str(), "GET");

    // Unlike the hub, which draws around a missing part, a read on its own says it failed: the
    // app keeps what it drew rather than drawing "no keys".
    let offline = app_with(StubTransport::new());
    let failed = call(&offline, json!({ "kind": "agentCredentials" })).await;
    assert_eq!(failed["ok"], false, "{failed}");
}

/// The modes and the Copilot status, each on its own, as the server answers them.
#[tokio::test]
async fn the_agent_modes_and_copilot_status_are_read_on_their_own_awtd_1610() {
    let agents = json!([
        { "mailbox": "claude", "email": "claude@astrid.cc", "mode": "api", "locked": false },
        { "mailbox": "muse", "email": "muse@astrid.cc", "mode": "polling", "locked": true },
    ]);
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "/users/me/agent-modes",
                200,
                json!({ "agents": agents.clone(), "modes": { "claude": "api", "muse": "polling" },
                        "meta": { "apiVersion": "v1" } }),
            )
            .push_json(
                "/integrations/copilot/status",
                200,
                json!({ "connected": true }),
            ),
    );

    let modes = call(&app, json!({ "kind": "agentModes" })).await;
    assert_eq!(modes["ok"], true, "{modes}");
    assert_eq!(modes["value"]["agents"], agents);
    assert_eq!(modes["value"]["modes"]["muse"], "polling");
    assert!(modes["value"].get("meta").is_none());

    let copilot = call(&app, json!({ "kind": "copilotStatus" })).await;
    assert_eq!(copilot["ok"], true, "{copilot}");
    assert_eq!(copilot["value"], json!({ "connected": true }));

    assert_eq!(transport.requests().len(), 2);
}

/// The assignee and mention list is `/users/me/available-agents`, whole for a picker and
/// narrowed with `serverRun=true` for what can power the default assistant (AITD-297).
#[tokio::test]
async fn the_available_agents_are_listed_whole_or_server_run_only_awtd_1610() {
    let agents = json!([
        { "id": "claude-agent", "name": "Claude", "email": "claude@astrid.cc",
          "image": null, "service": "claude" },
    ]);
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "/available-agents",
                200,
                json!({ "agents": agents.clone() }),
            )
            .push_json(
                "/available-agents",
                200,
                json!({ "agents": agents.clone() }),
            ),
    );

    let whole = call(&app, json!({ "kind": "availableAgents" })).await;
    assert_eq!(whole["ok"], true, "{whole}");
    assert_eq!(whole["value"], json!({ "agents": agents }));

    let narrowed = call(
        &app,
        json!({ "kind": "availableAgents", "serverRunOnly": true }),
    )
    .await;
    assert_eq!(narrowed["ok"], true, "{narrowed}");

    let requests = transport.requests();
    assert!(
        !requests[0].url.contains("serverRun"),
        "the picker's list is not narrowed: {}",
        requests[0].url
    );
    assert!(
        requests[1].url.contains("serverRun=true"),
        "{}",
        requests[1].url
    );
}

/// `FeatureFlagService` keeps every flag — rollouts included — and the version it compares a
/// `featureFlagsUpdated` frame against. `features` names five; `featureFlags` answers them all.
#[tokio::test]
async fn the_whole_flag_map_comes_back_with_its_version_awtd_1610() {
    let whole = json!({ "version": 7, "features": {
        "project_mode": true, "google_tasks": false, "a_rollout_nobody_named": true,
    } });
    let mut tagged = json_response(200, whole.clone());
    tagged
        .headers
        .push(("etag".into(), "\"features-7-101\"".into()));
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push("/api/v1/features", Ok(tagged))
            // Unchanged since: answered from what was kept, version and all.
            .push(
                "/api/v1/features",
                Ok(crate::api::transport::HttpResponse {
                    status: 304,
                    headers: vec![],
                    body: vec![],
                }),
            ),
    );

    let fresh = call(&app, json!({ "kind": "featureFlags" })).await;
    assert_eq!(fresh["ok"], true, "{fresh}");
    assert_eq!(fresh["value"], whole);

    // The five-flag command reads the same fetch, unchanged.
    let named = call(&app, json!({ "kind": "features" })).await;
    assert_eq!(named["value"]["projectMode"], true);
    assert_eq!(named["value"]["googleTasks"], false);

    let unchanged = call(&app, json!({ "kind": "featureFlags" })).await;
    assert_eq!(unchanged["ok"], true, "{unchanged}");
    assert_eq!(unchanged["value"], whole);
    let second = &transport.requests()[1];
    assert_eq!(
        second
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("if-none-match"))
            .map(|(_, value)| value.as_str()),
        Some("\"features-7-101\""),
        "asked conditionally, like the tick"
    );
}

/// A tag stored by a build that kept only the bare map: a 304 against it has no version to
/// answer with, so the command asks once more, unconditionally.
#[tokio::test]
async fn a_304_with_no_whole_map_kept_asks_again_awtd_1610() {
    let whole = json!({ "version": 3, "features": { "project_mode": true } });
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push(
                "/api/v1/features",
                Ok(crate::api::transport::HttpResponse {
                    status: 304,
                    headers: vec![],
                    body: vec![],
                }),
            )
            .push_json("/api/v1/features", 200, whole.clone()),
    );
    app.store
        .set_metadata("etag:features", "\"features-3-1\"")
        .expect("stores");
    app.store
        .set_metadata("account.features", r#"{"project_mode":true}"#)
        .expect("stores");

    let flags = call(&app, json!({ "kind": "featureFlags" })).await;
    assert_eq!(flags["ok"], true, "{flags}");
    assert_eq!(flags["value"], whole);
    assert_eq!(transport.requests().len(), 2);
}

/// Offline, the command fails rather than answering an old copy as if it were fresh: iOS keeps
/// its own copy and stays on it.
#[tokio::test]
async fn the_flag_map_fails_offline_awtd_1610() {
    let app = app_with(StubTransport::new());
    let flags = call(&app, json!({ "kind": "featureFlags" })).await;
    assert_eq!(flags["ok"], false, "{flags}");
}

/// The iOS screens upload a photo the moment it is picked and keep only its address, so
/// `updateProfile` takes that address as it is — no second upload.
#[tokio::test]
async fn a_profile_photo_already_uploaded_is_put_on_the_account_by_address_awtd_1610() {
    let me = json!({ "user": { "id": "me", "name": "Jon", "image": "https://blob.test/new.png" } });
    let (app, transport) =
        app_and_transport(StubTransport::new().fallback(Ok(json_response(200, me))));
    app.store
        .set_metadata("account.current-user", r#"{"id":"me","name":"Jon"}"#)
        .expect("stores");

    let pictured = call(
        &app,
        json!({ "kind": "updateProfile", "image": "https://blob.test/new.png" }),
    )
    .await;
    assert_eq!(pictured["ok"], true, "{pictured}");
    let requests = transport.requests();
    assert!(
        !requests
            .iter()
            .any(|request| request.url.contains("/upload")),
        "nothing uploaded twice"
    );
    let put = requests
        .iter()
        .find(|request| request.method.as_str() == "PUT")
        .expect("a PUT");
    assert!(put.url.ends_with("/api/v1/users/me"), "{}", put.url);
    let body: serde_json::Value =
        serde_json::from_slice(put.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body, json!({ "image": "https://blob.test/new.png" }));

    // A file AND an address is two answers to one question.
    let sent = transport.requests().len();
    let both = call(
        &app,
        json!({ "kind": "updateProfile", "photoPath": "/tmp/x.png",
                "image": "https://blob.test/new.png" }),
    )
    .await;
    assert_eq!(both["ok"], false, "{both}");
    assert_eq!(
        transport.requests().len(),
        sent,
        "refused before any request"
    );
}

/// Send and cancel go where resend goes, with their own action.
#[tokio::test]
async fn verification_can_be_sent_and_cancelled_awtd_1610() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "action=send",
                200,
                json!({ "success": true, "message": "Verification email sent" }),
            )
            .push_json(
                "action=cancel",
                200,
                json!({ "success": true, "message": "Email change cancelled" }),
            )
            .push_json(
                "action=cancel",
                400,
                json!({ "success": false, "message": "No pending email change" }),
            ),
    );

    let sent = call(&app, json!({ "kind": "sendVerification" })).await;
    assert_eq!(sent["ok"], true, "{sent}");
    assert_eq!(
        sent["value"],
        json!({ "message": "Verification email sent" })
    );

    let cancelled = call(&app, json!({ "kind": "cancelVerification" })).await;
    assert_eq!(cancelled["ok"], true, "{cancelled}");
    assert_eq!(cancelled["value"]["message"], "Email change cancelled");

    let declined = call(&app, json!({ "kind": "cancelVerification" })).await;
    assert_eq!(declined["ok"], false, "a 400 is a refusal: {declined}");

    let requests = transport.requests();
    assert!(requests
        .iter()
        .all(|request| request.method.as_str() == "POST"
            && request.url.contains("/api/v1/users/me/verify-email")));
    let body: serde_json::Value =
        serde_json::from_slice(requests[0].body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["action"], "send");
}

/// Member autocomplete and recommended collaborators, as the routes answer them.
#[tokio::test]
async fn contacts_are_searched_and_collaborators_recommended_awtd_1610() {
    let hit = json!({ "id": "c1", "email": "ann@x.io", "name": "Ann", "phoneNumber": null,
                      "isAstridUser": true, "astridUserName": "Ann B",
                      "astridUserImage": "https://img.test/ann.png" });
    let recommended = json!([{ "id": "u1", "email": "ann@x.io", "name": null,
                               "image": null, "contactName": "Ann", "isMutual": true }]);
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "/contacts/search",
                200,
                json!({ "results": [hit.clone()], "meta": { "apiVersion": "v1" } }),
            )
            .push_json(
                "/contacts/recommended",
                200,
                json!({ "recommended": recommended.clone(),
                        "stats": { "mutual": 1, "nonMutual": 0, "total": 1 },
                        "meta": { "apiVersion": "v1" } }),
            )
            .push_json(
                "/contacts/recommended",
                200,
                json!({ "recommended": [],
                        "message": "No contacts uploaded yet. Sync your address book to get recommendations.",
                        "meta": { "apiVersion": "v1" } }),
            ),
    );

    let found = call(
        &app,
        json!({ "kind": "searchContacts", "query": " an ", "excludeListId": "l1" }),
    )
    .await;
    assert_eq!(found["ok"], true, "{found}");
    assert_eq!(found["value"], json!({ "results": [hit] }));

    let suggested = call(
        &app,
        json!({ "kind": "recommendedCollaborators", "excludeListId": "l1", "limit": 5 }),
    )
    .await;
    assert_eq!(suggested["ok"], true, "{suggested}");
    assert_eq!(suggested["value"]["recommended"], recommended);
    assert_eq!(suggested["value"]["stats"]["mutual"], 1);
    assert!(suggested["value"]["message"].is_null());

    let none = call(&app, json!({ "kind": "recommendedCollaborators" })).await;
    assert_eq!(none["ok"], true, "{none}");
    assert_eq!(none["value"]["recommended"], json!([]));
    assert!(none["value"]["stats"].is_null());
    assert!(none["value"]["message"]
        .as_str()
        .is_some_and(|message| message.starts_with("No contacts")));

    let requests = transport.requests();
    let search = &requests[0].url;
    assert!(search.contains("q=an"), "trimmed: {search}");
    assert!(search.contains("limit=10"), "the app's default: {search}");
    assert!(search.contains("excludeListId=l1"), "{search}");
    assert!(requests[1].url.contains("limit=5"), "{}", requests[1].url);
    assert!(requests[1].url.contains("excludeListId=l1"));
    assert!(requests[2].url.contains("limit=20"), "{}", requests[2].url);
    assert!(!requests[2].url.contains("excludeListId"));

    // Under two characters the route answers 400; it is not asked.
    let short = call(&app, json!({ "kind": "searchContacts", "query": "a" })).await;
    assert_eq!(short["ok"], false, "{short}");
    assert_eq!(transport.requests().len(), 3);
}

/// A Custom Agent's photo from a picked file, in one command (AWTD-1628): the file goes up through
/// the same upload the profile photo uses, then its address is PATCHed onto the agent, and the
/// answer is the image the server stored — not the one asked for.
#[tokio::test]
async fn a_custom_agent_photo_can_come_from_a_picked_file_awtd_1628() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "v1/upload",
                200,
                json!({ "url": "https://blob.test/agent.png" }),
            )
            .push_json(
                "custom-agents/agents/a1",
                200,
                json!({ "success": true, "image": "https://blob.test/agent.png" }),
            ),
    );
    let photo =
        std::env::temp_dir().join(format!("astrid-agent-{}.png", crate::outbox::new_temp_id()));
    std::fs::write(&photo, b"\x89PNG").expect("writes");

    let set = call(
        &app,
        json!({ "kind": "setCustomAgentPhoto", "agentId": "a1",
                "photoPath": photo.to_string_lossy() }),
    )
    .await;
    let _ = std::fs::remove_file(&photo);

    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["value"]["image"], "https://blob.test/agent.png");
    let requests = transport.requests();
    assert_eq!(requests.len(), 2, "an upload, then a PATCH");
    assert!(
        requests[0].url.ends_with("/api/v1/upload"),
        "{}",
        requests[0].url
    );
    let upload_body = String::from_utf8_lossy(requests[0].body.as_deref().expect("bytes"));
    assert!(upload_body.contains("name=\"file\""));
    assert!(upload_body.contains("image/png"));
    assert_eq!(requests[1].method.as_str(), "PATCH");
    assert!(
        requests[1].url.ends_with("/api/v1/custom-agents/agents/a1"),
        "{}",
        requests[1].url
    );
    let body: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().expect("a body")).expect("json");
    assert_eq!(body, json!({ "image": "https://blob.test/agent.png" }));
}

/// An address and a clear still go straight to the PATCH with nothing uploaded; a file AND an
/// address is refused before any request; a failed upload PATCHes nothing (AWTD-1628).
#[tokio::test]
async fn a_custom_agent_photo_by_address_or_cleared_uploads_nothing_awtd_1628() {
    let (app, transport) = app_and_transport(
        StubTransport::new()
            .push_json(
                "custom-agents/agents/a1",
                200,
                json!({ "success": true, "image": "https://elsewhere.test/a.png" }),
            )
            .push_json(
                "custom-agents/agents/a1",
                200,
                json!({ "success": true, "image": "/images/ai-agents/openclaw.svg" }),
            )
            .push_json(
                "v1/upload",
                500,
                json!({ "error": "Internal server error" }),
            ),
    );

    let by_address = call(
        &app,
        json!({ "kind": "setCustomAgentPhoto", "agentId": "a1",
                "image": "https://elsewhere.test/a.png" }),
    )
    .await;
    assert_eq!(by_address["ok"], true, "{by_address}");
    assert_eq!(by_address["value"]["image"], "https://elsewhere.test/a.png");

    let cleared = call(
        &app,
        json!({ "kind": "setCustomAgentPhoto", "agentId": "a1" }),
    )
    .await;
    assert_eq!(cleared["ok"], true, "{cleared}");
    assert_eq!(cleared["value"]["image"], "/images/ai-agents/openclaw.svg");
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(!requests
        .iter()
        .any(|request| request.url.contains("/upload")));
    let body: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().expect("a body")).expect("json");
    assert_eq!(
        body,
        json!({ "image": null }),
        "a clear still sends image: null"
    );

    let both = call(
        &app,
        json!({ "kind": "setCustomAgentPhoto", "agentId": "a1",
                "photoPath": "/tmp/x.png", "image": "https://elsewhere.test/a.png" }),
    )
    .await;
    assert_eq!(both["ok"], false, "{both}");
    assert_eq!(transport.requests().len(), 2, "refused before any request");

    let photo =
        std::env::temp_dir().join(format!("astrid-agent-{}.png", crate::outbox::new_temp_id()));
    std::fs::write(&photo, b"\x89PNG").expect("writes");
    let failed = call(
        &app,
        json!({ "kind": "setCustomAgentPhoto", "agentId": "a1",
                "photoPath": photo.to_string_lossy() }),
    )
    .await;
    let _ = std::fs::remove_file(&photo);
    assert_eq!(failed["ok"], false, "{failed}");
    let requests = transport.requests();
    assert_eq!(
        requests.len(),
        3,
        "the upload, and no PATCH after it failed"
    );
    assert!(requests[2].url.ends_with("/api/v1/upload"));
}
