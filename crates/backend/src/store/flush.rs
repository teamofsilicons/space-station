//! The flusher: the only place cursors are assigned. Every second, or as soon as 16 MB wait, it
//! turns the head of `staging` into one `INSERT … FORMAT JSONEachRow`, survives a crash through
//! the `flushing` key and ClickHouse's deduplication token, quarantines rows ClickHouse refuses
//! in `dead`, and broadcasts `Flushed`. `Watermarks` is what everyone else reads: the last cursor
//! a successful flush landed while this process is the flusher, else `max(cursor)` from
//! ClickHouse, re-read at most once a flush interval so the other half of a deploy keeps up.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::RawValue;
use space_station_shared::limits::{CURSOR_BOOT_SHIFT, FLUSH_BYTES, FLUSH_INTERVAL_MS};
use tokio::sync::watch;
use uuid::Uuid;

use super::clickhouse::u64_of;
use super::{ChError, Clickhouse, Lease, Store, now_ms};
use crate::{dev_errors, lock};

/// Rows `(from, to]` of `table` are now in ClickHouse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flushed {
    pub org: String,
    pub table: String,
    pub from: u64,
    pub to: u64,
}

/// Rows read from `staging` per drain.
const CHUNK: usize = 10_000;

/// Cleanup must be atomic: a crash after trimming staging but before clearing the marker would
/// replay the same trim against the next records. The identity check also makes a stale holder
/// harmless if another holder has already completed this flush or begun the next one.
const FINISH: &str = "local pending = redis.call('GET', KEYS[2])\n\
    if not pending or cjson.decode(pending).flush_id ~= ARGV[1] then return 0 end\n\
    redis.call('LTRIM', KEYS[1], ARGV[2], -1)\n\
    redis.call('DEL', KEYS[2])\n\
    return 1";

/// Per (org, table): the watermark, the cursor counter once this process has assigned one, and
/// when ClickHouse was last read for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    watermark: u64,
    next: Option<u64>,
    read_at: Instant,
}

impl Default for Mark {
    fn default() -> Self {
        Mark { watermark: 0, next: None, read_at: Instant::now() }
    }
}

impl Mark {
    /// The first of `n` fresh cursors. A table starts at 1; once ClickHouse holds a row for it,
    /// the counter starts at `watermark + CURSOR_BOOT_SHIFT` on its first use, so a restart
    /// leaves room for rows a previous holder landed but this read cannot see yet. There is no
    /// such room to leave above zero: a watermark of 0 is a table ClickHouse answers nothing for,
    /// which +10000 would not make any safer.
    fn reserve(&mut self, n: u64) -> u64 {
        let seed = if self.watermark == 0 { 1 } else { self.watermark + CURSOR_BOOT_SHIFT };
        let first = self.next.unwrap_or(seed);
        self.next = Some(first + n);
        first
    }
}

#[derive(Clone)]
pub struct Watermarks {
    ch: Clickhouse,
    marks: Arc<Mutex<HashMap<(String, String), Mark>>>,
    /// Set while this process is the flusher, which is the only thing that makes a mark the
    /// truth rather than a reading of ClickHouse.
    flushing: Arc<AtomicBool>,
}

impl Watermarks {
    pub fn new(ch: Clickhouse) -> Self {
        Watermarks { ch, marks: Arc::default(), flushing: Arc::default() }
    }

    /// The watermark of (org, table): what the flusher last landed while this process is the
    /// flusher, else `max(cursor)` from ClickHouse, re-read at most once a flush interval.
    pub async fn get(&self, org: &str, table: &str) -> Result<u64, ChError> {
        Ok(self.mark(org, table).await?.watermark)
    }

    async fn mark(&self, org: &str, table: &str) -> Result<Mark, ChError> {
        let key = (org.to_owned(), table.to_owned());
        let fresh =
            |m: &Mark| self.flushing.load(Relaxed) || m.read_at.elapsed().as_millis() < FLUSH_INTERVAL_MS.into();
        if let Some(mark) = lock(&self.marks).get(&key).filter(|m| fresh(m)) {
            return Ok(*mark);
        }
        let sql = "SELECT max(cursor) AS m FROM records WHERE org_id = {org:String} AND table_id = {table:String} \
                   FORMAT JSONEachRow";
        let rows = self.ch.query_admin(sql, &[("org", org), ("table", table)]).await?;
        let watermark = rows.first().and_then(|r| u64_of(&r["m"])).unwrap_or(0);
        let mut marks = lock(&self.marks);
        let mark = marks.entry(key).or_default();
        mark.watermark = mark.watermark.max(watermark);
        mark.read_at = Instant::now();
        Ok(*mark)
    }

