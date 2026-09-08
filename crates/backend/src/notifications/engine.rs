//! The engine: one task per enabled notification, alive only while this process holds the engine
//! lease. A worker owns its trigger registrations, its timer and its cursors; a hit arms
//! `now + delay`, a hit during a run re-arms it, and the run happens inline, so a notification is
//! serial with itself. Saving, subscribing and taking the lease rebuild the fleet, and every
//! worker opens by asking what landed while it was away — which makes a rebuild lose nothing.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use space_station_shared::limits::DEDUP_KEY_MAX;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant, sleep_until};
use uuid::Uuid;

use super::cron::Cron;
use super::{Def, deliver, duration_ms};
use crate::http::AppState;
use crate::query::Restrict;
use crate::{dev_errors, query, sql};

/// A definition, or a subscription, changed here. The engine is one process (it runs under the
/// lease) and it owns the write path, so a `Notify` is the whole signal.
static CHANGED: Notify = Notify::const_new();
/// How long a worker with nothing armed sleeps before looking again.
const IDLE: Duration = Duration::from_secs(3600);

pub fn changed() {
    CHANGED.notify_one();
}

pub struct Engine;

impl Engine {
    /// Starts the supervisor; it does nothing until this process holds the lease.
    pub fn start(state: AppState) {
        tokio::spawn(supervise(state));
    }
}

/// The fleet exists exactly while the lease is held, and is rebuilt whenever a definition moves.
async fn supervise(state: AppState) {
    let (mut lease, mut stop) = (state.lease.clone(), state.stop.clone());
    let mut fleet = None;
    loop {
        match (lease.held(), fleet.is_some()) {
            (true, false) => fleet = Some(spawn_all(&state).await),
            (false, true) => stand_down(&mut fleet).await,
            _ => {}
        }
        tokio::select! {
            _ = lease.changed() => {}
            _ = CHANGED.notified() => stand_down(&mut fleet).await,
            _ = stop.changed() => break,
        }
    }
    stand_down(&mut fleet).await;
}

type Fleet = (watch::Sender<()>, Vec<JoinHandle<()>>);
/// One `notifications` row joined to its current definition, as Postgres hands it over.
type Loaded = (Uuid, String, Value, Value, Option<DateTime<Utc>>, Value);

/// Dropping the generation asks every worker to leave; they leave at the top of their loop, so a
/// run in flight finishes first and its cursors land.
async fn stand_down(fleet: &mut Option<Fleet>) {
    if let Some((generation, workers)) = fleet.take() {
        drop(generation);
        for worker in workers {
            let _ = worker.await;
        }
    }
}

/// One worker per enabled notification, in every org.
async fn spawn_all(state: &AppState) -> Fleet {
    let (generation, leaving) = watch::channel(());
    let sql = "SELECT n.id, n.org, n.recipients, n.cursors, n.last_cron_at, v.def FROM notifications n \
               JOIN notification_versions v ON v.id = n.current_version WHERE n.enabled";
    let rows: Vec<Loaded> = sqlx::query_as(sql).fetch_all(&state.store.pg).await.unwrap_or_else(|e| {
        tracing::error!("notifications could not be loaded: {e}");
        Vec::new()
    });
    let workers = rows
        .into_iter()
        .filter_map(|(id, org, recipients, cursors, last_cron_at, def)| {
            let job = Job {
                id,
                org,
                def: serde_json::from_value(def)
                    .inspect_err(|e| tracing::error!("notification {id} has an unreadable definition: {e}"))
                    .ok()?,
                recipients: crate::access::list(&recipients),
                cursors: serde_json::from_value(cursors).unwrap_or_default(),
                last_cron_at,
            };
            Some(tokio::spawn(work(state.clone(), job, leaving.clone())))
        })
        .collect();
    (generation, workers)
}

/// One notification as the engine holds it: the definition, who it goes to, and the two things
/// that move.
struct Job {
    id: Uuid,
    org: String,
    def: Def,
    recipients: Vec<String>,
    cursors: BTreeMap<String, u64>,
    last_cron_at: Option<DateTime<Utc>>,
}

