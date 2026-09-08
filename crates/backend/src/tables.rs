//! Tables: an org's mirage tables as Postgres rows — id, hashed key, access list — with record
//! counts and watermarks from ClickHouse on every read, the one-time key on create and rotate,
//! the overview the Tables tab polls, and the delete that takes the records with it.

use std::collections::HashMap;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use chrono::{DateTime, Utc};
use futures_util::future::try_join_all;
use serde::Deserialize;
use serde_json::{Value, json};
use space_station_shared::secrets::{sha256_hex, table_key, valid_table_id};

use crate::http::auth::Auth;
use crate::http::{ApiError, AppState, Json, Path, Query};
use crate::iam::Identity;
use crate::store::clickhouse::u64_of;
use crate::store::now_ms;
use crate::{access, dev_errors};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/tables", get(list).post(create))
        .route("/orgs/{org}/tables/overview", get(overview))
        .route("/orgs/{org}/tables/{table}", put(update).delete(remove))
        .route("/orgs/{org}/tables/{table}/rotate-key", post(rotate))
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    key_hash: String,
    access: Value,
    created_by: String,
    created_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT id, key_hash, access, created_by, created_at FROM tables";

/// The org's tables, visible ones only, with counts and watermarks.
async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<Value>>, ApiError> {
    auth.allow("tables")?;
    let org = auth.org();
    let sql = format!("{SELECT} WHERE org = $1 ORDER BY id");
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(org).fetch_all(&state.store.pg).await?;
    let visible = auth.visible_tables(&state).await?;
    let counts = counts(&state, org).await?;
    let rows = rows.into_iter().filter(|r| visible.as_ref().is_none_or(|v| v.contains(&r.id)));
    Ok(Json(try_join_all(rows.map(|row| item(&state, org, row, &counts))).await?))
}

async fn item(state: &AppState, org: &str, row: Row, counts: &HashMap<String, u64>) -> Result<Value, ApiError> {
    let watermark = state.store.watermarks.get(org, &row.id).await?;
    let records = counts.get(&row.id).copied().unwrap_or(0);
    Ok(json!({"id": row.id, "records": records, "watermark": watermark, "access": row.access,
              "created_by": row.created_by, "created_at": row.created_at}))
}

/// Records per table in the org, counted under the row policy.
async fn counts(state: &AppState, org: &str) -> Result<HashMap<String, u64>, ApiError> {
    let sql = "SELECT table_id, count() AS n FROM space_station.records GROUP BY table_id FORMAT JSONEachRow";
    let rows = state.store.ch.query_org(org, sql).await?;
    Ok(rows.iter().filter_map(|r| Some((r["table_id"].as_str()?.to_owned(), u64_of(&r["n"])?))).collect())
}

/// The table if it exists and `me` is on its access list: the one test every write shares.
async fn mine(state: &AppState, me: &Identity, table: &str) -> Result<Row, ApiError> {
    let sql = format!("{SELECT} WHERE org = $1 AND id = $2");
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(&me.org)
        .bind(table)
        .fetch_optional(&state.store.pg)
        .await?
        .ok_or_else(|| ApiError::not_found("table"))?;
    if access::matches(me, &access::list(&row.access)) { Ok(row) } else { Err(ApiError::forbidden()) }
}

#[derive(Deserialize)]
struct Create {
    id: String,
    #[serde(default)]
    access: Vec<String>,
}

async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<Create>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let me = auth.actor()?;
    if !valid_table_id(&body.id) {
        return Err(ApiError::bad_request("invalid_table_id", "table ids are 1-50 lowercase letters or digits"));
    }
    access::validate(&body.access)?;
    let key = table_key(&body.id);
    let insert = sqlx::query("INSERT INTO tables (org, id, key_hash, access, created_by) VALUES ($1, $2, $3, $4, $5)")
        .bind(&me.org)
        .bind(&body.id)
        .bind(sha256_hex(&key))
        .bind(Value::from(access::with_actor(body.access, &me.id)))
        .bind(&me.id)
        .execute(&state.store.pg)
        .await;
    match insert {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::new(StatusCode::CONFLICT, "exists", "a table with this id exists"));
        }
        other => other?,
    };
    access::forget(&state, &me.org);
    Ok((StatusCode::CREATED, Json(json!({"key": key}))))
}

#[derive(Deserialize)]
struct Update {
    access: Vec<String>,
}

async fn update(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, table)): Path<(String, String)>,
    Json(body): Json<Update>,
) -> Result<Json<Value>, ApiError> {
    let me = auth.actor()?;
    let row = mine(&state, me, &table).await?;
    access::validate(&body.access)?;
    let access = Value::from(access::with_actor(body.access, &me.id));
    let update = sqlx::query("UPDATE tables SET access = $3 WHERE org = $1 AND id = $2");
    update.bind(&me.org).bind(&table).bind(&access).execute(&state.store.pg).await?;
    access::forget(&state, &me.org);
    let counts = counts(&state, &me.org).await?;
    Ok(Json(item(&state, &me.org, Row { access, ..row }, &counts).await?))
}

