//! The inbox (web task ab0572cb): assigned, mentioned, replied, commented, status changed,
//! completed.
//!
//! Read from the cache and refreshed from the server — on every sync pass, since the web sends
//! no live event for it. Marking read goes straight to the server and is not journalled: an
//! acknowledgement is not somebody's work, and one replayed a week later would un-badge an inbox
//! the person has long since read on their phone. The cache is updated at once regardless, so the
//! badge clears on the click.

use serde_json::json;

use super::{Context, Result};
use crate::api::endpoints;
use crate::model::{Inbox, Notification};

const INBOX_KEY: &str = "account.notifications";

pub struct NotificationService {
    context: Context,
}

impl NotificationService {
    pub fn new(context: Context) -> Self {
        NotificationService { context }
    }

    /// The inbox as last fetched. Empty before the first refresh.
    pub fn inbox(&self) -> Result<Inbox> {
        Ok(self
            .context
            .store
            .metadata(INBOX_KEY)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    /// Fetch the inbox and keep it.
    pub async fn refresh(&self) -> Result<Inbox> {
        let answer = self
            .context
            .client
            .send(
                self.context
                    .client
                    .get(endpoints::NOTIFICATIONS)
                    .query("limit", Some("50".to_string())),
            )
            .await?;
        let notifications = crate::model::lenient::<Notification>(
            answer
                .get("notifications")
                .cloned()
                .unwrap_or(serde_json::Value::Array(Vec::new())),
        )
        .into_items();
        let unread_count = answer
            .get("unreadCount")
            .and_then(serde_json::Value::as_u64)
            .map(|count| count as usize)
            .unwrap_or_else(|| notifications.iter().filter(|n| !n.is_read()).count());
        let inbox = Inbox {
            notifications,
            unread_count,
        };
        self.remember(&inbox)?;
        Ok(inbox)
    }

    /// Mark some rows read: in the cache now, on the server now.
    pub async fn mark_read(&self, ids: &[String]) -> Result<Inbox> {
        if ids.is_empty() {
            return self.inbox();
        }
        let inbox = self.mark_locally(|notification| ids.contains(&notification.id))?;
        let request = self
            .context
            .client
            .put(endpoints::NOTIFICATIONS)
            .value(json!({ "ids": ids }));
        self.context.client.send(request).await?;
        Ok(inbox)
    }

    /// Mark everything read.
    pub async fn mark_all_read(&self) -> Result<Inbox> {
        let inbox = self.mark_locally(|_| true)?;
        let request = self
            .context
            .client
            .put(endpoints::NOTIFICATIONS)
            .value(json!({ "all": true }));
        self.context.client.send(request).await?;
        Ok(inbox)
    }

    fn mark_locally(&self, chosen: impl Fn(&Notification) -> bool) -> Result<Inbox> {
        let now = self.context.clock.now();
        let mut inbox = self.inbox()?;
        for notification in &mut inbox.notifications {
            if notification.read_at.is_none() && chosen(notification) {
                notification.read_at = Some(now);
            }
        }
        inbox.unread_count = inbox
            .notifications
            .iter()
            .filter(|notification| !notification.is_read())
            .count();
        self.remember(&inbox)?;
        Ok(inbox)
    }

    fn remember(&self, inbox: &Inbox) -> Result<()> {
        self.context
            .store
            .set_metadata(INBOX_KEY, &serde_json::to_string(inbox).unwrap_or_default())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport};
    use crate::platform::{FixedClock, MemorySecureStore, SESSION_COOKIE_KEY};
    use crate::store::Store;
    use std::sync::Arc;

    fn service(transport: StubTransport) -> (NotificationService, Arc<StubTransport>) {
        let transport = Arc::new(transport);
        let store = Arc::new(Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                transport.clone(),
                Arc::new(MemorySecureStore::with(
                    SESSION_COOKIE_KEY,
                    "next-auth.session-token=abc",
                )),
            )),
            store,
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        );
        (NotificationService::new(context), transport)
    }

    fn inbox_json() -> serde_json::Value {
        json!({
            "notifications": [
                { "id": "n1", "kind": "assigned", "taskId": "t1", "readAt": null,
                  "createdAt": "2026-09-07T11:00:00Z",
                  "task": { "id": "t1", "identifier": "AST-142", "title": "Book flights", "completed": false } },
                { "id": "n2", "kind": "mentioned", "taskId": "t2", "readAt": "2026-09-06T09:00:00Z",
                  "createdAt": "2026-09-06T08:00:00Z",
                  "task": { "id": "t2", "title": "Write it up", "completed": true } },
                { "id": "n3", "kind": "something_newer", "taskId": null, "readAt": null }
            ],
            "unreadCount": 2
        })
    }

    #[tokio::test]
    async fn the_inbox_is_kept_and_read_back_from_the_cache() {
        let (service, _) =
            service(StubTransport::new().push_json("/api/v1/notifications", 200, inbox_json()));
        assert_eq!(service.inbox().expect("reads"), Inbox::default());

        let fetched = service.refresh().await.expect("refreshes");
        assert_eq!(fetched.notifications.len(), 3);
        assert_eq!(fetched.unread_count, 2);
        assert_eq!(
            fetched.notifications[0]
                .task
                .as_ref()
                .map(|t| t.identifier.as_deref()),
            Some(Some("AST-142"))
        );
        assert_eq!(
            fetched.notifications[2].kind, "something_newer",
            "a kind this build has not heard of still arrives"
        );
        assert_eq!(service.inbox().expect("reads"), fetched);
    }

    /// The badge clears on the click, and the server hears which rows.
    #[tokio::test]
    async fn marking_read_clears_the_badge_at_once_and_tells_the_server() {
        let (service, transport) = service(
            StubTransport::new()
                .push_json("/api/v1/notifications", 200, inbox_json())
                .push_json("/api/v1/notifications", 200, json!({ "updated": 1 })),
        );
        service.refresh().await.expect("refreshes");

        let after = service.mark_read(&["n1".to_string()]).await.expect("marks");
        assert_eq!(after.unread_count, 1);
        assert!(after.notifications[0].is_read());
        assert!(!after.notifications[2].is_read());

        let put = transport
            .requests()
            .into_iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("a PUT");
        let body: serde_json::Value =
            serde_json::from_slice(put.body.as_deref().unwrap_or(b"{}")).expect("json");
        assert_eq!(body, json!({ "ids": ["n1"] }));
    }

    #[tokio::test]
    async fn marking_everything_read_says_all() {
        let (service, transport) = service(
            StubTransport::new()
                .push_json("/api/v1/notifications", 200, inbox_json())
                .push_json("/api/v1/notifications", 200, json!({ "updated": 2 })),
        );
        service.refresh().await.expect("refreshes");

        let after = service.mark_all_read().await.expect("marks");
        assert_eq!(after.unread_count, 0);
        let put = transport
            .requests()
            .into_iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("a PUT");
        let body: serde_json::Value =
            serde_json::from_slice(put.body.as_deref().unwrap_or(b"{}")).expect("json");
        assert_eq!(body, json!({ "all": true }));
    }
}