async fn work(state: AppState, mut job: Job, mut leaving: watch::Receiver<()>) {
    let (hit_tx, mut hits) = mpsc::unbounded_channel();
    let _registered: Vec<_> =
        job.def.tables().map(|(t, w)| state.triggers.register(&job.org, t, w, hit_tx.clone())).collect();
    let delay = Duration::from_millis(duration_ms(&job.def.delay).unwrap_or(2_000));
    let mut armed = behind(&state, &job).await.then(|| Instant::now() + delay);
    let mut cron = next_cron(&job);
    loop {
        let cron_at = cron.map(|at| Instant::now() + (at - Utc::now()).to_std().unwrap_or_default());
        let wake = [armed, cron_at].into_iter().flatten().min().unwrap_or_else(|| Instant::now() + IDLE);
        tokio::select! {
            _ = leaving.changed() => return,
            Some(_) = hits.recv() => armed = armed.or_else(|| Some(Instant::now() + delay)),
            _ = sleep_until(wake) => {
                let due = |at: &Instant| *at <= Instant::now();
                let (scheduled, armed_now) = (cron_at.as_ref().is_some_and(due), armed.as_ref().is_some_and(due));
                if scheduled {
                    job.last_cron_at = Some(Utc::now());
                    cron = next_cron(&job);
                }
                if armed_now {
                    armed = None;
                }
                if scheduled || armed_now {
                    // A schedule reads the whole table; a table trigger reads only its delta.
                    run(&state, &mut job, scheduled).await;
                }
            }
        }
    }
}

/// Did anything the triggers care about land while this worker was away? The same probe the
/// trigger fan-out runs after a flush, over `(cursor, watermark]`.
async fn behind(state: &AppState, job: &Job) -> bool {
    for (table, where_) in job.def.tables() {
        let to = state.store.watermarks.get(&job.org, table).await.unwrap_or(0);
        let from = job.cursors.get(table).copied().unwrap_or(0);
        if to <= from {
            continue;
        }
        let Ok(sql) = sql::trigger_sql(&job.org, table, where_, from, to) else { continue };
        if state.store.ch.query_org(&job.org, &sql).await.is_ok_and(|rows| !rows.is_empty()) {
            return true;
        }
    }
    false
}

/// The next occurrence of any schedule after the last cron run. One that was missed while the
/// engine was away is already in the past, so the worker fires it once, at once.
fn next_cron(job: &Job) -> Option<DateTime<Utc>> {
    let after = job.last_cron_at.unwrap_or_else(Utc::now);
    job.def.schedules().filter_map(|s| Cron::parse(s)?.next_after(after)).min()
}

/// `{table: {from: cursor}}` for every table trigger: the delta rule, shared with `POST …/test`.
pub fn restrict(def: &Def, cursors: &BTreeMap<String, u64>) -> BTreeMap<String, Restrict> {
    def.tables().map(|(t, _)| (t.to_owned(), Restrict { from: cursors.get(t).copied(), to: None })).collect()
}

/// One run: the sql over the delta (or over everything, for a schedule), every good row through
/// the cooldown and out, then the cursors move to the watermarks the query echoed. A bad row is
/// a dev error and nothing else — the cursors still move, or every later run would meet it again.
async fn run(state: &AppState, job: &mut Job, whole: bool) {
    let restrict = if whole { BTreeMap::new() } else { restrict(&job.def, &job.cursors) };
    match query::run(state, &job.org, None, &job.def.sql, &restrict).await {
        Err(e) => report(state, job, &e.message, json!({"code": e.code})).await,
        Ok(result) => {
            for row in result["rows"].as_array().into_iter().flatten() {
                match event(row) {
                    Some((dedup_key, text, metadata)) => fire(state, job, dedup_key, text, metadata).await,
                    None => report(state, job, "a row is not {dedup_key, text, metadata}", row.clone()).await,
                }
            }
            let echoed = result["watermarks"].as_object().into_iter().flatten();
            job.cursors.extend(echoed.filter_map(|(table, to)| Some((table.clone(), to.as_u64()?))));
        }
    }
    let sql = "UPDATE notifications SET cursors = $2, last_cron_at = $3 WHERE id = $1";
    let saved = sqlx::query(sql).bind(job.id).bind(json!(job.cursors)).bind(job.last_cron_at);
    if let Err(e) = saved.execute(&state.store.pg).await {
        tracing::warn!("notification {} could not save its cursors: {e}", job.id);
    }
}

