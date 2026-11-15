//! Whether this process's TLS can verify a certificate yet (AWTD-1673).
//!
//! On every platform but Android the HTTP client verifies certificates with the platform's own TLS
//! (schannel, Security.framework, OpenSSL) and needs nothing from the shell. On Android it is rustls
//! with `rustls-platform-verifier`, which asks Android's trust manager — the device's trust store,
//! user-installed and enterprise roots included — and to do that it calls into the JVM. It cannot
//! find the JVM on its own: the app hands it a `JNIEnv` and a `Context` once, at startup, through
//! [`android::init`], which the binding crate exposes to Kotlin.
//!
//! Until then the verifier is not merely unready, it panics on first use. So the transport asks
//! [`platform_tls_ready`] before every request and answers
//! [`TransportError::TlsNotInitialized`](super::TransportError::TlsNotInitialized) instead of
//! reaching it — a typed, retryable failure (a queued write waits in the Outbox for the init rather
//! than being dropped), never a panic.
//!
//! The state itself is target-agnostic so that the pre-init path is an ordinary unit test on the
//! desktop: a transport can be built gated on any readiness function, and on Android
//! [`ReqwestTransport::new`](super::ReqwestTransport::new) gates on this one.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the platform verifier needs the shell's init before it can be used. Only Android's does.
pub const NEEDS_PLATFORM_INIT: bool = cfg!(target_os = "android");

/// Set once [`android::init`] has handed the verifier its JVM. Never cleared: the verifier keeps
/// its handles for the life of the process.
static PLATFORM_TLS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Whether a request can be verified now: always on a platform whose TLS needs no init, and on
/// Android once the app has called the init.
pub fn platform_tls_ready() -> bool {
    !NEEDS_PLATFORM_INIT || PLATFORM_TLS_INITIALIZED.load(Ordering::Acquire)
}

/// Record that the verifier has what it needs. Private to this module: the only caller is the
/// init that actually gave it that, so the flag cannot claim a readiness the verifier lacks.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn mark_initialized() {
    PLATFORM_TLS_INITIALIZED.store(true, Ordering::Release);
}

/// The init could not hand the verifier its JVM.
#[derive(Debug, Clone, thiserror::Error)]
#[error("could not initialise the platform certificate verifier: {0}")]
pub struct TlsInitError(pub String);

#[cfg(target_os = "android")]
impl From<jni::errors::Error> for TlsInitError {
    fn from(error: jni::errors::Error) -> Self {
        TlsInitError(error.to_string())
    }
}

/// The JVM handshake, Android only.
#[cfg(target_os = "android")]
pub mod android {
    use jni::objects::JObject;
    use jni::Env;

    use super::{mark_initialized, TlsInitError};

    /// Give `rustls-platform-verifier` the JVM and the app's `Context` it verifies certificates
    /// through, and mark this process's TLS ready. Call once, at startup, before the first request;
    /// a second call is harmless (the verifier keeps the first handles it was given).
    ///
    /// `context` should be the application context, which lives as long as the process.
    pub fn init(env: &mut Env<'_>, context: JObject<'_>) -> Result<(), TlsInitError> {
        rustls_platform_verifier::android::init_with_env(env, context)?;
        mark_initialized();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_android_waits_for_an_init_awtd_1673() {
        assert_eq!(NEEDS_PLATFORM_INIT, cfg!(target_os = "android"));
        if !NEEDS_PLATFORM_INIT {
            // The desktop transports must not start refusing requests because of this module.
            assert!(platform_tls_ready());
        }
    }
}
