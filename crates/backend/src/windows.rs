//! Space windows: name, access list, versions (each a named processor + renderer pair, refused
//! when either carries a secret), the current version inline on every read, and the state a
//! processor last produced with whether it is still live.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use space_station_shared::limits::{LIVE_WINDOW_MS, WINDOW_NAME_MAX};
use space_station_shared::secrets::find_secret;
use uuid::Uuid;

use crate::access;
use crate::http::auth::Auth;
use crate::http::{ApiError, AppState, Json, Path};
use crate::iam::Identity;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/windows", get(list).post(create))
        .route("/orgs/{org}/windows/{id}", get(show).put(update).delete(remove))
        .route("/orgs/{org}/windows/{id}/versions", get(versions).post(publish))
        .route("/orgs/{org}/windows/{id}/state", get(state_of))
}

const SELECT: &str = "SELECT w.id, w.name, w.access, w.created_by, w.created_at, v.name AS version, v.processor, v.renderer \
                      FROM windows w LEFT JOIN window_versions v ON v.id = w.current_version";

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    name: String,
    access: Value,
    created_by: String,
    created_at: DateTime<Utc>,
    version: Option<String>,
    processor: Option<String>,
    renderer: Option<String>,
}

impl Row {
    fn json(self) -> Value {
        let version =
            self.version.map(|name| json!({"name": name, "processor": self.processor, "renderer": self.renderer}));
        json!({"id": self.id, "name": self.name, "access": self.access, "created_by": self.created_by,
               "created_at": self.created_at, "version": version})
    }
}

/// The window if it exists and `me` may see it.
async fn fetch(state: &AppState, me: &Identity, id: Uuid) -> Result<Row, ApiError> {
    let sql = format!("{SELECT} WHERE w.org = $1 AND w.id = $2");
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(&me.org)
        .bind(id)
        .fetch_optional(&state.store.pg)
        .await?
        .ok_or_else(|| ApiError::not_found("window"))?;
    if access::matches(me, &access::list(&row.access)) { Ok(row) } else { Err(ApiError::forbidden()) }
}

