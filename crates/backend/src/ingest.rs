//! `/ws/ingest`: the daemon's batches become staged rows. A key resolves through a 60 s map to
//! its (org, table); every record is bounded and parsed; one Lua `EVAL` per batch dedups and
//! stages the survivors; the ack goes out only after Redis said yes. A store error closes the
//! socket without an ack, so the daemon's resend is the retry.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::any;
use redis::Script;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use space_station_shared::limits::{BATCH_MAX, DEDUP_TTL_SECS, RECORD_WIRE_MAX};
use space_station_shared::secrets::sha256_hex;
use space_station_shared::wire::{Ack, Code, Rejection};
use sqlx::PgPool;
use uuid::Uuid;

use crate::http::AppState;
use crate::iam::CACHE_TTL;
use crate::lock;

/// One `EVAL` per batch. ARGV is `record_id, row, record_id, row, …`; the reply lists the ids seen
/// within `DEDUP_TTL_SECS`, everything else is now on `staging`.
pub static LUA: LazyLock<String> = LazyLock::new(|| {
    format!(
        "local rejected = {{}} \
         for i = 1, #ARGV, 2 do \
           if redis.call('SET', 'dedup:' .. ARGV[i], '1', 'NX', 'EX', {DEDUP_TTL_SECS}) then \
             redis.call('RPUSH', 'staging', ARGV[i + 1]) \
           else rejected[#rejected + 1] = ARGV[i] end \
         end \
         return rejected"
    )
});
static SCRIPT: LazyLock<Script> = LazyLock::new(|| Script::new(&LUA));

/// `sha256(key) → (org, table)`, misses included, each entry refreshed from Postgres after 60 s.
#[derive(Default)]
pub struct Keys(Mutex<HashMap<String, (Resolved, Instant)>>);

type Resolved = Option<(String, String)>;

impl Keys {
    async fn resolve(&self, pg: &PgPool, key: &str) -> Result<Resolved, sqlx::Error> {
        let hash = sha256_hex(key);
        if let Some((found, at)) = lock(&self.0).get(&hash)
            && at.elapsed() < CACHE_TTL
        {
            return Ok(found.clone());
        }
        let found =
            sqlx::query_as("SELECT org, id FROM tables WHERE key_hash = $1").bind(&hash).fetch_optional(pg).await?;
        let mut keys = lock(&self.0);
        if keys.len() > 10_000 {
            keys.retain(|_, (_, at)| at.elapsed() < CACHE_TTL);
        }
        keys.insert(hash, (found.clone(), Instant::now()));
        Ok(found)
    }

