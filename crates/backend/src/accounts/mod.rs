//! Silicon Accounts identities. UUIDs own data; public carbon/silicon handles are display names.
pub mod client;
pub mod session;
pub mod webhook;

use crate::http::{ApiError, AppState};
pub use client::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;
pub const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub kind: Kind,
    pub id: String,
    pub uuid: Uuid,
    /// Physical namespace retained for existing records. Never caller-selected.
    #[serde(skip)]
    pub org: String,
}

impl Identity {
    pub async fn resolve(state: &AppState, namespace: &str, _actor: &str) -> Result<Identity, ApiError> {
        let row: Option<(Uuid, String, String)> =
            sqlx::query_as("SELECT uuid, actor, kind FROM account_owners WHERE namespace = $1 AND active")
                .bind(namespace)
                .fetch_optional(&state.store.pg)
                .await?;
        let (uuid, id, kind) =
            row.ok_or_else(|| ApiError::unauthorized("invalid_token", "this credential has no active account owner"))?;
        let kind = Kind::of(&id)
            .filter(|k| k.as_str() == kind)
            .ok_or_else(|| ApiError::unauthorized("invalid_token", "invalid account owner"))?;
        Ok(Identity { uuid, kind, id, org: namespace.to_owned() })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Carbon,
    Silicon,
}
impl Kind {
    pub fn of(actor: &str) -> Option<Kind> {
        let (kind, handle) = if let Some(handle) = actor.strip_prefix("c:") {
            (Kind::Carbon, handle)
        } else {
            (Kind::Silicon, actor.strip_prefix("si:")?)
        };
        ((!handle.is_empty())
            && handle.len() <= 80
            && handle.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-')))
        .then_some(kind)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Carbon => "carbon",
            Kind::Silicon => "silicon",
        }
    }
}
