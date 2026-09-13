//! Authenticated browser analytics and event collection.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use space_station::SpaceClient;

use crate::config::Config;
use crate::http::auth::Auth;
use crate::http::{ApiError, AppState, Json};

const MAX_EVENT_BYTES: usize = 64 * 1024;
const MAX_BATCH: usize = 40;

#[derive(Clone)]
pub struct Collector {
    analytics: Option<Arc<SpaceClient>>,
    events: Option<Arc<SpaceClient>>,
    analytics_table: Option<String>,
    events_table: Option<String>,
}

impl Collector {
    pub fn from_config(cfg: &Config) -> Result<Self, space_station::Error> {
        let build = |key: &Option<String>| {
            key.as_deref()
                .map(|key| {
                    SpaceClient::builder(key)
                        .home(cfg.telemetry_home.clone())
                        .url(cfg.telemetry_url.clone())
                        .flush_timeout(Duration::from_millis(100))
                        .on_error(|_| {})
                        .build()
                        .map(Arc::new)
                })
                .transpose()
        };
        let table = |key: &Option<String>| {
            key.as_deref().and_then(space_station_shared::secrets::parse_table_key).map(str::to_owned)
        };
        Ok(Self {
            analytics: build(&cfg.frontend_analytics_key)?,
            events: build(&cfg.frontend_events_key)?,
            analytics_table: table(&cfg.frontend_analytics_key),
            events_table: table(&cfg.frontend_events_key),
        })
    }

    fn record(
        &self,
        requested: &str,
        events: Vec<Event>,
        actor: &crate::iam::Identity,
        headers: &HeaderMap,
    ) -> Result<(), ApiError> {
        let (expected, client) = if self.analytics_table.as_deref() == Some(requested) {
            ("analytics", self.analytics.as_ref())
        } else if self.events_table.as_deref() == Some(requested) {
            ("events", self.events.as_ref())
        } else {
            return Err(ApiError::forbidden());
        };
        let client = client.ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "collector_disabled",
                format!("frontend {expected} telemetry is not configured"),
            )
        })?;
        if events.is_empty() || events.len() > MAX_BATCH {
            return Err(ApiError::bad_request("invalid_batch", "send between 1 and 40 frontend events"));
        }
        for event in events {
            if event.kind.is_empty() || event.kind.len() > 100 {
                return Err(ApiError::bad_request("invalid_event", "event type must be 1-100 bytes"));
            }
            let mut metadata = event.metadata.unwrap_or_default();
            metadata.insert("actor".into(), json!(actor.id));
            metadata.insert("kind".into(), json!(format!("{:?}", actor.kind).to_ascii_lowercase()));
            metadata.insert("org".into(), json!(actor.org));
            if let Some(value) = headers.get("user-agent").and_then(|v| v.to_str().ok()) {
                metadata.insert("user_agent".into(), json!(value));
            }
            if let Some(value) = headers.get("referer").and_then(|v| v.to_str().ok()) {
                metadata.insert("referer".into(), json!(value));
            }
            if let Some(value) =
                headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next())
            {
                metadata.insert("ip".into(), json!(value.trim()));
            }
            let record = json!({"type": expected, "event": event.kind, "data": event.data, "metadata": metadata});
            if serde_json::to_vec(&record).map_err(|e| ApiError::internal("frontend_telemetry", e))?.len()
                > MAX_EVENT_BYTES
            {
                return Err(ApiError::bad_request("event_too_large", "frontend telemetry events must be under 64 KiB"));
            }
            client.record(record);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Batch {
    table: String,
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    metadata: Option<Map<String, Value>>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/web/telemetry", post(record))
}

async fn record(
    State(state): State<AppState>,
    auth: Auth,
    headers: HeaderMap,
    Json(body): Json<Batch>,
) -> Result<StatusCode, ApiError> {
    let actor = auth.actor()?;
    state.frontend.record(&body.table, body.events, actor, &headers)?;
    Ok(StatusCode::NO_CONTENT)
}
