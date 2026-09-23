//! Who someone is. A session proves an actor and one org — IAM's short-lived token, exchanged once
//! and re-proved by introspection — and the directory mirror supplies the tags: the introspection
//! snapshot writes it at login and on every re-proof, a webhook updates it in between, so the
//! mirror is always the freshest word IAM has had about a member. `Identity::resolve` builds one
//! for an (org, actor) from that mirror, the same way for a session and for an access token; until
//! IAM has first spoken about a member they hold only what their `@id` grants. `client` is the one
//! IAM client, `session` the rows behind cookies and `sscli-` bearers, `webhook` the receiver.

pub mod client;
pub mod session;
pub mod webhook;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use client::Client;

use crate::access;
use crate::http::{ApiError, AppState};

/// How long a visibility list and an ingest key resolution stay cached, and how often a socket
/// re-validates its credential.
pub const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub kind: Kind,
    pub id: String,
    pub org: String,
    pub tags: Vec<String>,
}

impl Identity {
    /// `actor` in `org` with the tags the mirror holds for an active membership: none until IAM
    /// has said otherwise. Membership itself is never read here; the session proved it.
    pub async fn resolve(state: &AppState, org: &str, actor: &str) -> Result<Identity, ApiError> {
        let tags = mirrored_tags(state, org, actor).await?.unwrap_or_default();
        let kind = Kind::of(actor).ok_or_else(|| {
            ApiError::unauthorized("identity_migration_required", "migrate stored identities before signing in")
        })?;
        Ok(Identity { kind, id: actor.to_owned(), org: org.to_owned(), tags })
    }
}

/// The tag names the mirror holds for an active membership of `actor` in `org`; `None` while it
/// holds nothing, which a session then fills from its own snapshot.
pub async fn mirrored_tags(state: &AppState, org: &str, actor: &str) -> Result<Option<Vec<String>>, ApiError> {
    let sql = "SELECT tags FROM iam_members WHERE org = $1 AND actor = $2 AND status = 'active'";
    let tags: Option<Value> = sqlx::query_scalar(sql).bind(org).bind(actor).fetch_optional(&state.store.pg).await?;
    Ok(tags.as_ref().map(access::list))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Carbon,
    Silicon,
}

impl Kind {
    /// Full IAM public identities. Organization authority is always supplied separately.
    pub fn of(actor: &str) -> Option<Kind> {
        let (kind, handle, max) = if let Some(handle) = actor.strip_prefix("c:") {
            (Kind::Carbon, handle, 30)
        } else {
            (Kind::Silicon, actor.strip_prefix("si:")?, 50)
        };
        ((3..=max).contains(&handle.len())
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

#[cfg(test)]
mod tests {
    use super::Kind;

    #[test]
    fn actor_namespaces_and_handle_limits_are_explicit() {
        assert_eq!(Kind::of("c:alice0"), Some(Kind::Carbon));
        assert_eq!(Kind::of("si:bot"), Some(Kind::Silicon));
        assert_eq!(Kind::of(&format!("c:{}", "a".repeat(30))), Some(Kind::Carbon));
        assert_eq!(Kind::of(&format!("si:{}", "a".repeat(50))), Some(Kind::Silicon));
        for id in ["alice", "bot:tos", "c:ab", "si:ab", "c:Alice", "si:bot:tos", "c:si:bot"] {
            assert_eq!(Kind::of(id), None, "{id}");
        }
        assert_eq!(Kind::of(&format!("c:{}", "a".repeat(31))), None);
        assert_eq!(Kind::of(&format!("si:{}", "a".repeat(51))), None);
    }
}
