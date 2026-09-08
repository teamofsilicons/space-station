//! Access tokens and API keys. An access token is the one live `spacewindow-…` per (org, actor):
//! sealed so it can be shown again, hashed for lookup, and resolving through the directory mirror
//! like a session does, so its tags are whatever IAM last said. An API key acts for the org within
//! its scopes and is shown once. Both note when they were last used, at most once a minute.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use space_station_shared::secrets::{access_token, api_key, sha256_hex};
use uuid::Uuid;

use crate::crypto;
use crate::http::auth::Auth;
use crate::http::{ApiError, AppState, Json, Path};
use crate::iam::Identity;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/access-token", get(show))
        .route("/orgs/{org}/access-token/rotate", post(rotate))
        .route("/orgs/{org}/api-keys", get(list_keys).post(create_key))
        .route("/orgs/{org}/api-keys/{id}", delete(delete_key))
}

/// The caller's token, minted on first sight.
async fn show(State(state): State<AppState>, auth: Auth) -> Result<Json<Value>, ApiError> {
    let me = auth.actor()?;
    let sql = "SELECT token_enc, last_used_at FROM access_tokens WHERE org = $1 AND actor = $2";
    let row: Option<(String, Option<DateTime<Utc>>)> =
        sqlx::query_as(sql).bind(&me.org).bind(&me.id).fetch_optional(&state.store.pg).await?;
    if let Some((sealed, last_used_at)) = row
        && let Some(token) = crypto::open(&state.cfg.key, &sealed)
    {
        return Ok(Json(json!({"token": token, "last_used_at": last_used_at})));
    }
    // Insert-if-absent closes the first-use race: concurrent callers all read back the one
    // committed token, so nobody receives a token that the next request already replaced.
    Ok(Json(json!({"token": mint_if_absent(&state, me).await?, "last_used_at": null})))
}

async fn rotate(State(state): State<AppState>, auth: Auth) -> Result<Json<Value>, ApiError> {
    let me = auth.actor()?;
    Ok(Json(json!({"token": mint(&state, me).await?, "last_used_at": null})))
}

/// A fresh token for (org, actor), replacing whatever was there. The actor's membership and
/// principal ids are captured from the directory mirror so a removal tombstone — which names only
/// those — can still reach and revoke this token after the actor's session is gone.
async fn mint(state: &AppState, me: &Identity) -> Result<String, ApiError> {
    let token = access_token();
    let ids: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT membership_id, principal_id FROM iam_members WHERE org = $1 AND actor = $2")
            .bind(&me.org)
            .bind(&me.id)
            .fetch_optional(&state.store.pg)
            .await?;
    let (membership_id, principal_id) = ids.map(|(m, p)| (Some(m), p)).unwrap_or_default();
    sqlx::query(
        "INSERT INTO access_tokens (org, actor, token_hash, token_enc, membership_id, principal_id) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (org, actor) DO UPDATE SET token_hash = EXCLUDED.token_hash, token_enc = EXCLUDED.token_enc, \
         membership_id = EXCLUDED.membership_id, principal_id = EXCLUDED.principal_id, created_at = now(), \
         last_used_at = NULL",
    )
    .bind(&me.org)
    .bind(&me.id)
    .bind(sha256_hex(&token))
    .bind(crypto::seal(&state.cfg.key, &token))
    .bind(&membership_id)
    .bind(&principal_id)
    .execute(&state.store.pg)
    .await?;
    Ok(token)
}

async fn mint_if_absent(state: &AppState, me: &Identity) -> Result<String, ApiError> {
    let token = access_token();
    let ids: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT membership_id, principal_id FROM iam_members WHERE org = $1 AND actor = $2")
            .bind(&me.org)
            .bind(&me.id)
            .fetch_optional(&state.store.pg)
            .await?;
    let (membership_id, principal_id) = ids.map(|(m, p)| (Some(m), p)).unwrap_or_default();
    sqlx::query(
        "INSERT INTO access_tokens (org, actor, token_hash, token_enc, membership_id, principal_id) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (org, actor) DO NOTHING",
    )
    .bind(&me.org)
    .bind(&me.id)
    .bind(sha256_hex(&token))
    .bind(crypto::seal(&state.cfg.key, &token))
    .bind(&membership_id)
    .bind(&principal_id)
    .execute(&state.store.pg)
    .await?;
    let sealed: String = sqlx::query_scalar("SELECT token_enc FROM access_tokens WHERE org = $1 AND actor = $2")
        .bind(&me.org)
        .bind(&me.id)
        .fetch_one(&state.store.pg)
        .await?;
    match crypto::open(&state.cfg.key, &sealed) {
        Some(token) => Ok(token),
        None => mint(state, me).await,
    }
}

