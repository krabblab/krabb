//! Session limits shared by the daemon and standalone clients.

/// Bounds the daemon's stored history; tool calls and results count as messages. Context
/// compaction follows the model's token budget, so this only caps storage.
pub const MESSAGE_HISTORY_LIMIT: usize = 1_000;
pub const RESERVATION_TTL_SECONDS: u64 = 120;
