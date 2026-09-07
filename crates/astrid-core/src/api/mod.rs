//! Everything that speaks HTTP to the Astrid backend.
//!
//! This is the only module allowed to make a request. Services call it; the shell never sees it.
//! All paths are `/api/v1/...`.

pub mod platform;