    /// `(watermark, first cursor)` for `n` rows about to be inserted.
    async fn reserve(&self, org: &str, table: &str, n: u64) -> Result<(u64, u64), ChError> {
        self.mark(org, table).await?;
        let mut marks = lock(&self.marks);
        let mark = marks.entry((org.to_owned(), table.to_owned())).or_default();
        Ok((mark.watermark, mark.reserve(n)))
    }

    fn set(&self, org: &str, table: &str, to: u64) {
        lock(&self.marks).entry((org.to_owned(), table.to_owned())).or_default().watermark = to;
    }

    /// This process took the flusher's role, or lost it: forget everything, so the next use
    /// re-reads ClickHouse and re-seeds the counters.
    fn flushing(&self, yes: bool) {
        self.flushing.store(yes, Relaxed);
        lock(&self.marks).clear();
    }
}

/// A line of `staging`, as ingest wrote it.
#[derive(Deserialize)]
struct Staged<'a> {
    org: &'a str,
    table: &'a str,
    record_id: &'a str,
    event_ts_ms: i64,
    #[serde(borrow)]
    metadata: &'a RawValue,
    #[serde(borrow)]
    record: &'a RawValue,
}

/// A line of the INSERT body: a `records` row.
#[derive(Serialize)]
struct Row<'a> {
    org_id: &'a str,
    table_id: &'a str,
    cursor: u64,
    record_id: &'a str,
    event_ts_ms: i64,
    registered_ts_ms: i64,
    metadata: &'a RawValue,
    record: &'a RawValue,
}

#[derive(Deserialize)]
struct RowHead {
    org_id: String,
    record_id: String,
    cursor: u64,
}

/// One drain, kept in Redis under `flushing` from before its INSERT until it has landed.
#[derive(Serialize, Deserialize)]
struct Pending {
    flush_id: String,
    /// Lines of `staging` this covers, trimmed once landed.
    n: usize,
    body: String,
    marks: Vec<Flushed>,
}

type Fault = Box<dyn std::error::Error + Send + Sync>;

