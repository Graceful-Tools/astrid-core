//! Staying connected to the live stream.
//!
//! Ported from the connection loop in `astrid-ios/Astrid App/Core/RealTime/SSEClient.swift`.
//!
//! The loop is small because the two hard parts are elsewhere and pure: [`super::reconnect`]
//! decides whether and when to try again, and [`super::parse`] reads a frame. What is left is
//! connecting, reading, and knowing when to stop — and one rule that is easy to get wrong:
//!
//! **A connection that succeeded resets the count.** A stream that stayed up for an hour and then
//! dropped is not on its fifth consecutive failure; treating it as one gives up on a healthy
//! connection after a bad afternoon.

use std::sync::Arc;

use futures_util::StreamExt;

use super::{reconnect::ReconnectPolicy, RealtimeSink, SSE_PATH};
use crate::api::{ApiClient, ApiError};

/// Why the loop stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// The session is gone. Reconnecting cannot help; sign-in has to happen first.
    Unauthorized,
    /// It failed enough times in a row to give up. A wake or a network change should start it
    /// again with a fresh count — see [`super::reconnect`].
    OutOfAttempts,
    /// Somebody asked it to stop.
    Cancelled,
}

/// Connect, read, and keep reconnecting until told not to.
///
/// `should_continue` is checked between attempts, so a caller can stop the loop without needing a
/// channel: the shell owns the lifetime of this task and stopping it must not depend on the server
/// sending anything.
pub async fn run(
    client: Arc<ApiClient>,
    sink: Arc<RealtimeSink>,
    mut should_continue: impl FnMut() -> bool + Send,
) -> Stopped {
    let mut policy = ReconnectPolicy::new();

    loop {
        if !should_continue() {
            return Stopped::Cancelled;
        }

        match client.open_stream(client.get(SSE_PATH)).await {
            Ok(mut frames) => {
                // It connected: whatever failed before describes a world that is gone.
                policy.connected();
                while let Some(frame) = frames.next().await {
                    if !should_continue() {
                        return Stopped::Cancelled;
                    }
                    match frame {
                        Ok(frame) => {
                            sink.receive(&frame);
                        }
                        Err(error) => {
                            tracing::debug!(%error, "the live stream dropped; reconnecting");
                            break;
                        }
                    }
                }
            }
            Err(ApiError::Unauthorized) => return Stopped::Unauthorized,
            Err(error) => {
                tracing::debug!(%error, "the live stream could not be opened");
            }
        }

        match policy.failed() {
            Some(wait) => tokio::time::sleep(wait).await,
            None => return Stopped::OutOfAttempts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::transport::{
        FrameStream, HttpRequest, HttpResponse, HttpTransport, TransportError,
    };
    use crate::platform::MemorySecureStore;
    use crate::store::Store;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// A transport that hands out scripted stream attempts.
    struct StreamingStub {
        attempts: Mutex<Vec<Result<Vec<String>, TransportError>>>,
        opened: Mutex<usize>,
    }

    impl StreamingStub {
        fn new(attempts: Vec<Result<Vec<String>, TransportError>>) -> Self {
            StreamingStub {
                attempts: Mutex::new(attempts),
                opened: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl HttpTransport for StreamingStub {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
            Err(TransportError::Invalid("this stub only streams".into()))
        }

        async fn open_stream(&self, _request: HttpRequest) -> Result<FrameStream, TransportError> {
            *self.opened.lock().expect("lock") += 1;
            let mut attempts = self.attempts.lock().expect("lock");
            if attempts.is_empty() {
                return Err(TransportError::Unreachable("nothing left".into()));
            }
            match attempts.remove(0) {
                Ok(frames) => Ok(Box::pin(futures_util::stream::iter(
                    frames.into_iter().map(Ok),
                ))),
                Err(error) => Err(error),
            }
        }
    }

    fn client(stub: Arc<StreamingStub>) -> Arc<ApiClient> {
        Arc::new(ApiClient::new(
            "https://astrid.cc",
            stub,
            Arc::new(MemorySecureStore::new()),
        ))
    }

    #[tokio::test(start_paused = true)]
    async fn frames_from_the_stream_reach_the_cache() {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let sink = Arc::new(RealtimeSink::new(store.clone()));
        let stub = Arc::new(StreamingStub::new(vec![Ok(vec![
            "data: {\"type\":\"task_created\",\"data\":{\"id\":\"t1\",\"title\":\"Buy milk\"}}\n\n"
                .to_string(),
        ])]));

        let mut passes = 0;
        let stopped = run(client(stub), sink, move || {
            passes += 1;
            // One connection, then stop: the loop would otherwise reconnect forever, which is
            // what it is supposed to do.
            passes <= 2
        })
        .await;

        assert_eq!(stopped, Stopped::Cancelled);
        assert_eq!(
            store.task("t1").expect("reads").expect("present").title,
            "Buy milk"
        );
    }

    /// Reconnecting cannot fix a session that is gone, and trying gets the client rate-limited on
    /// top of being signed out.
    #[tokio::test(start_paused = true)]
    async fn an_expired_session_stops_the_loop_rather_than_reconnecting() {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let sink = Arc::new(RealtimeSink::new(store));
        let stub = Arc::new(StreamingStub::new(vec![Err(TransportError::Refused(401))]));

        let stopped = run(client(stub.clone()), sink, || true).await;
        assert_eq!(stopped, Stopped::Unauthorized);
        assert_eq!(
            *stub.opened.lock().expect("lock"),
            1,
            "one attempt, then stop — a signed-out client must not hammer the endpoint"
        );
    }

    /// Five failures in a row and it stops, rather than reconnecting into a network that is not
    /// there for the rest of the afternoon.
    #[tokio::test(start_paused = true)]
    async fn it_gives_up_after_its_attempts_are_gone() {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let sink = Arc::new(RealtimeSink::new(store));
        let stub = Arc::new(StreamingStub::new(vec![]));

        let stopped = run(client(stub.clone()), sink, || true).await;
        assert_eq!(stopped, Stopped::OutOfAttempts);
        assert_eq!(*stub.opened.lock().expect("lock"), 5);
    }

    /// A stream that stayed up and then dropped is not on its fifth consecutive failure. Counting
    /// it as one gives up on a healthy connection after a bad afternoon.
    #[tokio::test(start_paused = true)]
    async fn a_connection_that_worked_resets_the_failure_count() {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let sink = Arc::new(RealtimeSink::new(store));
        let stub = Arc::new(StreamingStub::new(vec![
            Err(TransportError::Unreachable("no".into())),
            Err(TransportError::Unreachable("no".into())),
            Err(TransportError::Unreachable("no".into())),
            Err(TransportError::Unreachable("no".into())),
            // The fifth attempt connects, which resets the count...
            Ok(vec!["data: {\"type\":\"ping\"}\n\n".to_string()]),
        ]));

        let stopped = run(client(stub.clone()), sink, || true).await;
        assert_eq!(stopped, Stopped::OutOfAttempts);
        // ...so it gets a full five attempts after that one, rather than stopping at five in
        // total. Nine opens: four failures, the connection, then five more.
        assert_eq!(*stub.opened.lock().expect("lock"), 9);
    }
}
