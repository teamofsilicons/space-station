//! One handle to the three stores. Postgres (sqlx) keeps every definition, Redis stages records
//! and holds the engine lease, ClickHouse holds the records. `Flushed` is the event the flusher
//! broadcasts once rows are queryable, and `Staged` is how ingest wakes the flusher early when
//! 16 MB are waiting.

pub mod clickhouse;
pub mod flush;
pub mod lease;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub use clickhouse::{ChError, Clickhouse};
pub use flush::{Flushed, Watermarks};
pub use lease::Lease;
use redis::aio::ConnectionManager;
use space_station_shared::limits::FLUSH_BYTES;
use space_station_shared::secrets::sha256_hex;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::{Notify, broadcast};

use crate::config::Config;

#[derive(Clone)]
pub struct Store {
    pub pg: PgPool,
    pub redis: ConnectionManager,
    pub ch: Clickhouse,
    pub watermarks: Watermarks,
    pub flushed: broadcast::Sender<Flushed>,
    pub staged: Arc<Staged>,
}

impl Store {
    /// Connects to all three, runs the migrations and the ClickHouse bootstrap; fails loudly.
    pub async fn connect(cfg: &Config) -> Result<Store, Box<dyn std::error::Error + Send + Sync>> {
        let pg = PgPoolOptions::new().max_connections(16).connect(&cfg.database_url).await?;
        sqlx::migrate!("./migrations").run(&pg).await?;
        let configured_key = cfg.iam_test_key.as_deref().map(sha256_hex);
        // Bind fresh stores once too: an empty store will later contain identities from this world.
        sqlx::query(
            "UPDATE public_identifier_migration SET environment_bound = true, testing_key_sha256 = $1 \
                     WHERE ready AND NOT environment_bound",
        )
        .bind(&configured_key)
        .execute(&pg)
        .await?;
        let (ready, testing_key): (bool, Option<String>) =
            sqlx::query_as("SELECT ready, testing_key_sha256 FROM public_identifier_migration").fetch_one(&pg).await?;
        if !ready {
            return Err("public identities need offline migration; follow docs/PUBLIC-ID-MIGRATION.md".into());
        }
        if testing_key != configured_key {
            return Err("IAM testing environment does not match this database's identifier migration".into());
        }
        let redis = ConnectionManager::new(redis::Client::open(cfg.redis_url.as_str())?).await?;
        let ch = Clickhouse::new(&cfg.clickhouse_url, &cfg.clickhouse_query_password)?;
        ch.bootstrap(&cfg.clickhouse_query_password).await?;
        let watermarks = Watermarks::new(ch.clone());
        Ok(Store { pg, redis, ch, watermarks, flushed: broadcast::channel(1024).0, staged: Arc::default() })
    }
}

/// Bytes pushed to `staging` since the flusher last drained; reaching `FLUSH_BYTES` wakes it.
#[derive(Default)]
pub struct Staged {
    bytes: AtomicUsize,
    wake: Notify,
}

impl Staged {
    pub fn add(&self, bytes: usize) {
        if self.bytes.fetch_add(bytes, Relaxed) + bytes >= FLUSH_BYTES {
            self.wake.notify_one();
        }
    }

    fn drained(&self) {
        self.bytes.store(0, Relaxed);
    }

    async fn full(&self) {
        self.wake.notified().await;
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