/// Drains while the lease is held; returns once `stop` is set and the in-flight flush is done.
pub async fn run(store: Store, mut lease: Lease, mut stop: watch::Receiver<bool>) {
    let mut was_held = false;
    while !*stop.borrow() {
        if !lease.held() {
            if std::mem::take(&mut was_held) {
                store.watermarks.flushing(false);
            }
            tokio::select! { _ = lease.changed() => {}, _ = stop.changed() => {} }
            continue;
        }
        if !was_held {
            was_held = true;
            store.watermarks.flushing(true);
            inherited(&store).await;
        }
        let wait = match step(&store).await {
            Ok(true) => continue,
            Ok(false) => Duration::from_millis(FLUSH_INTERVAL_MS),
            Err(e) => {
                tracing::warn!("flush failed, retrying: {e}");
                Duration::from_secs(1)
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = store.staged.full() => {}
            _ = lease.changed() => {}
            _ = stop.changed() => {}
        }
    }
}

/// What a previous holder left behind, said once when this process takes the lease: rows waiting
/// in `staging` and a flush that was interrupted before it landed. A clean handover says nothing.
async fn inherited(store: &Store) {
    let mut redis = store.redis.clone();
    let staged: usize = redis.llen("staging").await.unwrap_or(0);
    let interrupted = redis.get::<_, Option<String>>("flushing").await.ok().flatten();
    let interrupted = interrupted.and_then(|json| serde_json::from_str::<Pending>(&json).ok());
    match interrupted {
        Some(pending) => tracing::info!(
            "engine lease taken with {staged} staged rows waiting; re-landing interrupted flush {} of {} rows first",
            pending.flush_id,
            pending.n
        ),
        None if staged > 0 => tracing::info!("engine lease taken with {staged} staged rows waiting; draining them"),
        None => {}
    }
}

/// Lands the interrupted flush if there is one, else the next chunk; `true` when a full chunk
/// went, so the caller should not wait for the tick.
async fn step(store: &Store) -> Result<bool, Fault> {
    let mut redis = store.redis.clone();
    let pending = match redis.get::<_, Option<String>>("flushing").await? {
        Some(json) => serde_json::from_str(&json)?,
        None => match prepare(store).await? {
            Some(pending) => pending,
            None => return Ok(false),
        },
    };
    let full = pending.n == CHUNK || pending.body.len() >= FLUSH_BYTES;
    commit(store, pending).await?;
    Ok(full)
}

/// Reads the head of `staging`, assigns cursors and `registered_ts_ms`, writes `flushing`.
async fn prepare(store: &Store) -> Result<Option<Pending>, Fault> {
    let mut redis = store.redis.clone();
    let lines: Vec<String> = redis.lrange("staging", 0, CHUNK as isize - 1).await?;
    if lines.is_empty() {
        return Ok(None);
    }
    store.staged.drained();
    let (mut n, mut bytes) = (0, 0);
    for line in &lines {
        if n > 0 && bytes + line.len() > FLUSH_BYTES {
            break;
        }
        bytes += line.len();
        n += 1;
    }
    let lines = &lines[..n];
    let parsed: Vec<Option<Staged>> = lines.iter().map(|l| serde_json::from_str(l).ok()).collect();
    let mut counts: BTreeMap<(&str, &str), u64> = BTreeMap::new();
    for row in parsed.iter().flatten() {
        *counts.entry((row.org, row.table)).or_default() += 1;
    }
    let mut cursors = BTreeMap::new();
    for ((org, table), count) in counts {
        let (from, first) = store.watermarks.reserve(org, table, count).await?;
        let mark = Flushed { org: org.into(), table: table.into(), from, to: first + count - 1 };
        cursors.insert((org, table), (first, mark));
    }
    let (now, mut body, mut dead) = (now_ms(), Vec::with_capacity(bytes + 128 * n), Vec::new());
    for (line, row) in lines.iter().zip(parsed) {
        let Some(row) = row else {
            dead.push(line.as_str());
            continue;
        };
        let (next, _) = cursors.get_mut(&(row.org, row.table)).expect("counted above");
        let out = Row {
            org_id: row.org,
            table_id: row.table,
            cursor: *next,
            record_id: row.record_id,
            event_ts_ms: row.event_ts_ms,
            registered_ts_ms: now,
            metadata: row.metadata,
            record: row.record,
        };
        *next += 1;
        serde_json::to_writer(&mut body, &out)?;
        body.push(b'\n');
    }
    if !dead.is_empty() {
        tracing::warn!("{} unparseable staging rows moved to dead", dead.len());
        let _: usize = redis.rpush("dead", dead).await?;
    }
    let marks = cursors.into_values().map(|(_, mark)| mark).collect();
    let pending = Pending { flush_id: Uuid::new_v4().to_string(), n, body: String::from_utf8(body)?, marks };
    let _: () = redis.set("flushing", serde_json::to_string(&pending)?).await?;
    Ok(Some(pending))
}

/// The INSERT (row by row when ClickHouse refuses the block), then the trim and the broadcast.
async fn commit(store: &Store, pending: Pending) -> Result<(), Fault> {
    match store.ch.insert(pending.body.clone().into_bytes(), &pending.flush_id).await {
        Ok(()) => {}
        Err(ChError::Http { status: 400 | 413 | 422, message }) => {
            tracing::warn!("flush {} refused ({message}), inserting row by row", pending.flush_id);
            quarantine(store, &pending).await?;
        }
        Err(e) => return Err(e.into()),
    }
    let mut redis = store.redis.clone();
    let finished: bool = redis::Script::new(FINISH)
        .key("staging")
        .key("flushing")
        .arg(&pending.flush_id)
        .arg(pending.n)
        .invoke_async(&mut redis)
        .await?;
    if !finished {
        return Ok(());
    }
    for mark in pending.marks {
        store.watermarks.set(&mark.org, &mark.table, mark.to);
        let _ = store.flushed.send(mark);
    }
    Ok(())
}

/// Every row alone under its own token; a refused row goes to `dead` with a `dev_errors` entry.
async fn quarantine(store: &Store, pending: &Pending) -> Result<(), Fault> {
    let mut redis = store.redis.clone();
    for (i, line) in pending.body.lines().enumerate() {
        match store.ch.insert(line.as_bytes().to_vec(), &format!("{}/{i}", pending.flush_id)).await {
            Ok(()) => {}
            Err(ChError::Http { status: 400 | 413 | 422, message }) => {
                let _: usize = redis.rpush("dead", line).await?;
                let head: RowHead = serde_json::from_str(line)?;
                let detail = json!({"cursor": head.cursor, "flush_id": pending.flush_id});
                dev_errors::insert(store, &head.org_id, "flush", &head.record_id, &message, detail).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
