//! Every number from `UNDERSTANDING.md`, in one place. Sizes are UTF-8 byte lengths of the
//! JSON text as sent; nobody re-serialises to decide.

/// A string value larger than this is cut in the middle.
pub const VALUE_MAX: usize = 32 * 1024;
/// A record larger than this is rejected by the library before it leaves the process.
pub const RECORD_MAX: usize = 256 * 1024;
/// What the server still accepts for one record object: `RECORD_MAX` plus room for the
/// sanitizer's markers. Anything above is `size_exceeded`.
pub const RECORD_WIRE_MAX: usize = 264 * 1024;
/// One ingest frame (a batch).
pub const BATCH_MAX: usize = 8 * 1024 * 1024;
/// The processor's output, and any tool result.
pub const SILICON_JSON_MAX: usize = 64 * 1024;
/// The flusher drains when either is reached.
pub const FLUSH_BYTES: usize = 16 * 1024 * 1024;
pub const FLUSH_INTERVAL_MS: u64 = 1_000;
/// A record id seen within this window is a duplicate.
pub const DEDUP_TTL_SECS: u64 = 300;
/// `onTrigger`, `init` queries and tools get this long.
pub const TRIGGER_TIMEOUT_MS: u64 = 10_000;
/// A window with no runner heartbeat for this long is inactive.
pub const WINDOW_IDLE_MS: u64 = 600_000;
/// A fresh cursor counter starts this far above the highest cursor ClickHouse knows.
pub const CURSOR_BOOT_SHIFT: u64 = 10_000;
/// The daemon truncates a fully acked spool once it is bigger than this.
pub const SPOOL_TRUNCATE_BYTES: u64 = 64 * 1024 * 1024;
/// Records waiting in the library before they reach the daemon socket.
pub const QUEUE_CAPACITY: usize = 10_000;
/// Space window names are "under 20 characters".
pub const WINDOW_NAME_MAX: usize = 19;
/// Notification `dedup_key`.
pub const DEDUP_KEY_MAX: usize = 256;
/// Access tokens go stale for `is_live` after this.
pub const LIVE_WINDOW_MS: u64 = 30_000;
