//! What every Space Station crate agrees on: the limits, the ingest wire format, the
//! shapes of secrets, and the sanitizer that keeps records inside those limits.
//!
//! Nothing here does I/O.

#![forbid(unsafe_code)]

pub mod limits;
pub mod sanitize;
pub mod secrets;
pub mod wire;
