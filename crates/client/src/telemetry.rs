//! Space Station's own telemetry stream.
//!
//! Telemetry is an ordinary record in the `tos` organization's `spacestation` table.  The
//! table key is supplied by deployment (the normal table key shown once by the CLI), so this
//! module has no privileged backend path and uses the same daemon and spool as every other app.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::{Error, SpaceClient, default_home};

/// The organization and table reserved for Space Station's own events.
pub const ORG: &str = "tos";
pub const TABLE: &str = "spacestation";
/// Environment variable containing the ordinary `tos.spacestation` table key.
pub const KEY_ENV: &str = "SPACE_STATION_TELEMETRY_KEY";
/// Set to `0`, `false`, or `off` to stop emitting telemetry.
pub const ENABLED_ENV: &str = "SPACE_STATION_TELEMETRY";

/// A sender for self-contained Space Station events.  It shares the regular daemon and is
/// intentionally stateless apart from that client's local spool.
pub struct Telemetry {
    client: SpaceClient,
}

impl Telemetry {
    /// Build a sender from the key created for `tos`/`spacestation`.
    pub fn new(table_key: &str) -> Result<Self, Error> {
        let table = crate::shared::secrets::parse_table_key(table_key).ok_or(Error::InvalidKey)?;
        if table != TABLE {
            return Err(Error::Local(format!("telemetry requires the {ORG}.{TABLE} table key, got {table}")));
        }
        Ok(Self { client: SpaceClient::new(table_key)? })
    }

    /// Load the key from [`KEY_ENV`] or `<home>/telemetry.key`.  Missing configuration disables
    /// telemetry rather than making the host application fail to start.  Opt-out is explicit via
    /// [`ENABLED_ENV`].
    pub fn from_env() -> Result<Option<Self>, Error> {
        if std::env::var(ENABLED_ENV)
            .ok()
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        {
            return Ok(None);
        }
        let key = std::env::var(KEY_ENV).ok().or_else(|| read_key(&default_home().join("telemetry.key")));
        key.filter(|key| !key.trim().is_empty()).map(|key| Self::new(key.trim())).transpose()
    }

    /// Queue one context-rich event.  The daemon supplies record and system metadata as usual.
    pub fn record<C: Serialize>(&self, source: &str, step: &str, progress: Option<f64>, event: &str, context: C) {
        self.client.record(TelemetryRecord { source, step, progress, event, context });
    }

    /// Wait for the normal daemon acknowledgement/spool handoff.
    pub fn flush(&self) -> bool {
        self.client.flush()
    }
}

fn read_key(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|key| key.trim().to_owned())
}

#[derive(Serialize)]
struct TelemetryRecord<'a, C> {
    source: &'a str,
    step: &'a str,
    progress: Option<f64>,
    event: &'a str,
    context: C,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_shape_is_self_contained() {
        let record = serde_json::to_value(TelemetryRecord {
            source: "daemon",
            step: "ship",
            progress: Some(0.5),
            event: "batch_sent",
            context: json!({"batch_id": "b1"}),
        })
        .unwrap();
        assert_eq!(record["source"], "daemon");
        assert_eq!(record["step"], "ship");
        assert_eq!(record["progress"], 0.5);
        assert_eq!(record["event"], "batch_sent");
        assert_eq!(record["context"]["batch_id"], "b1");
    }

    #[test]
    fn only_spacestation_keys_are_accepted() {
        let key = "table-orders-0123456789abcdef0123456789abcdef";
        assert!(matches!(Telemetry::new(key), Err(Error::Local(message)) if message.contains("tos.spacestation")));
    }
}
