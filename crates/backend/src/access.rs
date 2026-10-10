//! Account-owned visibility and legacy access-list serialization. Historical lists remain
//! readable, while notification delivery matches only an explicit Carbon or Silicon handle.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde_json::Value;

use crate::accounts::{CACHE_TTL, Identity, Kind};
use crate::http::{ApiError, AppState};
use crate::lock;

#[derive(Debug, PartialEq, Eq)]
pub enum Entry<'a> {
    Actor(&'a str),
    Webhook(&'a str),
    Tag(&'a str),
}

impl<'a> Entry<'a> {
    pub fn parse(s: &'a str) -> Entry<'a> {
        if let Some(actor) = s.strip_prefix('@') {
            Entry::Actor(actor)
        } else if let Some(id) = s.strip_prefix("webhook:") {
            Entry::Webhook(id)
        } else {
            Entry::Tag(s)
        }
    }
}

/// Does the recipient list name this account?
pub fn matches(identity: &Identity, list: &[String]) -> bool {
    list.iter().any(|entry| match Entry::parse(entry) {
        Entry::Actor(actor) => actor == identity.id,
        Entry::Tag(_) => false,
        Entry::Webhook(_) => false,
    })
}

/// `list` with `@actor` appended when absent: the actor writing an access list stays on it, so
/// no table, window or notification is ever left with a list nobody matches.
pub fn with_actor(mut list: Vec<String>, actor: &str) -> Vec<String> {
    let me = format!("@{actor}");
    if !list.contains(&me) {
        list.push(me);
    }
    list
}

/// An access list as a client sent it: short, non-empty entries, never `webhook:`.
pub fn validate(list: &[String]) -> Result<(), ApiError> {
    let bad = |e: &&String| {
        e.len() > 100
            || match Entry::parse(e) {
                Entry::Actor(actor) => Kind::of(actor).is_none(),
                Entry::Webhook(_) | Entry::Tag("") => true,
                Entry::Tag(_) => false,
            }
    };
    match list.iter().find(bad) {
        Some(entry) => Err(ApiError::bad_request("invalid_access", format!("{entry:?} is not a valid access entry"))),
        None => Ok(()),
    }
}

/// The strings of a stored jsonb list.
pub fn list(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// (org, actor) → visible table ids, and when they were read.
#[derive(Default)]
pub struct Cache(Mutex<HashMap<(String, String), Visible>>);

type Visible = (Vec<String>, Instant);

/// The account's tables this identity may read, sorted; one Postgres read per identity per minute.
pub async fn visible_tables(state: &AppState, identity: &Identity) -> Result<Vec<String>, ApiError> {
    let key = (identity.org.clone(), identity.id.clone());
    if let Some((tables, at)) = lock(&state.visible.0).get(&key)
        && at.elapsed() < CACHE_TTL
    {
        return Ok(tables.clone());
    }
    let tables: Vec<String> = sqlx::query_scalar("SELECT id FROM tables WHERE org = $1 ORDER BY id")
        .bind(&identity.org)
        .fetch_all(&state.store.pg)
        .await?;
    lock(&state.visible.0).insert(key, (tables.clone(), Instant::now()));
    Ok(tables)
}

/// A table was created or its access changed: the account's cached visibility is stale.
pub fn forget(state: &AppState, org: &str) {
    lock(&state.visible.0).retain(|(o, _), _| o != org);
}
