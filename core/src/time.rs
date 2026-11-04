//! Timestamps, as the space document specification defines them.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, in Coordinated Universal Time.
pub type Timestamp = i64;

/// The current time on this machine's clock.
pub fn now() -> Timestamp {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}
