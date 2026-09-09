//! The signed-in user: who they are, what they have set, and what the server can do.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/AccountService.swift`,
//! `UserSettingsService.swift` and `ServerCapabilityService.swift`.
//!
//! ## Why capabilities are cached and read pessimistically
//!
//! This client ships to the Store and updates on its own schedule; the server deploys on another.
//! A feature the client can draw may not exist on the deployment it is talking to, and the failure
//! mode of guessing wrong is a control that does nothing. So capabilities are fetched, cached, and
//! **absent means no** — a client that assumed yes would show the feature to everyone on an older
//! deployment and only find out from a support ticket.

use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;
use crate::filters::my_tasks::Preferences as MyTasksPreferences;
use crate::model::User;
use crate::platform::CURRENT_USER_ID_KEY;

/// Cache keys for the things that are one value under one name.
const CURRENT_USER_KEY: &str = "account.current-user";

/// Where My Tasks' filters are cached, so the view draws before the network answers.
const MY_TASKS_PREFERENCES_KEY: &str = "account.myTasksPreferences";
const CAPABILITIES_KEY: &str = "account.capabilities";
const SETTINGS_KEY: &str = "account.settings";
const SMART_TASKS_KEY: &str = "account.smartTasks";

/// What the server makes somebody type before it deletes their account, character for character
/// (web's `AccountDeletionSection`). Checked here too, so a near miss is refused without a request.
pub const DELETE_CONFIRMATION: &str = "DELETE MY ACCOUNT";

pub struct AccountService {
    context: Context,
}

impl AccountService {
    pub fn new(context: Context) -> Self {
        AccountService { context }
    }

    /// The signed-in user, from the cache. Available before the first request comes back, which is
    /// what lets the first frame draw an avatar rather than a placeholder that then changes.
    pub fn current_user(&self) -> Result<Option<User>> {
        Ok(self
            .context
            .store
            .metadata(CURRENT_USER_KEY)?
            .and_then(|json| serde_json::from_str(&json).ok()))
    }

    pub fn current_user_id(&self) -> Result<Option<String>> {
        Ok(self.current_user()?.map(|user| user.id))
    }

    /// Remember who is signed in. Called by sign-in, before anything is fetched.
    pub async fn set_current_user(&self, user: &User) -> Result<()> {
        self.context
            .store
            .upsert_users(std::slice::from_ref(user))?;
        self.context.store.set_metadata(
            CURRENT_USER_KEY,
            &serde_json::to_string(user).unwrap_or_default(),
        )?;
        // Beside the credential as well, so the shell can name the user before the cache is open.
        let _ = self
            .context
            .client
            .secure_store()
            .set(CURRENT_USER_ID_KEY, &user.id)
            .await;
        Ok(())
    }

    pub async fn refresh_current_user(&self) -> Result<User> {
        let value = self
            .context
            .client
            .send(self.context.client.get(endpoints::ME))
            .await?;
        let user: User = serde_json::from_value(value.get("user").cloned().unwrap_or(value))
            .map_err(|error| crate::api::ApiError::Decode(error.to_string()))?;
        self.set_current_user(&user).await?;
        Ok(user)
    }

    /// Change the signed-in user's name, photo, or both (task 19fd9289). Either may be left alone:
    /// the server keeps what it is not sent. The user is fetched back afterwards so the cache — and
    /// every avatar drawn from it — says what the server now says.
    pub async fn update_profile(&self, name: Option<&str>, image: Option<&str>) -> Result<User> {
        let mut body = serde_json::Map::new();
        if let Some(name) = name {
            body.insert("name".into(), json!(name.trim()));
        }
        if let Some(image) = image {
            body.insert("image".into(), json!(image));
        }
        let request = self
            .context
            .client
            .put(endpoints::ME)
            .value(serde_json::Value::Object(body));
        self.context.client.send(request).await?;
        self.refresh_current_user().await
    }

