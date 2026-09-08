//! What [`Space`](crate::Space) hands back. These are the public API: every field the server
//! sends and nothing invented, `Serialize` so a caller can print the JSON without a second shape.
//!
//! Timestamps are RFC 3339 strings exactly as the server wrote them, and ids are the strings it
//! gave, so nothing here needs a date or a uuid crate to be read.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A carbon or a silicon, inside one org: the public id people know it by (stored and sent
/// without the `@` a UI prepends) and the tag names the directory mirror holds for it there. No
/// display name: the id is the handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub kind: Kind,
    pub id: String,
    pub org: String,
    pub tags: Vec<String>,
}

/// A human, or a machine. The colon in a silicon's id (`bot:tos`) is what tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Carbon,
    Silicon,
}

/// An IAM organization, as the directory mirror knows it. Everything else is scoped to one; the
/// name is there once IAM has told the mirror about the org.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Org {
    pub id: String,
    pub name: Option<String>,
}

/// A logical table inside an org: records go in through its key, queries read it by id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub id: String,
    pub records: u64,
    /// The highest cursor a query on this table can currently see.
    pub watermark: u64,
    pub access: Vec<String>,
    pub created_by: String,
    pub created_at: String,
}

/// The Tables tab in one value: how much is stored, what is busiest over the asked window, and
/// how long records take to travel from `event_ts_ms` to `registered_ts_ms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overview {
    pub tables: u64,
    pub records: u64,
    pub top: Vec<TopTable>,
    pub avg_lag_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopTable {
    pub id: String,
    pub records: u64,
}

/// A space window: processor → SiliconJSON → renderer. `version` is the published pair it runs,
/// absent until the first `publish`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub id: String,
    pub name: String,
    pub access: Vec<String>,
    pub created_by: String,
    pub created_at: String,
    pub version: Option<Version>,
}

/// One named pair of processor and renderer code. A window carries only the three fields that
/// make it run; the version list carries who published it and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub processor: String,
    pub renderer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// The last SiliconJSON a runner published for a window, and how fresh it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub json: Option<Value>,
    pub metadata: StateMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMetadata {
    pub processor_version: Option<String>,
    pub renderer_version: Option<String>,
    pub produced_at: Option<String>,
    pub is_live: bool,
}

/// A notification: its current definition, who is subscribed, and who made it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub def: Def,
    pub recipients: Vec<String>,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: String,
}

/// What a notification does. This is what `create_notification` takes and what a definition file
/// holds; `delay` and `cooldown` are `^[0-9]+(ms|s|m|h|d)$`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Def {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub triggers: Vec<Trigger>,
    pub sql: String,
    #[serde(default = "default_delay")]
    pub delay: String,
    #[serde(default = "default_cooldown")]
    pub cooldown: String,
    #[serde(default)]
    pub access: Vec<String>,
}

/// A row landing in a table, or the clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Trigger {
    Table {
        table: String,
        #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
        where_: Option<String>,
    },
    /// Five-field cron, UTC.
    Schedule { schedule: String },
}

fn yes() -> bool {
    true
}

fn default_delay() -> String {
    "2s".into()
}

fn default_cooldown() -> String {
    "10m".into()
}

/// One fired notification. Append only: storing it is what "read" means.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub dedup_key: String,
    pub text: String,
    pub metadata: Value,
    pub created_at: String,
}

/// What a notification's SQL returns right now, without advancing its cursors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRun {
    pub rows: Vec<Value>,
    #[serde(default)]
    pub error: Option<String>,
    pub last_trigger_at: Option<String>,
}

/// An org webhook a notification can deliver to. The signing secret is shown once, on creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    pub created_by: String,
    pub created_at: String,
}

/// A programmatic key acting for the org inside its scopes (`tables`, `notifications`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub scopes: Vec<String>,
    pub created_by: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// A secret the server shows exactly once: a table key, an API key, a webhook signing secret.
/// Print it alone, hand it to a password manager, and never log it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Key {
    /// What it belongs to, when the server named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(alias = "key", alias = "secret")]
    pub value: String,
}

/// The access token processors and dev servers use. One per (org, actor), rotatable, and shown
/// as often as you like — unlike a `Key`, the server can still read this one back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub token: String,
    pub last_used_at: Option<String>,
}

/// Rows of a query, and the cursor each named table was read up to. `cursor`, `event_ts_ms` and
/// `registered_ts_ms` arrive as strings: they are 64-bit and JSON numbers are not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rows {
    pub rows: Vec<Value>,
    pub watermarks: BTreeMap<String, u64>,
}

/// `{table: {from?, to?}}`: the cursor range a query reads each table over. The server fills a
/// missing `to` with the table's watermark at query start and echoes what it used.
pub type Restrict = BTreeMap<String, Bounds>;

/// Exclusive `from`, inclusive `to`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Bounds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<u64>,
}

/// Something a processor, a renderer or a notification did wrong, recorded for its org.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevError {
    pub id: i64,
    pub source: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub message: String,
    pub detail: Value,
    pub created_at: String,
}

/// The local ingest daemon: whether one is listening on this machine's socket, how many records
/// are still waiting in the spool for the server to acknowledge them, and where both live — the
/// socket moves to the temp dir when the home is too long for a socket address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub unacked: u64,
    pub home: PathBuf,
    pub socket: PathBuf,
}
