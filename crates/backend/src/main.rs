//! The `space-station-backend` binary: config from the environment, logs on stderr, the whole
//! server through `App::start`, and a clean stop on Ctrl-C or SIGTERM.

use std::process::ExitCode;

use space_station_backend::App;
use space_station_backend::config::Config;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("config: {e}");
            return ExitCode::from(2);
        }
    };
    let app = match App::start(cfg).await {
        Ok(app) => app,
        Err(e) => {
            eprintln!("start: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        eprintln!("cannot listen for SIGTERM");
        return ExitCode::FAILURE;
    };
    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
    tracing::info!("stopping");
    app.stop().await;
    ExitCode::SUCCESS
}
