//! The ingest protocol between a daemon and the backend: one JSON text frame per batch, one
//! JSON text frame per ack.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// `→ {"batch_id", "records": [...]}` — the oldest unacked records regardless of key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch {
    pub batch_id: Uuid,
    pub records: Vec<Entry>,
}

/// One record on the wire. `key` is the table key it was recorded under; the server resolves
/// it and ignores `metadata.table_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub metadata: Metadata,
    pub record: Value,
}

/// What the library stamps on every record. Everything after `event_ts_ms` is sampled by the
/// daemon at send time and is `None` when it could not be measured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub record_id: Uuid,
    pub table_id: String,
    pub event_ts_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<System>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free_mb: Option<u64>,
}

/// Cached once per machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct System {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub cpu: String,
    pub cores: u32,
    pub ram_mb: u64,
}

/// `← {"batch_id", "status": "ok"}` or `{"batch_id", "status": "rejected", "rejected": [...]}`.
/// Records not listed in `rejected` were accepted. A frame over `BATCH_MAX` gets
/// `status: rejected, code: batch_too_large` and the socket is closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ack {
    pub batch_id: Uuid,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<Rejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Code>,
}

impl Ack {
    pub fn ok(batch_id: Uuid) -> Self {
        Self { batch_id, status: Status::Ok, rejected: Vec::new(), code: None }
    }
    pub fn rejected(batch_id: Uuid, rejected: Vec<Rejection>) -> Self {
        Self { batch_id, status: Status::Rejected, rejected, code: None }
    }
    pub fn batch_too_large(batch_id: Uuid) -> Self {
        Self { batch_id, status: Status::Rejected, rejected: Vec::new(), code: Some(Code::BatchTooLarge) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rejection {
    pub record_id: Uuid,
    pub code: Code,
    pub reason: String,
}

/// Branch on this, never on `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The key resolves to nothing; every record under it.
    Unauthorized,
    /// Seen within the last five minutes. The daemon treats this as accepted.
    Duplicate,
    /// The record object is over `RECORD_WIRE_MAX` bytes.
    SizeExceeded,
    /// Not a JSON object, or otherwise unparseable.
    Invalid,
    /// The whole frame was over `BATCH_MAX`.
    BatchTooLarge,
}