    /// Put a picture on the server and return its address, for [`Self::update_profile`].
    ///
    /// Straight to the upload route the web's own avatar goes through, and not the Outbox: a photo
    /// is chosen while somebody watches, and one queued to appear later would change their face
    /// unbidden, possibly on another day.
    pub async fn upload_photo(&self, path: &std::path::Path) -> Result<String> {
        let bytes = std::fs::read(path)
            .map_err(|error| super::ServiceError::LocalFile(error.to_string()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("photo");
        let mime = super::attachment::mime_for(path);
        let boundary = format!("astrid-photo-{}", crate::outbox::new_temp_id());
        let body = super::attachment::multipart(&boundary, name, &mime, &bytes, "{}");
        let request = self
            .context
            .client
            .post(endpoints::UPLOAD)
            .bytes(format!("multipart/form-data; boundary={boundary}"), body);
        let answer = self.context.client.send(request).await?;
        answer
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                super::ServiceError::Api(crate::api::ApiError::Decode(
                    "the upload answered without an address".into(),
                ))
            })
    }

    /// Ask the server to send the verification email again (task 19fd9289).
    ///
    /// The action goes in the query string, which is where the v1 route reads it, and in the body
    /// as well, which is where the web's own settings page puts it — whichever the server honours.
    pub async fn resend_verification(&self) -> Result<serde_json::Value> {
        let request = self
            .context
            .client
            .post(endpoints::VERIFY_EMAIL)
            .query("action", Some("resend".to_string()))
            .value(json!({ "action": "resend" }));
        Ok(self.context.client.send(request).await?)
    }

    /// Delete the account, for good (task 19fd9289). The server checks the phrase; so does the
    /// command that calls this. What is left on this machine afterwards is [`Self::sign_out`]'s.
    pub async fn delete_account(&self, confirmation: &str) -> Result<()> {
        let request = self
            .context
            .client
            .post(endpoints::DELETE_ACCOUNT)
            .value(json!({ "confirmationText": confirmation }));
        self.context.client.send(request).await?;
        Ok(())
    }

    // ─── Settings ─────────────────────────────────────────────────────────────────────────────

