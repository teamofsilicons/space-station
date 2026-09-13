//! Same-origin browser collection. Keys stay on the server; acknowledgements use normal ingest.

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use space_station_shared::secrets::parse_table_key;
use space_station_shared::wire::Code;
use uuid::Uuid;

use crate::config::Config;
use crate::http::auth::{bad_origin, origin_ok};
use crate::http::{ApiError, AppState, Json};
use crate::iam::session;

const MAX_EVENT_BYTES: usize = 64 * 1024;
const MAX_BATCH: usize = 40;

#[derive(Clone)]
pub struct Collector {
    analytics_key: Option<String>,
    events_key: Option<String>,
}

impl Collector {
    pub fn from_config(cfg: &Config) -> Self {
        Self { analytics_key: cfg.frontend_analytics_key.clone(), events_key: cfg.frontend_events_key.clone() }
    }

    fn batch(&self, body: Batch, server: Value) -> Result<Value, ApiError> {
        if body.events.is_empty() || body.events.len() > MAX_BATCH {
            return Err(ApiError::bad_request("invalid_batch", "send between 1 and 40 frontend events"));
        }
        let (kind, key) = [("analytics", &self.analytics_key), ("events", &self.events_key)]
            .into_iter()
            .find_map(|(kind, key)| {
                key.as_deref().filter(|key| parse_table_key(key) == Some(body.table.as_str())).map(|key| (kind, key))
            })
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    "collector_table_unavailable",
                    "this frontend table is not configured or telemetry is disabled",
                )
            })?;
        let mut records = Vec::with_capacity(body.events.len());
        for event in body.events {
            if event.kind.is_empty() || event.kind.len() > 160 || event.id.is_empty() || event.id.len() > 100 {
                return Err(ApiError::bad_request(
                    "invalid_event",
                    "event type must be 1-160 bytes and id must be 1-100 bytes",
                ));
            }
            // Namespace browser IDs by the secret table key so another public stream cannot dedup them.
            let digest = Sha256::digest(format!("{key}:{}", event.id));
            let record_id = Uuid::from_bytes(digest[..16].try_into().expect("SHA256 has 32 bytes"));
            let mut metadata = event.metadata.unwrap_or_default();
            let event_ts_ms = metadata
                .get("occurred_at")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());
            metadata.insert("server".into(), server.clone());
            let record = json!({"type": kind, "event": event.kind, "data": event.data, "metadata": metadata});
            if record.to_string().len() > MAX_EVENT_BYTES {
                return Err(ApiError::bad_request("event_too_large", "frontend telemetry events must be under 64 KiB"));
            }
            records.push(json!({"key": key, "metadata": {"record_id": record_id, "table_id": body.table, "event_ts_ms": event_ts_ms}, "record": record}));
        }
        // Validate the complete batch before staging any of it.
        Ok(json!({"batch_id": Uuid::new_v4(), "records": records}))
    }
}

#[derive(Deserialize)]
struct Batch {
    table: String,
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    // Compatibility with older clients which did not supply an ID.
    #[serde(default = "new_id")]
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    metadata: Option<Map<String, Value>>,
}
fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/web/telemetry", post(record))
}

fn request_context(headers: &HeaderMap) -> Value {
    let mut server = json!({"source": "spacestation_frontend", "received_at": Utc::now(), "client_reported": true});
    if let Some(ua) = headers.get("user-agent").and_then(|v| v.to_str().ok()) {
        server["user_agent"] = json!(ua);
    }
    if let Some(mut url) = headers.get("referer").and_then(|v| v.to_str().ok()).and_then(|s| url::Url::parse(s).ok()) {
        url.set_query(None);
        url.set_fragment(None);
        let _ = url.set_username("");
        let _ = url.set_password(None);
        server["referrer"] = json!(url.as_str());
    }
    // These headers are observations, not proof of identity or geography.
    for (header, field) in [
        ("x-forwarded-for", "forwarded_ip_reported"),
        ("x-vercel-ip-country", "country_reported"),
        ("x-vercel-ip-city", "city_reported"),
    ] {
        if let Some(value) = headers.get(header).and_then(|v| v.to_str().ok()) {
            server[field] = json!(value.split(',').next().unwrap_or("").trim());
        }
    }
    server
}

async fn record(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Batch>,
) -> Result<StatusCode, ApiError> {
    if !origin_ok(&headers, &state.cfg.origin) {
        return Err(bad_origin());
    }
    let mut server = request_context(&headers);
    // Public pages work too; only a verified session supplies actor attribution.
    if let Some(cookie) = session::cookie_value(&headers)
        && let Some(session) = session::load(&state, &cookie).await?
    {
        server["actor"] = json!(session.actor);
        server["org"] = json!(session.org);
        server["kind"] = json!(session.kind);
    }
    let batch = state.frontend.batch(body, server)?;
    // ponytail: one ceiling for this site's public collector; partition it if multi-site collection is needed.
    let mut redis = state.store.redis.clone();
    let count: u64 = redis::Script::new("local n = redis.call('INCRBY', KEYS[1], ARGV[1]); if n == tonumber(ARGV[1]) then redis.call('EXPIRE', KEYS[1], 60) end; return n")
        .key("frontend:events_per_minute").arg(batch["records"].as_array().unwrap().len()).invoke_async(&mut redis).await?;
    if count > 6000 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "collector_rate_limit",
            "frontend collector limit is 6000 events per minute; retry after 60 seconds",
        ));
    }
    let ack = crate::ingest::accept(&state, &batch.to_string()).await?;
    if let Some(rejection) = ack.rejected.iter().find(|r| r.code != Code::Duplicate) {
        return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "telemetry_rejected", &rejection.reason));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_validate_before_staging_keep_retry_ids_and_strip_referrer_secrets() {
        let collector = Collector {
            analytics_key: Some("table-analytics-0123456789abcdef0123456789abcdef".into()),
            events_key: None,
        };
        let body = json!({"table":"analytics", "events":[{"id":"retry-id", "type":"page_view", "metadata":{"occurred_at":"2026-09-13T00:00:00Z"}}]});
        let one = collector.batch(serde_json::from_value(body.clone()).unwrap(), json!({})).unwrap();
        let two = collector.batch(serde_json::from_value(body.clone()).unwrap(), json!({})).unwrap();
        assert_eq!(one["records"][0]["metadata"], two["records"][0]["metadata"]);
        let mut bad = body;
        bad["events"].as_array_mut().unwrap().push(json!({"type":"", "id":"bad"}));
        assert!(collector.batch(serde_json::from_value(bad).unwrap(), json!({})).is_err());
        let mut headers = HeaderMap::new();
        headers.insert("referer", "https://user:secret@example.com/path?token=secret#secret".parse().unwrap());
        assert_eq!(request_context(&headers)["referrer"], "https://example.com/path");
    }
}