    /// A key was rotated: its hash must stop resolving now, not in a minute.
    pub fn forget(&self, hash: &str) {
        lock(&self.0).remove(hash);
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/ingest", any(upgrade))
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.max_message_size(2 * BATCH_MAX).on_upgrade(move |socket| session(socket, state))
}

async fn session(mut socket: WebSocket, state: AppState) {
    let mut stop = state.stop.clone();
    loop {
        let message = tokio::select! { m = socket.recv() => m, _ = stop.changed() => break };
        let text = match message {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
            Some(Ok(_)) => continue,
        };
        let (ack, close) = match handle(&state, &text).await {
            Ok(ack) => (ack, false),
            Err(Fault::TooLarge(ack)) => (ack, true),
            Err(Fault::Malformed) => break,
            Err(Fault::Store(e)) => {
                tracing::warn!("ingest batch dropped, closing without an ack: {e}");
                break;
            }
        };
        let frame = serde_json::to_string(&ack).unwrap_or_default();
        if socket.send(Message::Text(frame.into())).await.is_err() || close {
            break;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

enum Fault {
    TooLarge(Ack),
    Malformed,
    Store(String),
}

#[derive(Deserialize)]
struct Batch<'a> {
    batch_id: Uuid,
    #[serde(borrow)]
    records: Vec<Entry<'a>>,
}

#[derive(Deserialize)]
struct Entry<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
    #[serde(borrow)]
    metadata: &'a RawValue,
    #[serde(borrow)]
    record: &'a RawValue,
}

#[derive(Deserialize)]
struct Meta {
    record_id: Uuid,
    event_ts_ms: i64,
}

/// A line of `staging`.
#[derive(Serialize)]
struct Staged<'a> {
    org: &'a str,
    table: &'a str,
    record_id: Uuid,
    event_ts_ms: i64,
    metadata: &'a RawValue,
    record: &'a RawValue,
}

fn reject(record_id: Uuid, code: Code, reason: impl Into<String>) -> Rejection {
    Rejection { record_id, code, reason: reason.into() }
}

/// The ack for one frame.
async fn handle(state: &AppState, text: &str) -> Result<Ack, Fault> {
    if text.len() > BATCH_MAX {
        #[derive(Deserialize)]
        struct Head {
            batch_id: Uuid,
        }
        let id = serde_json::from_str::<Head>(text).map(|h| h.batch_id).unwrap_or_default();
        return Err(Fault::TooLarge(Ack::batch_too_large(id)));
    }
    let batch: Batch = serde_json::from_str(text).map_err(|_| Fault::Malformed)?;
    let (mut rejected, mut args, mut bytes) = (Vec::new(), Vec::with_capacity(batch.records.len() * 2), 0);
    for entry in &batch.records {
        let meta: Meta = match serde_json::from_str(entry.metadata.get()) {
            Ok(meta) => meta,
            Err(e) => {
                rejected.push(reject(Uuid::nil(), Code::Invalid, format!("metadata: {e}")));
                continue;
            }
        };
        let found = state.keys.resolve(&state.store.pg, &entry.key).await.map_err(|e| Fault::Store(e.to_string()))?;
        let Some((org, table)) = found else {
            rejected.push(reject(meta.record_id, Code::Unauthorized, "unknown table key"));
            continue;
        };
        let record = entry.record.get();
        if record.len() > RECORD_WIRE_MAX {
            let reason = format!("record is {} bytes, the limit is {RECORD_WIRE_MAX}", record.len());
            rejected.push(reject(meta.record_id, Code::SizeExceeded, reason));
            continue;
        }
        if !record.starts_with('{') {
            rejected.push(reject(meta.record_id, Code::Invalid, "record must be a JSON object"));
            continue;
        }
        let row = Staged {
            org: &org,
            table: &table,
            record_id: meta.record_id,
            event_ts_ms: meta.event_ts_ms,
            metadata: entry.metadata,
            record: entry.record,
        };
        let row = serde_json::to_string(&row).map_err(|_| Fault::Malformed)?;
        bytes += row.len();
        args.push(meta.record_id.to_string());
        args.push(row);
    }
    if !args.is_empty() {
        let mut invocation = SCRIPT.prepare_invoke();
        for arg in &args {
            invocation.arg(arg);
        }
        let mut redis = state.store.redis.clone();
        let duplicates: Vec<String> =
            invocation.invoke_async(&mut redis).await.map_err(|e| Fault::Store(e.to_string()))?;
        state.store.staged.add(bytes);
        let seen = duplicates.iter().filter_map(|id| id.parse().ok());
        rejected.extend(seen.map(|id| reject(id, Code::Duplicate, "seen in the last five minutes")));
    }
    Ok(if rejected.is_empty() { Ack::ok(batch.batch_id) } else { Ack::rejected(batch.batch_id, rejected) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_dedups_with_set_nx_ex_and_stages_with_rpush() {
        let lua = LUA.as_str();
        assert!(lua.contains("'SET', 'dedup:' .. ARGV[i], '1', 'NX', 'EX', 300"));
        assert!(lua.contains("'RPUSH', 'staging', ARGV[i + 1]"));
        assert!(lua.ends_with("return rejected"));
    }

    #[test]
    fn a_batch_borrows_its_records_without_reserialising_them() {
        let text = r#"{"batch_id":"11111111-1111-4111-8111-111111111111","records":[{"key":"table-orders-0123","metadata":{"record_id":"22222222-2222-4222-8222-222222222222","table_id":"orders","event_ts_ms":7},"record":{"a": 1}}]}"#;
        let batch: Batch = serde_json::from_str(text).unwrap();
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].record.get(), r#"{"a": 1}"#);
        let meta: Meta = serde_json::from_str(batch.records[0].metadata.get()).unwrap();
        assert_eq!(meta.event_ts_ms, 7);
    }
}
