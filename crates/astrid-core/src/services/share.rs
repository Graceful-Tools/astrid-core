//! Share links (task 016ce981).
//!
//! The web's Share mints a shortcode for the task and hands out its URL (`useTaskShareLink`): the
//! link is made on the server so it can expire and be revoked there. That makes this online-only on
//! every client, and deliberately outside the Outbox — a link that does not exist yet cannot be
//! copied, so there is nothing a journal entry could stand in for.

use super::{Context, Result, ServiceError};
use crate::api::{endpoints, ApiError};
use serde_json::json;

pub struct ShareService {
    context: Context,
}

impl ShareService {
    pub fn new(context: Context) -> Self {
        ShareService { context }
    }

    /// Mint a share link for a task and return its address, as the web's Share does.
    pub async fn link_for_task(&self, task_id: &str) -> Result<String> {
        self.link_for("task", task_id).await
    }

    /// Mint a share link for a list — iOS's Share List (AATD-44).
    pub async fn link_for_list(&self, list_id: &str) -> Result<String> {
        self.link_for("list", list_id).await
    }

    /// What a share link opens: `{ targetType: "task" | "list", targetId }` (AATD-59). iOS's
    /// DeepLinkManager resolves `astrid.cc/s/<code>` the same way.
    pub async fn resolve(&self, code: &str) -> Result<serde_json::Value> {
        let answer = self
            .context
            .client
            .send(self.context.client.get(endpoints::shortcode(code)))
            .await?;
        Ok(json!({
            "targetType": answer.get("targetType").cloned().unwrap_or(serde_json::Value::Null),
            "targetId": answer.get("targetId").cloned().unwrap_or(serde_json::Value::Null),
        }))
    }

    async fn link_for(&self, target_type: &str, target_id: &str) -> Result<String> {
        let request = self
            .context
            .client
            .post(endpoints::SHORTCODES)
            .value(json!({ "targetType": target_type, "targetId": target_id }));
        let answer = self.context.client.send(request).await?;
        answer
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ServiceError::Api(ApiError::Decode(
                    "the server minted a share link without an address".into(),
                ))
            })
    }
}
