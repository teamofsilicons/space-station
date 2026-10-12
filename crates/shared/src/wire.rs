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

/// The identifiers and timestamp needed to route and deduplicate a record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub record_id: Uuid,
    pub table_id: String,
    pub event_ts_ms: i64,
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

#[cfg(test)]
mod tests {
    use super::Entry;
    use serde_json::json;

    #[test]
    fn legacy_host_metadata_is_not_forwarded_but_explicit_record_data_is_preserved() {
        let metadata = json!({
            "record_id": "00000000-0000-0000-0000-000000000001",
            "table_id": "orders",
            "event_ts_ms": 1,
        });
        let record = json!({"order": 42, "system": "explicit caller data"});
        let mut legacy = json!({"key": "table-orders-test", "metadata": metadata, "record": record});
        legacy["metadata"]["system"] = json!({"hostname": "private-host", "os": "private-os"});
        legacy["metadata"]["cpu_pct"] = json!(12.5);
        legacy["metadata"]["gpu_pct"] = json!(8.0);
        legacy["metadata"]["ram_pct"] = json!(41.0);
        legacy["metadata"]["disk_free_mb"] = json!(120000);

        let entry: Entry = serde_json::from_value(legacy).unwrap();
        let forwarded = serde_json::to_value(entry).unwrap();

        assert_eq!(forwarded["metadata"], metadata);
        assert_eq!(forwarded["record"], record);
    }
}