fn due(last_used_at: Option<DateTime<Utc>>) -> bool {
    last_used_at.is_none_or(|at| Utc::now() - at > TimeDelta::minutes(1))
}

/// The identity behind a `spacewindow-` bearer, in `org` when given.
pub async fn by_access_token(state: &AppState, token: &str, org: Option<&str>) -> Result<Identity, ApiError> {
    let hash = sha256_hex(token);
    let sql =
        "SELECT org, actor, last_used_at FROM access_tokens WHERE token_hash = $1 AND ($2::text IS NULL OR org = $2)";
    let row: Option<(String, String, Option<DateTime<Utc>>)> =
        sqlx::query_as(sql).bind(&hash).bind(org).fetch_optional(&state.store.pg).await?;
    let (org, actor, last_used_at) =
        row.ok_or_else(|| ApiError::unauthorized("invalid_token", "unknown access token"))?;
    if due(last_used_at) {
        sqlx::query("UPDATE access_tokens SET last_used_at = now() WHERE token_hash = $1")
            .bind(&hash)
            .execute(&state.store.pg)
            .await?;
    }
    Identity::resolve(state, &org, &actor).await
}

/// The scopes of an `apikey-` bearer in `org`.
pub async fn by_api_key(state: &AppState, key: &str, org: &str) -> Result<Vec<String>, ApiError> {
    let hash = sha256_hex(key);
    let sql = "SELECT scopes, last_used_at FROM api_keys WHERE key_hash = $1 AND org = $2";
    let row: Option<(Vec<String>, Option<DateTime<Utc>>)> =
        sqlx::query_as(sql).bind(&hash).bind(org).fetch_optional(&state.store.pg).await?;
    let (scopes, last_used_at) = row.ok_or_else(|| ApiError::unauthorized("invalid_key", "unknown API key"))?;
    if due(last_used_at) {
        sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE key_hash = $1")
            .bind(&hash)
            .execute(&state.store.pg)
            .await?;
    }
    Ok(scopes)
}

#[derive(sqlx::FromRow, Serialize)]
struct ApiKey {
    id: Uuid,
    scopes: Vec<String>,
    created_by: String,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

async fn list_keys(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<ApiKey>>, ApiError> {
    auth.actor()?;
    let sql =
        "SELECT id, scopes, created_by, created_at, last_used_at FROM api_keys WHERE org = $1 ORDER BY created_at";
    Ok(Json(sqlx::query_as(sql).bind(auth.org()).fetch_all(&state.store.pg).await?))
}

const SCOPES: [&str; 2] = ["tables", "notifications"];

#[derive(Deserialize)]
struct CreateKey {
    scopes: Vec<String>,
}

async fn create_key(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<CreateKey>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let me = auth.actor()?;
    if body.scopes.is_empty() || body.scopes.iter().any(|s| !SCOPES.contains(&s.as_str())) {
        return Err(ApiError::bad_request("invalid_scopes", "scopes are a non-empty subset of tables, notifications"));
    }
    let (id, key) = (Uuid::new_v4(), api_key());
    sqlx::query("INSERT INTO api_keys (id, org, key_hash, scopes, created_by) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(&me.org)
        .bind(sha256_hex(&key))
        .bind(&body.scopes)
        .bind(&me.id)
        .execute(&state.store.pg)
        .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id, "key": key}))))
}

async fn delete_key(
    State(state): State<AppState>,
    auth: Auth,
    Path((_, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    auth.actor()?;
    let deleted = sqlx::query("DELETE FROM api_keys WHERE org = $1 AND id = $2")
        .bind(auth.org())
        .bind(id)
        .execute(&state.store.pg)
        .await?;
    if deleted.rows_affected() == 0 { Err(ApiError::not_found("api key")) } else { Ok(StatusCode::NO_CONTENT) }
}
