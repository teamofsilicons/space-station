//! Trigger fan-out. Subscriptions and notifications register `(org, table, where)` with a
//! channel; on every `Flushed` the registry probes each distinct registration once with
//! `trigger_sql` over `(from, to]` and sends a `Hit` to everyone registered for it. A `where`
//! that fails at run time is a `dev_errors` row for its org, at most once a minute.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::UnboundedSender;

use crate::http::AppState;
use crate::store::{ChError, Flushed};
use crate::{dev_errors, lock, sql};

/// A flush `(from, to]` of `table` held a row matching `where_`.
#[derive(Debug, Clone)]
pub struct Hit {
    pub org: String,
    pub table: String,
    pub where_: Option<String>,
    pub from: u64,
    pub to: u64,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct Key {
    org: String,
    table: String,
    where_: Option<String>,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
    next: AtomicU64,
}

#[derive(Default)]
struct Inner {
    subs: HashMap<Key, Vec<(u64, UnboundedSender<Hit>)>>,
    /// When a `where` was last reported to `dev_errors`.
    reported: HashMap<Key, Instant>,
}

/// A live registration; dropping it unregisters.
pub struct Registration {
    registry: Arc<Registry>,
    key: Key,
    id: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut inner = lock(&self.registry.inner);
        if let Some(senders) = inner.subs.get_mut(&self.key) {
            senders.retain(|(id, _)| *id != self.id);
            if senders.is_empty() {
                inner.subs.remove(&self.key);
            }
        }
    }
}

impl Registry {
    pub fn register(
        self: &Arc<Self>,
        org: &str,
        table: &str,
        where_: Option<&str>,
        hit: UnboundedSender<Hit>,
    ) -> Registration {
        let key = Key { org: org.to_owned(), table: table.to_owned(), where_: where_.map(str::to_owned) };
        let id = self.next.fetch_add(1, Relaxed);
        lock(&self.inner).subs.entry(key.clone()).or_default().push((id, hit));
        Registration { registry: self.clone(), key, id }
    }
}

/// Runs the fan-out on the store's `Flushed` stream for the life of the process.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut flushes = state.store.flushed.subscribe();
        loop {
            let flushed = match flushes.recv().await {
                Ok(flushed) => flushed,
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("trigger fan-out skipped {n} flushes");
                    continue;
                }
                Err(RecvError::Closed) => return,
            };
            let keys: Vec<Key> = {
                let inner = lock(&state.triggers.inner);
                inner.subs.keys().filter(|k| k.org == flushed.org && k.table == flushed.table).cloned().collect()
            };
            join_all(keys.into_iter().map(|key| probe(&state, key, &flushed))).await;
        }
    });
}

async fn probe(state: &AppState, key: Key, flushed: &Flushed) {
    let sql = match sql::trigger_sql(&key.org, &key.table, key.where_.as_deref(), flushed.from, flushed.to) {
        Ok(sql) => sql,
        Err(e) => return report(state, &key, &e.to_string()).await,
    };
    match state.store.ch.query_org(&key.org, &sql).await {
        Ok(rows) if rows.is_empty() => {}
        Ok(_) => {
            let hit = Hit {
                org: key.org.clone(),
                table: key.table.clone(),
                where_: key.where_.clone(),
                from: flushed.from,
                to: flushed.to,
            };
            let inner = lock(&state.triggers.inner);
            for (_, sender) in inner.subs.get(&key).into_iter().flatten() {
                let _ = sender.send(hit.clone());
            }
        }
        Err(ChError::Http { message, .. }) => report(state, &key, &message).await,
        Err(e) => tracing::warn!("trigger probe failed: {e}"),
    }
}

/// A `dev_errors` row for the org, once a minute per `where`.
async fn report(state: &AppState, key: &Key, message: &str) {
    {
        let mut inner = lock(&state.triggers.inner);
        if inner.reported.get(key).is_some_and(|at| at.elapsed() < Duration::from_secs(60)) {
            return;
        }
        inner.reported.retain(|_, at| at.elapsed() < Duration::from_secs(60));
        inner.reported.insert(key.clone(), Instant::now());
    }
    dev_errors::insert(&state.store, &key.org, "trigger", &key.table, message, json!({"where": key.where_})).await;
}
