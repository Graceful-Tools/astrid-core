//! Everything that speaks HTTP to the Astrid backend.
//!
//! This is the only module allowed to make a request. Services call it; the shell never sees it.
//! All paths are `/api/v1/...`.

pub mod client;
pub mod pagination;
pub mod path;
pub mod platform;
pub mod transport;

pub use client::{ApiClient, ApiError, Request, API_PREFIX, DEFAULT_BASE_URL};
pub use pagination::{fetch_all, Page};
pub use transport::{
    HttpRequest, HttpResponse, HttpTransport, Method, ReqwestTransport, StubTransport,
    TransportError,
};
