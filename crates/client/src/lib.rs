//! Space Station, as a Rust crate. Everything Space Station can do is a method here; the
//! `spacestation` command is a shell over this crate and the web app is a subset of it.
//!
//! Two halves, one dependency:
//!
//! * **recording** — [`SpaceClient`] takes a table key and a JSON value per event ([`record`]),
//!   and a background [`daemon`] spools every line to disk and ships it over one WebSocket.
//! * **managing** — [`Auth`] proves who you are (a session, an access token, an API key) and
//!   [`Space`] is every HTTP call, typed, synchronous, one request per method.
//!
//! The managing half is stateless: `Auth` is a value the caller builds and `Space` is a function
//! of `(url, Auth)`. Nothing here talks to IAM: a person or a silicon obtains a *short-lived
//! token* from the `iam` CLI (or the browser), [`exchange`] hands it to the backend once, and the
//! backend is what holds and refreshes the session — so nothing on this side ever rotates, and a
//! 401 is final. The recording half keeps a spool on purpose, in the home it is given, and its
//! `flush` waits for the server's acks when this process is the one running the daemon, so a
//! one-shot program delivers before it exits. Neither half reads the environment or the home
//! directory on its own; [`default_url`] and [`default_home`] are here for a program that wants
//! those conventions.
//!
//! ```no_run
//! use space_station::{Auth, Space, SpaceClient};
//!
//! # fn main() -> Result<(), space_station::Error> {
//! let ss = SpaceClient::new("table-orders-0123456789abcdef0123456789abcdef")?;
//! ss.record(serde_json::json!({"id": "o-42", "amount": 12.5}));
//!
//! let url = space_station::default_url();
//! let auth = space_station::exchange(&url, "<what `iam silicon-login --app-id 'tos>spacestation'` printed>", "tos")?;
//! let space = Space::new(url, auth)?.org("tos");
//! let tables = space.tables()?;
//! let rows = space.query("SELECT count() FROM orders", &Default::default())?;
//! # Ok(()) }
//! ```
//!
//! [`record`]: SpaceClient::record

mod api;
mod auth;
pub mod daemon;
mod record;
mod spool;
#[cfg(test)]
mod tests;
mod types;
mod windows;

pub use space_station_shared as shared;

pub use api::Space;
pub use auth::{Auth, exchange, login};
pub use record::{Builder, SpaceClient};
pub use types::{
    AccessToken, ApiKey, Bounds, DaemonStatus, Def, DevError, Event, Identity, Key, Kind, Notification, Org, Overview,
    Restrict, Rows, StateMetadata, Table, TestRun, TopTable, Trigger, Version, Webhook, Window, WindowState,
};
pub use windows::WindowOutput;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use shared::limits::{QUEUE_CAPACITY, RECORD_MAX};
use shared::wire::Code;
use uuid::Uuid;

/// Where the daemon connects unless `$SPACE_STATION_URL` or `Builder::url` says otherwise.
pub const DEFAULT_URL: &str = "https://backend.spacestation.teamofsilicons.com";

/// `$SPACE_STATION_HOME`, else `~/.space-station`.
pub fn default_home() -> PathBuf {
    match std::env::var_os("SPACE_STATION_HOME") {
        Some(home) => home.into(),
        None => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".space-station"),
    }
}

/// `$SPACE_STATION_URL`, else `DEFAULT_URL`.
pub fn default_url() -> String {
    std::env::var("SPACE_STATION_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

/// Everything that can go wrong, whichever half you use. `Rejected` is the server's verdict on
/// one record and `Api` its verdict on one request: branch on `code`, never on the message. A
/// `status` of 401 means the credential itself was refused, and that is final: the backend holds
/// and refreshes the session, so nothing here can mend one — sign in again. A `status` of 0 is a
/// refusal relayed by the local window runtime (`window_tool`), which carries the code but not
/// the HTTP status.
#[derive(Debug)]
pub enum Error {
    InvalidKey,
    SizeExceeded {
        bytes: usize,
    },
    QueueFull,
    Rejected {
        record_id: Uuid,
        code: Code,
        reason: String,
    },
    Io(io::Error),
    Ws(String),
    /// Space Station could not be reached, or answered something that is not its API.
    Transport(String),
    /// It answered, and said no. `code` is snake_case and stable.
    Api {
        status: u16,
        code: String,
        message: String,
    },
    /// Nothing left the machine: no credential, no org, no `node`, a secret in the code.
    Local(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidKey => write!(f, "not a table key (expected table-<table_id>-<32 hex>)"),
            Error::SizeExceeded { bytes } => {
                write!(f, "record is {bytes} bytes, the limit is {RECORD_MAX}")
            }
            Error::QueueFull => write!(f, "{QUEUE_CAPACITY} records already wait for the daemon, record dropped"),
            Error::Rejected { record_id, code, reason } => {
                write!(f, "record {record_id} rejected as {code:?}: {reason}")
            }
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Ws(e) => write!(f, "websocket: {e}"),
            Error::Transport(e) => write!(f, "{e}"),
            Error::Api { code, message, .. } => write!(f, "{code}: {message}"),
            Error::Local(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub(crate) type Hook = Arc<dyn Fn(Error) + Send + Sync>;

/// Lock a mutex, recovering the data if a panicking thread poisoned it.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A fresh home for one test.
#[cfg(test)]
pub(crate) fn test_home() -> PathBuf {
    let home = std::env::temp_dir().join(format!("ss-{}", &Uuid::new_v4().simple().to_string()[..8]));
    std::fs::create_dir_all(&home).unwrap();
    home
}
