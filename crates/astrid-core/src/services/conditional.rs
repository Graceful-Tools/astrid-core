//! Reads that ask whether the answer has changed before paying for it.
//!
//! Task 5751fcb8, finding C3 of the November platform assessment: there was no `If-None-Match`
//! anywhere in this crate. The background pass re-fetched lists, tasks, projects, the feature
//! flags and the notification inbox on every sixty-second tick, in full, for the whole time an app
//! is open — and `app/api/v1/features/route.ts` had been answering 304 to a conditional read since
//! it was written, with nobody asking.
//!
//! Two halves, kept apart on purpose:
//!
//! - [`crate::api::ApiClient::send_conditional`] knows about HTTP and nothing about the cache: it
//!   sends the tag it is given and reports a 304 as `None`.
//! - This module knows where a tag is *kept* — one row in `metadata` per resource — and so needs
//!   the store. Hence a free function over both rather than a method on either: the API layer has
//!   no business reaching into SQLite.
//!
//! **A tag is only ever stored from a response that carried a body.** A validator remembered from
//! a 304 would be remembered from a response that proved nothing about what this client holds.

use crate::api::{ApiClient, ApiError, Request};
use crate::store::Store;

/// The `metadata` row a resource's validator lives in.
///
/// `metadata` is cleared on sign-out, which is the behaviour these rows need: a tag kept across
/// accounts would answer 304 for a cache that is now somebody else's. Prefixed so they are
/// legible in a dump of the table, and findable with [`Store::metadata_with_prefix`].
///
/// An **empty** value means "this deployment does not tag this route" — see [`get_conditional`].
pub fn etag_key(resource: &str) -> String {
    format!("etag:{resource}")
}

/// A conditional read of `resource`: `None` means the server said nothing had changed.
///
/// `None` is a *different answer* from an empty body, which is why this returns an `Option` rather
/// than the JSON. A caller that collapsed them would write "nothing" over its cache on the one
/// tick that confirmed the cache was right — for the feature flags that is every control on the
/// screen turning itself off once a minute.
pub async fn get_conditional(
    client: &ApiClient,
    store: &Store,
    resource: &str,
    request: Request,
) -> Result<Option<serde_json::Value>, ApiError> {
    let key = etag_key(resource);
    // An empty row is the recorded fact that this route answered without a tag. Sending it would
    // ask a question in a form no server answers yes to, on every read, forever.
    let etag = store
        .metadata(&key)
        .ok()
        .flatten()
        .filter(|etag| !etag.is_empty());

    let Some(response) = client.send_conditional(request, etag.as_deref()).await? else {
        return Ok(None);
    };

    // Only now, with a body in hand: see the module note.
    match response.header("etag") {
        Some(etag) => {
            let _ = store.set_metadata(&key, etag);
        }
        // A deployment that does not tag this route must not be answered conditionally next time,
        // or an old tag would sit there asking a question nothing will ever say yes to.
        None => {
            let _ = store.set_metadata(&key, "");
        }
    }

    if response.body.is_empty() {
        return Ok(Some(serde_json::Value::Null));
    }
    serde_json::from_slice(&response.body)
        .map(Some)
        .map_err(|error| ApiError::Decode(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::transport::{HttpResponse, StubTransport};
    use crate::platform::MemorySecureStore;
    use std::sync::Arc;

    fn parts(
        transport: StubTransport,
    ) -> (
        ApiClient,
        Arc<Store>,
        Arc<std::sync::Mutex<Vec<crate::api::transport::HttpRequest>>>,
    ) {
        let recorded = transport.recorded.clone();
        let client = ApiClient::new(
            "https://astrid.cc",
            Arc::new(transport),
            Arc::new(MemorySecureStore::new()),
        );
        (
            client,
            Arc::new(Store::in_memory().expect("opens")),
            recorded,
        )
    }

    fn tagged(
        status: u16,
        etag: Option<&str>,
        body: &str,
    ) -> Result<HttpResponse, crate::api::TransportError> {
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if let Some(etag) = etag {
            headers.push(("etag".to_string(), etag.to_string()));
        }
        Ok(HttpResponse {
            status,
            headers,
            body: body.as_bytes().to_vec(),
        })
    }

    /// Task 5751fcb8: the first read asks for nothing, remembers what it is told, and the second
    /// read quotes it back.
    #[tokio::test]
    async fn task_5751fcb8_the_tag_from_one_read_conditions_the_next() {
        let transport = StubTransport::new()
            .push("/api/v1/features", tagged(200, Some("\"v1\""), "{\"a\":1}"))
            .push("/api/v1/features", tagged(304, Some("\"v1\""), ""));
        let (client, store, recorded) = parts(transport);

        let first = get_conditional(&client, &store, "features", client.get("/api/v1/features"))
            .await
            .expect("reads");
        assert_eq!(first, Some(serde_json::json!({ "a": 1 })));
        assert_eq!(
            store
                .metadata(&etag_key("features"))
                .expect("reads")
                .as_deref(),
            Some("\"v1\"")
        );

        let second = get_conditional(&client, &store, "features", client.get("/api/v1/features"))
            .await
            .expect("a 304 is not a failure");
        assert_eq!(second, None, "unchanged, which is not the same as empty");

        let sent = recorded.lock().expect("lock");
        assert_eq!(sent[0].header("if-none-match"), None);
        assert_eq!(sent[1].header("if-none-match"), Some("\"v1\""));
    }

    /// A deployment that does not tag a route gets ordinary unconditional reads forever, rather
    /// than one stale tag asking a question its server will never answer yes to.
    #[tokio::test]
    async fn task_5751fcb8_an_untagged_route_is_never_asked_conditionally() {
        let transport = StubTransport::new()
            .push("/api/v1/lists", tagged(200, None, "{\"lists\":[]}"))
            .push("/api/v1/lists", tagged(200, None, "{\"lists\":[]}"));
        let (client, store, recorded) = parts(transport);

        for _ in 0..2 {
            get_conditional(&client, &store, "lists", client.get("/api/v1/lists"))
                .await
                .expect("reads");
        }

        let sent = recorded.lock().expect("lock");
        assert!(sent
            .iter()
            .all(|request| request.header("if-none-match").is_none()));
    }
}
