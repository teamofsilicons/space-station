//! Signed Silicon Accounts updates. Sign-out notices recheck each session independently.
use super::Kind;
use crate::{
    crypto,
    http::{ApiError, AppState},
};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
pub fn routes() -> Router<AppState> {
    Router::new().route("/accounts/webhook", post(receive))
}
#[derive(Deserialize)]
struct Event {
    event_id: Uuid,
    app_id: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    occurred_at: DateTime<Utc>,
    data: Value,
}
async fn receive(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Result<StatusCode, ApiError> {
    let get = |key: &str| headers.get(key).and_then(|v| v.to_str().ok());
    let timestamp = get("x-accounts-timestamp").ok_or_else(invalid)?;
    if timestamp.parse::<i64>().ok().is_none_or(|ts| Utc::now().timestamp().abs_diff(ts) > 300) {
        return Err(invalid());
    }
    let signature = get("x-accounts-signature").ok_or_else(invalid)?;
    let mut message = format!("{timestamp}.").into_bytes();
    message.extend_from_slice(&body);
    let valid = std::iter::once(state.cfg.accounts_webhook_secret.as_str())
        .chain(state.cfg.accounts_webhook_secret_previous.as_deref())
        .any(|secret| {
            let expected = crypto::hmac_sha256_hex(secret.as_bytes(), &message);
            signature
                .split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter_map(|v| v.strip_prefix("v1="))
                .any(|v| crypto::ct_eq(expected.as_bytes(), v.as_bytes()))
        });
    if !valid {
        return Err(invalid());
    }
    let event: Event = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("invalid_webhook", "invalid Accounts event"))?;
    if event.app_id.as_deref() != Some(&state.cfg.accounts_app_id)
        || get("x-accounts-event-id") != Some(event.event_id.to_string().as_str())
        || get("x-accounts-event-type") != Some(event.kind.as_str())
    {
        return Err(invalid());
    }
    let mut tx = state.store.pg.begin().await?;
    let inserted = sqlx::query("INSERT INTO account_events (event_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(event.event_id)
        .execute(&mut *tx)
        .await?;
    if inserted.rows_affected() == 0 {
        return Ok(StatusCode::NO_CONTENT);
    }
    if let Some(uuid) = event.data["uuid"].as_str().and_then(|v| Uuid::parse_str(v).ok()) {
        match event.kind.as_str() {
            "account.id_changed" | "account.updated" => {
                let id = event.data["new_id"].as_str().or(event.data["account"]["id"].as_str());
                if let Some(id) = id.filter(|v| Kind::of(v).is_some()) {
                    sqlx::query("UPDATE account_owners SET actor=$2,updated_at=$3 WHERE uuid=$1 AND updated_at<$3")
                        .bind(uuid)
                        .bind(id)
                        .bind(event.occurred_at)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            "membership.signed_out" => {
                // The event has no family ID. Introspection identifies exactly the revoked
                // browser or CLI session; signing one out must not erase its live siblings.
                sqlx::query("UPDATE account_sessions SET checked_at='epoch' WHERE account_uuid=$1")
                    .bind(uuid)
                    .execute(&mut *tx)
                    .await?;
            }
            "account.deleted" | "membership.access_removed" => {
                sqlx::query("DELETE FROM account_sessions WHERE account_uuid=$1 AND created_at<=$2")
                    .bind(uuid)
                    .bind(event.occurred_at)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE account_owners SET active=false,updated_at=$2 WHERE uuid=$1 AND updated_at<=$2")
                    .bind(uuid)
                    .bind(event.occurred_at)
                    .execute(&mut *tx)
                    .await?;
            }
            _ => {}
        }
    }
    tx.commit().await?;
    crate::notifications::engine::changed();
    Ok(StatusCode::NO_CONTENT)
}
fn invalid() -> ApiError {
    ApiError::unauthorized("invalid_webhook_signature", "invalid Silicon Accounts webhook signature or recipient")
}
