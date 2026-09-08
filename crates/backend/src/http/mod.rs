//! The HTTP layer: the router that mounts every module under `/api`, the shared `AppState`, and
//! `ApiError`, the one error shape every response uses: `{"error": {"code", "message"}}`. The
//! `Json`, `Path` and `Query` extractors here are axum's with that envelope on rejection.

pub mod auth;

use std::ops::Deref;
use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;
use serde_json::json;
use tokio::sync::watch;

use crate::config::Config;
use crate::iam;
use crate::sql::GuardError;
use crate::store::{ChError, Lease, Store};
use crate::{access, dev_errors, ingest, live, notifications, query, tables, tokens, triggers, webhooks, windows};

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub cfg: Config,
    pub store: Store,
    pub iam: iam::Client,
    pub lease: Lease,
    /// Set once on shutdown; long-lived loops leave when it changes.
    pub stop: watch::Receiver<bool>,
    pub triggers: Arc<triggers::Registry>,
    pub hub: live::Hub,
    pub keys: ingest::Keys,
    pub visible: access::Cache,
}

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .merge(auth::routes())
        .merge(iam::session::routes())
        .merge(iam::webhook::routes())
        .merge(tables::routes())
        .merge(query::routes())
        .merge(tokens::routes())
        .merge(webhooks::routes())
        .merge(windows::routes())
        .merge(dev_errors::routes())
        .merge(ingest::routes())
        .merge(live::routes())
        .merge(notifications::routes());
    Router::new()
        .nest("/api", api)
        // The URL registered with IAM (`--webhook-url …/webhooks/api/`); changing it there needs
        // a review cycle, so the receiver answers at both paths — and, since axum matches a
        // trailing slash strictly, with and without it.
        .merge(iam::webhook::routes_at("/webhooks/api/"))
        .merge(iam::webhook::routes_at("/webhooks/api"))
        .fallback(async || ApiError::not_found("route"))
        .with_state(state)
}

/// Readiness checks the stores used by real requests. Failures reveal no connection details,
/// and a stuck dependency cannot keep a proxy's health probe waiting indefinitely.
async fn health(State(state): State<AppState>) -> Response {
    let check = async {
        let mut redis = state.store.redis.clone();
        let ping = redis::cmd("PING");
        let (pg, redis, ch) = tokio::join!(
            sqlx::query("SELECT 1").execute(&state.store.pg),
            ping.query_async::<String>(&mut redis),
            state.store.ch.query_admin("SELECT 1 FORMAT JSONEachRow", &[]),
        );
        pg.is_ok() && redis.is_ok() && ch.is_ok()
    };
    let ready =
        !*state.stop.borrow() && tokio::time::timeout(std::time::Duration::from_secs(3), check).await.unwrap_or(false);
    if ready {
        Json(json!({"status": "ok"})).into_response()
    } else {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "not_ready", "a required service is unavailable").into_response()
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        ApiError { status, code: code.into(), message: message.into() }
    }

    pub fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn unauthorized(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "no access")
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", format!("{what} not found"))
    }

    /// Logged in full, answered without detail.
    pub fn internal(what: &str, e: impl std::fmt::Display) -> Self {
        tracing::warn!("{what}: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, what, format!("{what} error"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, axum::Json(json!({"error": {"code": self.code, "message": self.message}}))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal("database", e)
    }
}

impl From<redis::RedisError> for ApiError {
    fn from(e: redis::RedisError) -> Self {
        Self::internal("redis", e)
    }
}

impl From<ChError> for ApiError {
    fn from(e: ChError) -> Self {
        match e {
            ChError::Transport(e) => Self::new(StatusCode::SERVICE_UNAVAILABLE, "clickhouse_unavailable", e),
            ChError::Http { message, .. } => Self::bad_request("query_failed", message),
        }
    }
}

impl From<GuardError> for ApiError {
    fn from(e: GuardError) -> Self {
        Self::bad_request(e.code(), e.to_string())
    }
}

impl From<JsonRejection> for ApiError {
    fn from(e: JsonRejection) -> Self {
        Self::bad_request("bad_json", e.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(e: PathRejection) -> Self {
        Self::bad_request("bad_path", e.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(e: QueryRejection) -> Self {
        Self::bad_request("bad_query", e.body_text())
    }
}

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct Path<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct Query<T>(pub T);