/// A row the sql returned, as an event: `{dedup_key: a non-empty string of at most 256 bytes,
/// text: a string, metadata: an object}`. Anything else is a bad row.
fn event(row: &Value) -> Option<(&str, &str, &Value)> {
    let key = row.get("dedup_key")?.as_str().filter(|k| !k.is_empty() && k.len() <= DEDUP_KEY_MAX)?;
    let metadata = row.get("metadata").filter(|m| m.is_object())?;
    Some((key, row.get("text")?.as_str()?, metadata))
}

/// The cooldown, in one statement: the event is delivered only if this INSERT inserted it.
async fn fire(state: &AppState, job: &Job, dedup_key: &str, text: &str, metadata: &Value) {
    let insert = "INSERT INTO notification_events (notification, org, dedup_key, text, metadata) \
                  SELECT $1, $2, $3, $4, $5 WHERE NOT EXISTS (SELECT 1 FROM notification_events \
                  WHERE notification = $1 AND dedup_key = $3 AND created_at > now() - $6::interval) \
                  RETURNING id, created_at";
    let cooldown = format!("{} milliseconds", duration_ms(&job.def.cooldown).unwrap_or(600_000));
    let stored = sqlx::query_as(insert)
        .bind(job.id)
        .bind(&job.org)
        .bind(dedup_key)
        .bind(text)
        .bind(metadata)
        .bind(cooldown)
        .fetch_optional(&state.store.pg)
        .await;
    let event: Option<(i64, DateTime<Utc>)> = match stored {
        Ok(event) => event,
        Err(e) => return tracing::warn!("notification {} could not store an event: {e}", job.id),
    };
    let Some((event_id, fired_at)) = event else { return };
    let frame = json!({"type": "notification", "event_id": event_id, "notification": job.id, "name": job.def.name,
                       "dedup_key": dedup_key, "text": text, "metadata": metadata, "fired_at": fired_at});
    deliver::send(state, &job.org, job.recipients.clone(), frame);
}

/// Whatever went wrong, for the org's dev errors: `source` is `notification`, `ref` is its id.
async fn report(state: &AppState, job: &Job, message: &str, detail: Value) {
    dev_errors::insert(&state.store, &job.org, "notification", &job.id.to_string(), message, detail).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_an_event_only_in_exactly_the_documented_shape() {
        let good = json!({"dedup_key": "o-42", "text": "Order o-42", "metadata": {"amount": "120.5"}});
        assert_eq!(event(&good), Some(("o-42", "Order o-42", &json!({"amount": "120.5"}))));
        assert!(event(&json!({"dedup_key": "k", "text": "t", "metadata": {}, "extra": 1})).is_some());
        for bad in [
            json!({"text": "t", "metadata": {}}),
            json!({"dedup_key": "", "text": "t", "metadata": {}}),
            json!({"dedup_key": 42, "text": "t", "metadata": {}}),
            json!({"dedup_key": "k", "text": 42, "metadata": {}}),
            json!({"dedup_key": "k", "text": "t"}),
            json!({"dedup_key": "k", "text": "t", "metadata": []}),
            json!({"dedup_key": "k", "text": "t", "metadata": "{}"}),
            json!({"dedup_key": "k".repeat(DEDUP_KEY_MAX + 1), "text": "t", "metadata": {}}),
        ] {
            assert_eq!(event(&bad), None, "{bad} is a bad row");
        }
        assert!(event(&json!({"dedup_key": "k".repeat(DEDUP_KEY_MAX), "text": "", "metadata": {}})).is_some());
    }

    #[test]
    fn the_delta_restriction_is_one_bound_per_table_trigger() {
        let def: Def = serde_json::from_value(json!({"name": "n", "sql": "SELECT 1",
            "triggers": [{"table": "orders"}, {"table": "signups"}, {"schedule": "*/5 * * * *"}]}))
        .unwrap();
        let cursors = BTreeMap::from([("orders".to_owned(), 130)]);
        let restrict = restrict(&def, &cursors);
        assert_eq!(restrict.keys().collect::<Vec<_>>(), ["orders", "signups"], "schedules bound nothing");
        assert_eq!((restrict["orders"].from, restrict["orders"].to), (Some(130), None), "the server fills `to`");
        assert_eq!(restrict["signups"].from, None, "a table with no cursor yet is unbounded");
    }
}