    /// The user's settings as last fetched. Free-form: the set grows on the server between client
    /// releases, and a typed struct here would drop whatever it had not heard of.
    pub fn settings(&self) -> Result<serde_json::Value> {
        Ok(self
            .context
            .store
            .metadata(SETTINGS_KEY)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| json!({})))
    }

    pub fn setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self.settings()?.get(key).cloned())
    }

    pub async fn refresh_settings(&self) -> Result<serde_json::Value> {
        let value = self
            .context
            .client
            .send(self.context.client.get(endpoints::USER_SETTINGS))
            .await?;
        let settings = value.get("settings").cloned().unwrap_or(value);
        self.context
            .store
            .set_metadata(SETTINGS_KEY, &settings.to_string())?;
        Ok(settings)
    }

    /// Change a setting. Written through, and cached optimistically so the toggle stays where the
    /// user put it while the request is in flight.
    pub async fn update_settings(&self, changes: serde_json::Value) -> Result<serde_json::Value> {
        let mut merged = self.settings()?;
        if let (Some(target), Some(source)) = (merged.as_object_mut(), changes.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        self.context
            .store
            .set_metadata(SETTINGS_KEY, &merged.to_string())?;

        let request = self
            .context
            .client
            .put(endpoints::USER_SETTINGS)
            .value(changes);
        self.context.client.send(request).await?;
        Ok(merged)
    }

    // ─── Task defaults and layout ─────────────────────────────────────────────────────────────

    /// The account's task defaults and task-detail layout as last fetched (task c0f3db19).
    /// Free-form like the settings above; [`crate::smart_tasks`] gives them shape and defaults.
    pub fn smart_task_settings(&self) -> Result<serde_json::Value> {
        Ok(self
            .context
            .store
            .metadata(SMART_TASKS_KEY)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| json!({})))
    }

    /// Fetch them. The server answers with the user's columns and an envelope; the envelope is not
    /// a setting and does not go into the cache.
    pub async fn refresh_smart_task_settings(&self) -> Result<serde_json::Value> {
        let mut value = self
            .context
            .client
            .send(self.context.client.get(endpoints::SMART_TASKS))
            .await?;
        if let Some(object) = value.as_object_mut() {
            object.remove("meta");
        }
        self.context
            .store
            .set_metadata(SMART_TASKS_KEY, &value.to_string())?;
        Ok(value)
    }

    /// Change some of them. Merged into the cache first so the control stays where it was put
    /// while the request is in flight — the same bargain [`Self::update_settings`] makes.
    pub async fn update_smart_task_settings(
        &self,
        changes: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut merged = self.smart_task_settings()?;
        if let (Some(target), Some(source)) = (merged.as_object_mut(), changes.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        self.context
            .store
            .set_metadata(SMART_TASKS_KEY, &merged.to_string())?;

        let request = self
            .context
            .client
            .patch(endpoints::SMART_TASKS)
            .value(changes);
        self.context.client.send(request).await?;
        Ok(merged)
    }

    /// The task-detail layout the account chose: what rows and the detail draw with when the shell
    /// does not say otherwise. From the cache, so a choice made here applies at once and one made
    /// on the web applies after the next settings refresh.
    pub fn display_mode(&self) -> crate::rows::DisplayMode {
        crate::smart_tasks::SmartTaskSettings::from_stored(
            &self.smart_task_settings().unwrap_or_default(),
        )
        .display_mode()
    }

    // ─── Capabilities ─────────────────────────────────────────────────────────────────────────

    /// Whether the deployment this client is talking to supports `name`.
    ///
    /// **Absent means no.** See the module note: assuming yes shows a control that does nothing to
    /// everyone on an older server.
    pub fn supports(&self, name: &str) -> Result<bool> {
        Ok(self
            .capabilities()?
            .get(name)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false))
    }

    pub fn capabilities(&self) -> Result<serde_json::Value> {
        Ok(self
            .context
            .store
            .metadata(CAPABILITIES_KEY)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| json!({})))
    }

    pub async fn refresh_capabilities(&self) -> Result<serde_json::Value> {
        let value = self
            .context
            .client
            .send(self.context.client.get(endpoints::CAPABILITIES))
            .await?;
        let capabilities = value.get("capabilities").cloned().unwrap_or(value);
        self.context
            .store
            .set_metadata(CAPABILITIES_KEY, &capabilities.to_string())?;
        Ok(capabilities)
    }

    // ─── People ───────────────────────────────────────────────────────────────────────────────

    /// Search for people to assign or invite. Straight to the server — a search answered from a
    /// cache that only holds people you have already worked with cannot find anyone new.
    /// The three numbers on somebody's profile: finished, inspired, supported.
    ///
    /// Fetched rather than counted here. They are about the whole account across every device, and
    /// a client counting its own cache would answer with whatever it happens to have synced.
    pub async fn stats(&self, user_id: &str) -> Result<serde_json::Value> {
        let request = self.context.client.get(endpoints::user_profile(user_id));
        let answer = self.context.client.send(request).await?;
        Ok(answer.get("stats").cloned().unwrap_or(json!({})))
    }

    /// Everything this account has, as bytes, written to `path`.
    ///
    /// Straight to a file rather than back across the boundary: an export is megabytes of somebody's
    /// entire history, and carrying it through JSON to hand it to a save dialog would be work for
    /// its own sake.
    pub async fn export(&self, format: &str, path: &std::path::Path) -> Result<u64> {
        let request = self
            .context
            .client
            .get(endpoints::EXPORT)
            .query("format", Some(format.to_string()));
        let response = self.context.client.send_raw(request).await?;
        std::fs::write(path, &response.body)
            .map_err(|error| super::ServiceError::LocalFile(error.to_string()))?;
        Ok(response.body.len() as u64)
    }

    pub async fn search_users(&self, query: &str) -> Result<Vec<User>> {
        let request = self
            .context
            .client
            .get(endpoints::USER_SEARCH)
            .query("q", Some(query.to_string()));
        let found = self
            .context
            .client
            .send_collection::<User>(request, Some(endpoints::envelope::USERS))
            .await?;
        // Cached so avatars and names resolve offline afterwards.
        self.context.store.upsert_users(&found.items)?;
        Ok(found.into_items())
    }

    pub fn user(&self, id: &str) -> Result<Option<User>> {
        Ok(self.context.store.user(id)?)
    }

    /// What My Tasks is filtered and sorted by, for this account.
    ///
    /// From the cache first and the server second, in that order and always: this decides what a
    /// screen draws, and a screen that waits for the network to say "no filters" is a screen that
    /// is empty on a train. The server's answer replaces the cached one when it arrives.
    pub fn my_tasks_preferences(&self) -> Result<MyTasksPreferences> {
        Ok(self
            .context
            .store
            .metadata(MY_TASKS_PREFERENCES_KEY)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    /// Fetch them, and remember what came back.
    pub async fn refresh_my_tasks_preferences(&self) -> Result<MyTasksPreferences> {
        let request = self.context.client.get(endpoints::MY_TASKS_PREFERENCES);
        let answer = self.context.client.send(request).await?;
        // A blob the server has been storing for years, so anything it cannot read falls back to
        // the defaults rather than failing the screen.
        let preferences: MyTasksPreferences = serde_json::from_value(answer).unwrap_or_default();
        self.remember_my_tasks_preferences(&preferences)?;
        Ok(preferences)
    }

    /// Change them. On screen immediately; the server hears about it now, not later.
    ///
    /// Not through the Outbox, unlike a task: a filter is a preference rather than somebody's
    /// work, and a queued filter change replayed after a week would move a screen under whoever is
    /// looking at it. It is written locally either way, so the choice survives being offline on
    /// this machine even when the account never hears it.
    pub async fn set_my_tasks_preferences(
        &self,
        preferences: &MyTasksPreferences,
    ) -> Result<MyTasksPreferences> {
        self.remember_my_tasks_preferences(preferences)?;
        let request = self
            .context
            .client
            .patch(endpoints::MY_TASKS_PREFERENCES)
            .value(serde_json::to_value(preferences).unwrap_or_default());
        let answer = self.context.client.send(request).await?;
        let confirmed: MyTasksPreferences = serde_json::from_value(answer).unwrap_or_default();
        self.remember_my_tasks_preferences(&confirmed)?;
        Ok(confirmed)
    }

    fn remember_my_tasks_preferences(&self, preferences: &MyTasksPreferences) -> Result<()> {
        self.context.store.set_metadata(
            MY_TASKS_PREFERENCES_KEY,
            &serde_json::to_string(preferences).unwrap_or_default(),
        )?;
        Ok(())
    }

    /// Forget everything. Sign-out: the cache, the journal, the credential.
    pub async fn sign_out(&self) -> Result<()> {
        self.context.store.clear()?;
        let store = self.context.client.secure_store();
        let _ = store.delete(crate::platform::SESSION_COOKIE_KEY).await;
        let _ = store.delete(CURRENT_USER_ID_KEY).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport};
    use crate::model::Task;
    use crate::platform::{FixedClock, MemorySecureStore, SecureStore, SESSION_COOKIE_KEY};
    use crate::store::Store;
    use std::sync::Arc;

    struct Fixture {
        service: AccountService,
        store: Arc<Store>,
        secure: Arc<MemorySecureStore>,
    }

    fn fixture(transport: StubTransport) -> Fixture {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let secure = Arc::new(MemorySecureStore::with(
            SESSION_COOKIE_KEY,
            "next-auth.session-token=abc",
        ));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                Arc::new(transport),
                secure.clone(),
            )),
            store.clone(),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        );
        Fixture {
            service: context.account(),
            store,
            secure,
        }
    }

    fn user(id: &str) -> User {
        serde_json::from_value(json!({ "id": id, "email": "ada@example.com", "name": "Ada" }))
            .expect("decodes")
    }

    #[tokio::test]
    async fn the_signed_in_user_is_readable_before_any_request_comes_back() {
        let fixture = fixture(StubTransport::new());
        assert!(fixture.service.current_user().expect("reads").is_none());

        fixture
            .service
            .set_current_user(&user("u1"))
            .await
            .expect("stores");
        assert_eq!(
            fixture.service.current_user_id().expect("reads").as_deref(),
            Some("u1")
        );
        assert_eq!(
            fixture.secure.get(CURRENT_USER_ID_KEY).await.as_deref(),
            Some("u1"),
            "the id sits beside the credential so the shell can name the user before the cache opens"
        );
    }

    /// A control the server cannot honour must not be drawn. Absent is no.
    #[tokio::test]
    async fn an_unknown_capability_is_read_as_unsupported() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/capabilities",
            200,
            json!({ "capabilities": { "chat": true, "board": false } }),
        ));
        assert!(!fixture.service.supports("chat").expect("reads"));

        fixture
            .service
            .refresh_capabilities()
            .await
            .expect("refreshes");
        assert!(fixture.service.supports("chat").expect("reads"));
        assert!(!fixture.service.supports("board").expect("reads"));
        assert!(
            !fixture.service.supports("somethingLater").expect("reads"),
            "a capability nobody has heard of is not a capability"
        );
    }

    /// The settings set grows on the server between client releases. A typed struct would drop
    /// whatever this build had not heard of, and write the loss back on the next save.
    #[tokio::test]
    async fn settings_this_build_does_not_know_survive_a_change_to_one_it_does() {
        let fixture = fixture(
            StubTransport::new()
                .push_json(
                    "/api/v1/users/me/settings",
                    200,
                    json!({ "settings": { "theme": "dark", "somethingLater": 42 } }),
                )
                .push_json("/api/v1/users/me/settings", 200, json!({ "ok": true })),
        );
        fixture.service.refresh_settings().await.expect("refreshes");

        let merged = fixture
            .service
            .update_settings(json!({ "theme": "light" }))
            .await
            .expect("updates");
        assert_eq!(merged["theme"], "light");
        assert_eq!(merged["somethingLater"], 42);
    }

    /// A toggle that springs back while the request is in flight reads as a failure that has not
    /// happened yet.
    #[tokio::test]
    async fn a_changed_setting_stays_where_the_user_put_it() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/users/me/settings",
            200,
            json!({ "ok": true }),
        ));
        fixture
            .service
            .update_settings(json!({ "theme": "dark" }))
            .await
            .expect("updates");
        assert_eq!(
            fixture.service.setting("theme").expect("reads"),
            Some(json!("dark"))
        );
    }

    /// A cache that outlives its session shows one person's tasks to the next.
    #[tokio::test]
    async fn signing_out_leaves_no_trace_of_the_previous_session() {
        let fixture = fixture(StubTransport::new());
        fixture
            .service
            .set_current_user(&user("u1"))
            .await
            .expect("stores");
        fixture
            .store
            .upsert_task(&Task::new("t1", "Buy milk"))
            .expect("stores");

        fixture.service.sign_out().await.expect("signs out");
        assert!(fixture.store.tasks().expect("reads").is_empty());
        assert!(fixture.service.current_user().expect("reads").is_none());
        assert_eq!(fixture.secure.get(SESSION_COOKIE_KEY).await, None);
    }

    /// Sync state is per-account. A tombstone or a queued remote deletion left behind would have
    /// the next person on this machine deleting a stranger's Google task.
    #[tokio::test]
    async fn signing_out_takes_the_sync_ledger_with_it() {
        let fixture = fixture(StubTransport::new());
        crate::external::ledger::record_deletion(&fixture.store, "google", "r1", "c1")
            .expect("records");
        crate::external::ledger::remember_links(
            &fixture.store,
            "google",
            "c1",
            [("t1".to_string(), "r1".to_string())],
        )
        .expect("remembers");

        fixture.service.sign_out().await.expect("signs out");

        assert!(crate::external::ledger::pending(&fixture.store, "google").is_empty());
        assert!(crate::external::ledger::tombstoned(&fixture.store, "google").is_empty());
        assert!(crate::external::ledger::twin(&fixture.store, "google", "t1").is_none());
    }

    #[tokio::test]
    async fn searching_for_people_caches_what_it_finds() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/users/search",
            200,
            json!({ "users": [{ "id": "u9", "name": "Ada", "email": "ada@example.com" }] }),
        ));
        let found = fixture.service.search_users("ada").await.expect("searches");
        assert_eq!(found.len(), 1);
        assert_eq!(
            fixture
                .service
                .user("u9")
                .expect("reads")
                .expect("present")
                .display_name(),
            "Ada"
        );
    }
}
