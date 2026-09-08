//! The engine lease: `SET lease:engine NX PX 5000`, renewed every 2 s, so exactly one process
//! runs the flusher and the notification engine. Holders watch `held()`; a failed renewal flips
//! it to false at once. `release` hands the lease over on shutdown. Every change of hands is one
//! `info` line — acquired, lost, passive behind another holder — because those are the moments an
//! operator reading the log needs to see; the healthy steady state says nothing.

use std::time::Duration;

use redis::aio::ConnectionManager;
use redis::{AsyncCommands, ExistenceCheck, Script, SetExpiry, SetOptions};
use tokio::sync::watch;
use uuid::Uuid;

const KEY: &str = "lease:engine";
const TTL_MS: u64 = 5_000;
const RENEW_EVERY: Duration = Duration::from_secs(2);
/// Touch or drop the key only while it still carries our id.
const RENEW: &str =
    "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('PEXPIRE', KEYS[1], ARGV[2]) end return 0";
const RELEASE: &str = "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end return 0";

#[derive(Clone)]
pub struct Lease {
    held: watch::Receiver<bool>,
    release: watch::Sender<bool>,
    /// Set once the loop has let go of the key.
    done: watch::Receiver<bool>,
}

impl Lease {
    /// Starts competing for the lease in the background.
    pub fn start(mut redis: ConnectionManager) -> Lease {
        let (held_tx, held) = watch::channel(false);
        let (release, mut released) = watch::channel(false);
        let (done_tx, done) = watch::channel(false);
        tokio::spawn(async move {
            let id = Uuid::new_v4().to_string();
            let mut was: Option<bool> = None;
            loop {
                let holding =
                    if *held_tx.borrow() { renew(&mut redis, &id).await } else { acquire(&mut redis, &id).await };
                match (was, holding) {
                    (Some(true), true) | (Some(false), false) => {}
                    (None, false) => tracing::info!("engine lease held by another instance; this one is passive"),
                    (Some(true), false) => tracing::info!("engine lease lost: the flusher and the engine stop here"),
                    (_, true) => tracing::info!("engine lease acquired: this instance flushes and fires notifications"),
                }
                was = Some(holding);
                held_tx.send_if_modified(|h| std::mem::replace(h, holding) != holding);
                tokio::select! { _ = tokio::time::sleep(RENEW_EVERY) => {}, _ = released.changed() => break }
            }
            let _: Result<i64, _> = Script::new(RELEASE).key(KEY).arg(&id).invoke_async(&mut redis).await;
            held_tx.send_replace(false);
            done_tx.send_replace(true);
        });
        Lease { held, release, done }
    }

    pub fn held(&self) -> bool {
        *self.held.borrow()
    }

    /// Resolves when `held()` flips either way.
    pub async fn changed(&mut self) {
        let _ = self.held.changed().await;
    }

    /// Lets go of the lease and returns once the key is gone (or was never ours).
    pub async fn release(&self) {
        self.release.send_replace(true);
        let mut done = self.done.clone();
        while !*done.borrow_and_update() {
            if done.changed().await.is_err() {
                break;
            }
        }
    }
}

async fn acquire(redis: &mut ConnectionManager, id: &str) -> bool {
    let options = SetOptions::default().conditional_set(ExistenceCheck::NX).with_expiration(SetExpiry::PX(TTL_MS));
    matches!(redis.set_options::<_, _, Option<String>>(KEY, id, options).await, Ok(Some(_)))
}

async fn renew(redis: &mut ConnectionManager, id: &str) -> bool {
    matches!(Script::new(RENEW).key(KEY).arg(id).arg(TTL_MS).invoke_async::<i64>(redis).await, Ok(1))
}