async fn list(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<Value>>, ApiError> {
    let me = auth.actor()?;
    let sql = format!("{SELECT} WHERE w.org = $1 ORDER BY w.created_at");
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(&me.org).fetch_all(&state.store.pg).await?;
    Ok(Json(rows.into_iter().filter(|r| access::matches(me, &access::list(&r.access))).map(Row::json).collect()))
}

fn valid_name(name: &str) -> Result<(), ApiError> {
    let n = name.chars().count();
    if n == 0 || n > WINDOW_NAME_MAX {
        return Err(ApiError::bad_request(
            "invalid_name",
            format!("a window name is 1 to {WINDOW_NAME_MAX} characters"),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct Create {
    name: String,
    #[serde(default)]
    access: Vec<String>,
}

async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<Create>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let me = auth.actor()?;
    valid_name(&body.name)?;
    access::validate(&body.access)?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO windows (id, org, name, access, created_by) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(&me.org)
        .bind(&body.name)
        .bind(Value::from(access::with_actor(body.access, &me.id)))
        .bind(&me.id)
        .execute(&state.store.pg)
        .await?;
    Ok((StatusCode::CREATED, Json(fetch(&state, me, id).await?.json())))
}

async fn show(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(fetch(&state, auth.actor()?, id).await?.json()))
}

#[derive(Deserialize)]
struct Update {
    name: Option<String>,
    access: Option<Vec<String>>,
}

async fn update(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
    Json(body): Json<Update>,
) -> Result<Json<Value>, ApiError> {
    let me = auth.actor()?;
    fetch(&state, me, id).await?;
    if let Some(name) = &body.name {
        valid_name(name)?;
    }
    if let Some(access) = &body.access {
        access::validate(access)?;
    }
    sqlx::query("UPDATE windows SET name = COALESCE($2, name), access = COALESCE($3, access) WHERE id = $1")
        .bind(id)
        .bind(body.name)
        .bind(body.access.map(|a| Value::from(access::with_actor(a, &me.id))))
        .execute(&state.store.pg)
        .await?;
    Ok(Json(fetch(&state, me, id).await?.json()))
}

/// A window takes its versions and its stored state with it. One statement is enough even though
/// `windows.current_version` points *into* `window_versions`: that foreign key is `NO ACTION`,
/// which Postgres checks at the end of the statement, by which time the row holding the pointer
/// has been deleted too. The "Deleting" step of `the_whole_station_end_to_end` in tests/core.rs
/// is the proof.
async fn remove(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    fetch(&state, auth.actor()?, id).await?;
    sqlx::query("DELETE FROM windows WHERE id = $1").bind(id).execute(&state.store.pg).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow, Serialize)]
struct Version {
    id: Uuid,
    name: String,
    processor: String,
    renderer: String,
    created_by: String,
    created_at: DateTime<Utc>,
}

const VERSION: &str = "id, name, processor, renderer, created_by, created_at";

async fn versions(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<Version>>, ApiError> {
    fetch(&state, auth.actor()?, id).await?;
    let sql = format!("SELECT {VERSION} FROM window_versions WHERE window_id = $1 ORDER BY created_at DESC");
    Ok(Json(sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_all(&state.store.pg).await?))
}

#[derive(Deserialize)]
struct Publish {
    name: String,
    processor: String,
    renderer: String,
}

/// A new version becomes current at once; code carrying any secret shape is refused.
async fn publish(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
    Json(body): Json<Publish>,
) -> Result<(StatusCode, Json<Version>), ApiError> {
    let me = auth.actor()?;
    fetch(&state, me, id).await?;
    if body.name.is_empty() || body.name.chars().count() > 64 {
        return Err(ApiError::bad_request("invalid_name", "a version name is 1 to 64 characters"));
    }
    if let Some(secret) = find_secret(&body.processor).or_else(|| find_secret(&body.renderer)) {
        let message = format!("the code contains a secret ({}…); use the dev server and .env instead", &secret[..12]);
        return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "secret_in_code", message));
    }
    let mut tx = state.store.pg.begin().await?;
    let sql = format!(
        "INSERT INTO window_versions (id, window_id, name, processor, renderer, created_by) VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING {VERSION}"
    );
    let version: Version = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(&body.name)
        .bind(&body.processor)
        .bind(&body.renderer)
        .bind(&me.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e.as_database_error().and_then(|d| d.constraint()) {
            Some("window_versions_window_name") => ApiError::new(
                StatusCode::CONFLICT,
                "duplicate_name",
                format!("this window already has a version {}", body.name),
            ),
            _ => e.into(),
        })?;
    sqlx::query("UPDATE windows SET current_version = $2 WHERE id = $1")
        .bind(id)
        .bind(version.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(version)))
}

/// `{json, metadata: {processor_version, renderer_version, produced_at, is_live}}`.
async fn state_of(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    let row = fetch(&state, auth.actor()?, id).await?;
    let sql = "SELECT state, state_version, produced_at FROM windows WHERE id = $1";
    let (json, state_version, produced_at): (Option<Value>, Option<String>, Option<DateTime<Utc>>) =
        sqlx::query_as(sql).bind(id).fetch_one(&state.store.pg).await?;
    let is_live = is_live(state_version.as_deref(), row.version.as_deref(), produced_at, Utc::now());
    Ok(Json(json!({"json": json, "metadata": {"processor_version": row.version, "renderer_version": row.version,
                   "produced_at": produced_at, "is_live": is_live}})))
}

/// A state for the current version, received less than `LIVE_WINDOW_MS` ago.
fn is_live(
    state_version: Option<&str>,
    current: Option<&str>,
    produced_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    current.is_some()
        && state_version == current
        && produced_at.is_some_and(|at| now - at < TimeDelta::milliseconds(LIVE_WINDOW_MS as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_means_the_current_version_within_the_window() {
        let now = Utc::now();
        let recent = Some(now - TimeDelta::seconds(5));
        let old = Some(now - TimeDelta::seconds(31));
        assert!(is_live(Some("v3"), Some("v3"), recent, now));
        assert!(!is_live(Some("v2"), Some("v3"), recent, now), "an older version's state is stale");
        assert!(!is_live(Some("v3"), Some("v3"), old, now), "silence for 30 s is stale");
        assert!(!is_live(Some("v3"), None, recent, now), "no version published yet");
        assert!(!is_live(None, Some("v3"), None, now));
    }
}
