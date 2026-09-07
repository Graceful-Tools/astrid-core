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
use crate::model::User;
use crate::platform::CURRENT_USER_ID_KEY;

/// Cache keys for the things that are one value under one name.
const CURRENT_USER_KEY: &str = "account.current-user";
const CAPABILITIES_KEY: &str = "account.capabilities";
const SETTINGS_KEY: &str = "account.settings";

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
