//! Engine internals: identity, persistence, and the local event log.
//!
//! This crate deliberately knows nothing about UI or Reticulum wire formats.
//! It stores conversations and messages, and hands the daemon the data it needs
//! to route.

pub mod identity;
pub mod store;

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
