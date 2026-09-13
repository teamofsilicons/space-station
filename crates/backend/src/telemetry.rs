//! Backend self-observation through the ordinary `tos.spacestation` table.

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use space_station::Telemetry as ClientTelemetry;
use uuid::Uuid;

use crate::config::Config;

#[derive(Clone)]
pub struct Telemetry(Arc<ClientTelemetry>);

impl Telemetry {
    pub fn from_config(cfg: &Config) -> Result<Option<Self>, space_station::Error> {
        cfg.telemetry_key
            .as_deref()
            .map(|key| {
                ClientTelemetry::with_options(key, cfg.telemetry_home.clone(), cfg.telemetry_url.clone())
                    .map(|t| Self(Arc::new(t)))
            })
            .transpose()
    }

    pub fn record(&self, source: &str, step: &str, event: &str, context: serde_json::Value) {
        self.0.record(source, step, None, event, context);
    }

    pub fn request(&self, trace_id: Uuid, method: &str, path: &str, status: u16, started: Instant) {
        let event = if status >= 400 { "request_error" } else { "request_completed" };
        self.record(
            "backend",
            "http",
            event,
            json!({
                "trace_id": trace_id,
                "method": method,
                "path": path,
                "status": status,
                "duration_ms": started.elapsed().as_millis() as u64,
            }),
        );
    }
}
