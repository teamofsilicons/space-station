//! Access lists: `["@alice", "@bot:tos", "tech"]`. `@` names an actor, `webhook:` an org webhook
//! (recipients only), anything else is a tag matched exactly. Whoever creates or edits something
//! is on its list, so every check is one membership test and nothing can be left unreachable.
//! `visible_tables` is that test over an org's tables, cached per identity for 60 s.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde_json::Value;

use crate::http::{ApiError, AppState};
use crate::iam::{CACHE_TTL, Identity};
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

/// Does `list` name this identity or one of its tags?
pub fn matches(identity: &Identity, list: &[String]) -> bool {
    list.iter().any(|entry| match Entry::parse(entry) {
        Entry::Actor(actor) => actor == identity.id,
        Entry::Tag(tag) => identity.tags.iter().any(|mine| mine == tag),
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
    let bad =
        |e: &&String| e.len() > 100 || matches!(Entry::parse(e), Entry::Webhook(_) | Entry::Actor("") | Entry::Tag(""));
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

/// The org's tables this identity may read, sorted; one Postgres read per identity per minute.
pub async fn visible_tables(state: &AppState, identity: &Identity) -> Result<Vec<String>, ApiError> {
    let key = (identity.org.clone(), identity.id.clone());
    if let Some((tables, at)) = lock(&state.visible.0).get(&key)
        && at.elapsed() < CACHE_TTL
    {
        return Ok(tables.clone());
    }
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT id, access FROM tables WHERE org = $1 ORDER BY id")
        .bind(&identity.org)
        .fetch_all(&state.store.pg)
        .await?;
    let tables: Vec<String> =
        rows.into_iter().filter(|(_, access)| matches(identity, &list(access))).map(|(id, _)| id).collect();
    lock(&state.visible.0).insert(key, (tables.clone(), Instant::now()));
    Ok(tables)
}

/// A table was created or its access changed: the org's cached visibility is stale.
pub fn forget(state: &AppState, org: &str) {
    lock(&state.visible.0).retain(|(o, _), _| o != org);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iam::Kind;

    fn alice() -> Identity {
        Identity { kind: Kind::Carbon, id: "alice".into(), org: "tos".into(), tags: vec!["tech".into()] }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn entries_parse_by_prefix() {
        assert_eq!(Entry::parse("@alice"), Entry::Actor("alice"));
        assert_eq!(Entry::parse("@bot:tos"), Entry::Actor("bot:tos"));
        assert_eq!(Entry::parse("webhook:abc"), Entry::Webhook("abc"));
        assert_eq!(Entry::parse("tech"), Entry::Tag("tech"));
    }

    #[test]
    fn an_actor_matches_by_id_or_tag_and_never_by_webhook() {
        assert!(matches(&alice(), &strings(&["@alice"])));
        assert!(matches(&alice(), &strings(&["@bob", "tech"])));
        assert!(!matches(&alice(), &strings(&["@bob", "ops", "Tech"])), "tags are case-sensitive");
        assert!(!matches(&alice(), &strings(&["webhook:alice", "alice"])));
        assert!(!matches(&alice(), &[]));
    }

    #[test]
    fn the_writing_actor_is_appended_once() {
        assert_eq!(with_actor(strings(&["ops"]), "alice"), strings(&["ops", "@alice"]));
        assert_eq!(with_actor(strings(&["@alice", "ops"]), "alice"), strings(&["@alice", "ops"]));
    }

    #[test]
    fn webhooks_and_empty_names_are_not_access_entries() {
        assert!(validate(&strings(&["@alice", "tech"])).is_ok());
        assert_eq!(validate(&strings(&["webhook:x"])).unwrap_err().code, "invalid_access");
        assert_eq!(validate(&strings(&["@"])).unwrap_err().code, "invalid_access");
        assert_eq!(validate(&strings(&[""])).unwrap_err().code, "invalid_access");
    }
}