async fn rotate(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, table)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let me = auth.actor()?;
    let row = mine(&state, me, &table).await?;
    let key = table_key(&table);
    let update = sqlx::query("UPDATE tables SET key_hash = $3, key_rotated_at = now() WHERE org = $1 AND id = $2");
    update.bind(&me.org).bind(&table).bind(sha256_hex(&key)).execute(&state.store.pg).await?;
    state.keys.forget(&row.key_hash);
    Ok(Json(json!({"key": key})))
}

/// The definition goes now — so the id is free to reuse at once and the key stops resolving —
/// and the records follow on ClickHouse's own clock.
async fn remove(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, table)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let me = auth.actor()?;
    let row = mine(&state, me, &table).await?;
    sqlx::query("DELETE FROM tables WHERE org = $1 AND id = $2")
        .bind(&me.org)
        .bind(&table)
        .execute(&state.store.pg)
        .await?;
    state.keys.forget(&row.key_hash);
    access::forget(&state, &me.org);
    drop_records(state.clone(), me.org.clone(), table);
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE FROM records` for a dropped table. A ClickHouse delete is a mutation, so nobody waits
/// for it; a failure becomes a dev error for the org rather than silence.
fn drop_records(state: AppState, org: String, table: String) {
    tokio::spawn(async move {
        let sql = "DELETE FROM records WHERE org_id = {org:String} AND table_id = {table:String}";
        if let Err(e) = state.store.ch.query_admin(sql, &[("org", &org), ("table", &table)]).await {
            let message = format!("the records of table {table} were not deleted: {e}");
            dev_errors::insert(&state.store, &org, "tables", &table, &message, json!({})).await;
        }
    });
}

const WINDOWS: [(&str, i64); 8] = [
    ("1m", 60_000),
    ("5m", 300_000),
    ("15m", 900_000),
    ("1h", 3_600_000),
    ("5h", 18_000_000),
    ("1d", 86_400_000),
    ("7d", 604_800_000),
    ("30d", 2_592_000_000),
];

/// `{tables, records, top: [{id, records}], avg_lag_ms}` over the visible tables; `top` counts
/// records registered inside `window` (default `5h`), the lag averages the last 100 registered.
async fn overview(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    auth.allow("tables")?;
    let org = auth.org();
    let window = q.get("window").map_or("5h", String::as_str);
    let ms = WINDOWS.iter().find(|(w, _)| *w == window).map(|(_, ms)| *ms);
    let ms = ms.ok_or_else(|| ApiError::bad_request("invalid_window", "window is one of 1m 5m 15m 1h 5h 1d 7d 30d"))?;
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM tables WHERE org = $1 ORDER BY id")
        .bind(org)
        .fetch_all(&state.store.pg)
        .await?;
    let ids: Vec<String> = match auth.visible_tables(&state).await? {
        Some(visible) => ids.into_iter().filter(|id| visible.contains(id)).collect(),
        None => ids,
    };
    let counts = counts(&state, org).await?;
    let records: u64 = ids.iter().map(|id| counts.get(id).copied().unwrap_or(0)).sum();
    let ch = &state.store.ch;
    let since = now_ms() - ms;
    let sql = format!(
        "SELECT table_id, count() AS n FROM space_station.records WHERE registered_ts_ms > {since} \
         GROUP BY table_id ORDER BY n DESC FORMAT JSONEachRow"
    );
    let recent = ch.query_org(org, &sql).await?;
    let visible = |r: &&Value| r["table_id"].as_str().is_some_and(|t| ids.iter().any(|id| id == t));
    let top: Vec<Value> = recent
        .iter()
        .filter(visible)
        .take(5)
        .map(|r| json!({"id": r["table_id"], "records": u64_of(&r["n"])}))
        .collect();
    let avg_lag_ms = if ids.is_empty() {
        Value::Null
    } else {
        let list = ids.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT avgOrNull(registered_ts_ms - event_ts_ms) AS lag FROM (SELECT event_ts_ms, registered_ts_ms \
             FROM space_station.records WHERE table_id IN ({list}) ORDER BY registered_ts_ms DESC LIMIT 100) FORMAT JSONEachRow"
        );
        ch.query_org(org, &sql).await?.first().map_or(Value::Null, |r| r["lag"].clone())
    };
    Ok(Json(json!({"tables": ids.len(), "records": records, "top": top, "avg_lag_ms": avg_lag_ms})))
}
