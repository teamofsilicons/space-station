//! The Space Station server. `App::start` boots everything on one tokio runtime — the stores,
//! IAM, the engine lease, the flusher, the trigger fan-out, the notification engine and the HTTP
//! router — and `App::stop` unwinds it in the order that loses nothing. Each module owns one idea
//! from docs/ARCHITECTURE.md.

#![forbid(unsafe_code)]

pub mod access;
pub mod config;
pub mod crypto;
pub mod dev_errors;
pub mod http;
pub mod iam;
pub mod iam_stub;
pub mod ingest;
pub mod live;
pub mod notifications;
pub mod query;
pub mod sql;
pub mod store;
pub mod tables;
pub mod tokens;
pub mod triggers;
pub mod webhooks;
pub mod windows;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::http::{AppState, Inner};

pub struct App {
    pub addr: SocketAddr,
    pub task: JoinHandle<()>,
    stop: watch::Sender<bool>,
}

impl App {
    /// Connects, migrates, bootstraps, binds `cfg.bind` and serves in the background.
    pub async fn start(cfg: Config) -> Result<App, Box<dyn std::error::Error + Send + Sync>> {
        let store = store::Store::connect(&cfg).await?;
        let iam = iam::Client::connect(&cfg).await?;
        let (stop_tx, stop) = watch::channel(false);
        let lease = store::Lease::start(store.redis.clone());
        let state = AppState(Arc::new(Inner {
            cfg,
            store,
            iam,
            lease,
            stop,
            triggers: Arc::default(),
            hub: Default::default(),
            keys: Default::default(),
            visible: Default::default(),
        }));
        let listener = listen(state.cfg.bind).await?;
        let addr = listener.local_addr()?;
        triggers::start(state.clone());
        let flusher = tokio::spawn(store::flush::run(state.store.clone(), state.lease.clone(), state.stop.clone()));
        notifications::Engine::start(state.clone());
        let mut stopped = state.stop.clone();
        let server = axum::serve(listener, http::router(state.clone())).with_graceful_shutdown(async move {
            let _ = stopped.changed().await;
        });
        let lease = state.lease.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = server.await {
                tracing::error!("server stopped: {e}");
            }
            let _ = flusher.await;
            lease.release().await;
        });
        tracing::info!("space station listening on {addr}");
        Ok(App { addr, task, stop: stop_tx })
    }

    /// Stop accepting, close the sockets, let the in-flight flush land, release the lease.
    pub async fn stop(self) {
        self.stop.send_replace(true);
        let _ = self.task.await;
    }
}

/// Binds `addr`, naming it and the reason when it cannot: `cannot listen on 0.0.0.0:8080: address
/// already in use` is what an operator starting a second instance by mistake needs to read.
pub async fn listen(addr: SocketAddr) -> Result<tokio::net::TcpListener, String> {
    tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        let why = match e.kind() {
            std::io::ErrorKind::AddrInUse => "address already in use".to_owned(),
            _ => e.to_string(),
        };
        format!("cannot listen on {addr}: {why}")
    })
}

/// Lock a mutex, recovering the data if a panicking thread poisoned it.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_taken_port_is_refused_by_name() {
        let taken = listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let addr = taken.local_addr().unwrap();
        assert_eq!(listen(addr).await.err(), Some(format!("cannot listen on {addr}: address already in use")));
    }
}
