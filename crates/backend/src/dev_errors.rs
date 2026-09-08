//! Dev errors: what went wrong server-side for an org — a flushed row ClickHouse refused, a
//! trigger `where` that fails, a notification run or delivery that failed. `insert` never fails
//! its caller; the Option+Shift+D panel and the CLI read the newest 200.

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::http::auth::Auth;
use crate::http::{ApiError, AppState, Json};
use crate::store::Store;

pub async fn insert(store: &Store, org: &str, source: &str, ref_: &str, message: &str, detail: Value) {
    let insert = sqlx::query("INSERT INTO dev_errors (org, source, ref, message, detail) VALUES ($1, $2, $3, $4, $5)");
    if let Err(e) = insert.bind(org).bind(source).bind(ref_).bind(message).bind(detail).execute(&store.pg).await {
        tracing::warn!("dev_errors insert failed: {e}");
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/orgs/{org}/dev-errors", get(list))
}

#[derive(sqlx::FromRow, Serialize)]
struct DevError {
    id: i64,
    source: String,
    #[sqlx(rename = "ref")]
    #[serde(rename = "ref")]
    reference: String,
    message: String,
    detail: Value,
    created_at: DateTime<Utc>,
}

async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<DevError>>, ApiError> {
    auth.actor()?;
    let sql =
        "SELECT id, source, ref, message, detail, created_at FROM dev_errors WHERE org = $1 ORDER BY id DESC LIMIT 200";
    Ok(Json(sqlx::query_as(sql).bind(auth.org()).fetch_all(&state.store.pg).await?))
}
