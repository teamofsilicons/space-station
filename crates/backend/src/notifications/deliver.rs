//! Where a fired notification goes. The `notification` frame reaches every mission-control
//! socket the recipients name — that is what a `@carbon` gets, and a `@silicon` too when it has
//! one open — and the same event is POSTed to every webhook behind them: a silicon's own
//! delivery webhook, and any `webhook:{id}` of the org. The body is exactly
//! `{dedup_key, text, metadata}`, signed over `"{seconds}.{body}"`, tried at 0 s, 10 s and 60 s,
//! and only the last failure becomes a `dev_errors` row.

use std::time::Duration;

use chrono::Utc;
use futures_util::future::join_all;
use reqwest::Response;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::access::Entry;
use crate::http::AppState;
use crate::{crypto, dev_errors, webhooks};

/// Waits before each attempt, so they land at 0 s, 10 s and 60 s.
const WAITS: [u64; 3] = [0, 10, 50];
/// What we are willing to read back from a webhook; only its status matters.
const RESPONSE_MAX: usize = 1024 * 1024;

/// Sends `frame` to the recipients and returns at once: the retries are their own task, so a
/// slow webhook never holds up the notification's next run.
pub fn send(state: &AppState, org: &str, recipients: Vec<String>, frame: Value) {
    if recipients.is_empty() {
        return;
    }
    state.hub.notify(org, &recipients, &frame);
    let (state, org) = (state.clone(), org.to_owned());
    tokio::spawn(async move {
        match targets(&state, &org, &recipients).await {
            Ok(targets) => {
                join_all(targets.into_iter().map(|target| post(&state, &org, &frame, target))).await;
            }
            Err(e) => tracing::warn!("notification webhooks could not be read: {e}"),
        }
    });
}

/// The webhooks behind `recipients`: a silicon's own delivery webhook for `@silicon`, the org's
/// for `webhook:{id}`. A carbon has none — a carbon is the WS frame and the stored event.
async fn targets(state: &AppState, org: &str, recipients: &[String]) -> Result<Vec<(String, String)>, sqlx::Error> {
    let (mut actors, mut ids) = (Vec::new(), Vec::new());
    for entry in recipients {
        match Entry::parse(entry) {
            Entry::Actor(actor) => actors.push(actor.to_owned()),
            Entry::Webhook(id) => ids.extend(Uuid::parse_str(id).ok()),
            Entry::Tag(_) => {}
        }
    }
    let sql = "SELECT url, secret_enc FROM webhooks WHERE org = $1 AND (actor = ANY($2) OR id = ANY($3))";
    let rows: Vec<(String, String)> =
        sqlx::query_as(sql).bind(org).bind(&actors).bind(&ids).fetch_all(&state.store.pg).await?;
    Ok(rows
        .into_iter()
        .filter_map(|(url, sealed)| match crypto::open(&state.cfg.key, &sealed) {
            Some(secret) => Some((url, secret)),
            None => {
                tracing::error!("the webhook secret for {url} does not open under this SS_KEY");
                None
            }
        })
        .collect())
}

/// One webhook, up to three times; the final failure is the org's to see.
async fn post(state: &AppState, org: &str, frame: &Value, (url, secret): (String, String)) {
    let body = json!({"dedup_key": frame["dedup_key"], "text": frame["text"], "metadata": frame["metadata"]});
    let body = serde_json::to_vec(&body).unwrap_or_default();
    let mut failure = String::new();
    for wait in WAITS {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        match attempt(state, &url, &secret, frame, &body).await {
            Ok(()) => return,
            Err(e) => failure = e,
        }
    }
    let reference = frame["notification"].as_str().unwrap_or_default();
    let message = format!("the webhook at {url} did not accept the event: {failure}");
    dev_errors::insert(&state.store, org, "notification", reference, &message, frame.clone()).await;
}

/// One attempt: the URL is checked again at send time and the connect is pinned to what that
/// check resolved, so DNS cannot answer differently in between; the signature covers this second.
async fn attempt(state: &AppState, url: &str, secret: &str, frame: &Value, body: &[u8]) -> Result<(), String> {
    let vetted = webhooks::validate_url(url, state.cfg.allow_private_webhooks).await.map_err(|e| e.message)?;
    let seconds = Utc::now().timestamp();
    let sent = vetted
        .client()
        .post(&vetted.url)
        .header("content-type", "application/json")
        .header("x-space-station-event-id", frame["event_id"].to_string())
        .header("x-space-station-notification", frame["notification"].as_str().unwrap_or_default())
        .header("x-space-station-timestamp", seconds.to_string())
        .header("x-space-station-signature", webhooks::sign(secret, seconds, body))
        .body(body.to_vec())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = sent.status();
    drain(sent).await;
    if status.is_success() { Ok(()) } else { Err(format!("HTTP {status}")) }
}

/// Read at most `RESPONSE_MAX` of whatever it answered and drop the rest; only the status counts.
async fn drain(mut sent: Response) {
    let mut left = RESPONSE_MAX;
    while left > 0
        && let Ok(Some(chunk)) = sent.chunk().await
    {
        left = left.saturating_sub(chunk.len());
    }
}
