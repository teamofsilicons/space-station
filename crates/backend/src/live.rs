//! `/ws/mission-control?org=`: the socket a processor host keeps open. Authenticated at upgrade
//! (a cookie under the Origin rule, or an `sscli-`/`spacewindow-` bearer) and re-checked every 60 s;
//! `subscribe` registers triggers and answers watermarks, hits become `trigger` frames, `state`
//! stores a processor's SiliconJSON for the current version, and `Hub` is how notifications reach
//! every socket whose actor they name.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::any;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use space_station_shared::limits::SILICON_JSON_MAX;
use tokio::sync::mpsc::{self, UnboundedSender};
use uuid::Uuid;

use crate::http::auth::{Principal, authenticate};
use crate::http::{ApiError, AppState, Query};
use crate::iam::{CACHE_TTL, Identity};
use crate::triggers::{Hit, Registration};
use crate::{access, lock, sql};

const PING_EVERY: Duration = Duration::from_secs(20);
/// Mission-control frames contain triggers and at most a 64 KiB SiliconJSON payload.
/// Bound both message and frame sizes so idle unauthenticated sockets cannot reserve tens of MiB.
/// Keep a 2 MiB allowance for subscriptions containing several SQL trigger predicates.
const WS_MAX_MESSAGE: usize = 2 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/mission-control", any(upgrade))
}

/// Every open socket, so notifications can find their recipients.
#[derive(Default)]
pub struct Hub {
    sockets: Mutex<HashMap<u64, Socket>>,
    next: AtomicU64,
}

struct Socket {
    org: String,
    identity: Identity,
    tx: UnboundedSender<String>,
}

impl Hub {
    /// `frame` to every socket in `org` whose actor the recipients name.
    pub fn notify(&self, org: &str, recipients: &[String], frame: &Value) {
        let text = frame.to_string();
        for socket in lock(&self.sockets).values() {
            if socket.org == org && access::matches(&socket.identity, recipients) {
                let _ = socket.tx.send(text.clone());
            }
        }
    }

    fn attach(&self, org: &str, identity: Identity, tx: UnboundedSender<String>) -> u64 {
        let id = self.next.fetch_add(1, Relaxed);
        lock(&self.sockets).insert(id, Socket { org: org.to_owned(), identity, tx });
        id
    }
}

/// Detaches its socket from the hub when the session ends, however it ends.
struct Attached<'a>(&'a AppState, u64);

impl Drop for Attached<'_> {
    fn drop(&mut self) {
        lock(&self.0.hub.sockets).remove(&self.1);
    }
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let org = q.get("org").cloned().unwrap_or_default();
    ws.max_message_size(WS_MAX_MESSAGE)
        .max_frame_size(WS_MAX_MESSAGE)
        .on_upgrade(move |socket| session(socket, state, headers, org))
}

/// 4401 and 4403 mean "this credential, this actor": a client that reconnects with the same one
/// gets the same answer, so the runtime stops. A failure on our side is a plain 1011 instead,
/// because reconnecting is exactly the right thing to do after a database or IAM blip.
async fn close(mut socket: WebSocket, e: ApiError) {
    let code = match e.status {
        StatusCode::FORBIDDEN => 4403,
        status if status.is_server_error() => 1011,
        _ => 4401,
    };
    let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: e.message.into() }))).await;
}

/// The credential on the upgrade, resolved again; anything but an actor ends the socket.
async fn actor(state: &AppState, headers: &HeaderMap, org: &str) -> Result<Identity, ApiError> {
    match authenticate(state, headers, org, true).await? {
        Principal::Actor(identity) => Ok(identity),
        Principal::Org { .. } => Err(ApiError::unauthorized("unauthorized", "API keys cannot open mission control")),
    }
}

struct Sub {
    triggers: Vec<Trigger>,
    _registrations: Vec<Registration>,
}

async fn session(mut socket: WebSocket, state: AppState, headers: HeaderMap, org: String) {
    let mut identity = match actor(&state, &headers, &org).await {
        Ok(identity) => identity,
        Err(e) => return close(socket, e).await,
    };
    let (tx, mut frames) = mpsc::unbounded_channel::<String>();
    let (hit_tx, mut hits) = mpsc::unbounded_channel::<Hit>();
    let attached = Attached(&state, state.hub.attach(&org, identity.clone(), tx.clone()));
    let mut subs: HashMap<String, Sub> = HashMap::new();
    let mut validated = Instant::now();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    let mut awaiting_pong = false;
    let mut stop = state.stop.clone();
    loop {
        let reply = tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    if validated.elapsed() >= CACHE_TTL {
                        match actor(&state, &headers, &org).await {
                            Ok(fresh) => {
                                identity = fresh;
                                validated = Instant::now();
                                if let Some(s) = lock(&state.hub.sockets).get_mut(&attached.1) {
                                    s.identity = identity.clone();
                                }
                            }
                            Err(e) => return close(socket, e).await,
                        }
                    }
                    handle(&state, &identity, &text, &mut subs, &hit_tx).await
                }
                Some(Ok(Message::Pong(_))) => {
                    awaiting_pong = false;
                    None
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => None,
            },
            Some(hit) = hits.recv() => {
                let fired = |s: &Sub| s.triggers.iter().any(|t| t.table == hit.table && t.where_ == hit.where_);
                for (id, _) in subs.iter().filter(|(_, s)| fired(s)) {
                    let _ = tx.send(json!({"type": "trigger", "id": id}).to_string());
                }
                None
            }
            Some(frame) = frames.recv() => {
                if socket.send(Message::Text(frame.into())).await.is_err() {
                    return;
                }
                None
            }
            _ = ping.tick() => {
                if awaiting_pong || socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
                awaiting_pong = true;
                None
            }
            _ = stop.changed() => break,
        };
        if let Some(reply) = reply
            && socket.send(Message::Text(reply.to_string().into())).await.is_err()
        {
            return;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

/// One frame from the client; which fields matter depends on `type`.
#[derive(Deserialize)]
struct Frame<'a> {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    triggers: Option<Vec<Trigger>>,
    window: Option<Uuid>,
    version: Option<String>,
    #[serde(borrow)]
    json: Option<&'a RawValue>,
}

#[derive(Deserialize, Clone)]
struct Trigger {
    table: String,
    #[serde(rename = "where")]
    where_: Option<String>,
}

/// An `error` frame. `id` echoes the frame that failed, when it carried one, so a client can tell
/// which subscription was refused rather than only that something was.
fn error(id: Option<String>, code: &str, message: impl Into<String>) -> Option<Value> {
    Some(json!({"type": "error", "id": id, "code": code, "message": message.into()}))
}

/// The reply frame to one client frame, if any.
async fn handle(
    state: &AppState,
    me: &Identity,
    text: &str,
    subs: &mut HashMap<String, Sub>,
    hits: &UnboundedSender<Hit>,
) -> Option<Value> {
    let frame = match serde_json::from_str::<Frame>(text) {
        Ok(frame) => frame,
        Err(e) => return error(None, "bad_frame", e.to_string()),
    };
    let echo = frame.id.clone();
    let result = match (frame.kind.as_str(), frame.id, frame.triggers, frame.window) {
        ("subscribe", Some(id), Some(triggers), _) => subscribe(state, me, id, triggers, subs, hits).await,
        ("unsubscribe", Some(id), _, _) => {
            subs.remove(&id);
            Ok(None)
        }
        ("state", _, _, Some(window)) => store_state(state, me, window, frame.version, frame.json).await,
        (kind, ..) => return error(echo, "bad_frame", format!("{kind:?} is not a frame this socket understands")),
    };
    result.unwrap_or_else(|e| error(echo, &e.code, e.message))
}

async fn subscribe(
    state: &AppState,
    me: &Identity,
    id: String,
    triggers: Vec<Trigger>,
    subs: &mut HashMap<String, Sub>,
    hits: &UnboundedSender<Hit>,
) -> Result<Option<Value>, ApiError> {
    let visible = access::visible_tables(state, me).await?;
    for t in &triggers {
        if !visible.contains(&t.table) {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "forbidden", format!("table {} is not visible", t.table)));
        }
        // Nobody vetted this `where` at save time the way a notification's was, so it is planned
        // under the subscriber's own visibility: a subquery in it reads no table they cannot.
        if let Some(where_) = &t.where_ {
            sql::trigger_plan(&t.table, Some(where_), &|table| visible.iter().any(|v| v == table))
                .map(drop)
                .map_err(|e| ApiError::bad_request("invalid_trigger", e.to_string()))?;
        }
    }
    let (mut registrations, mut watermarks) = (Vec::new(), serde_json::Map::new());
    for t in &triggers {
        registrations.push(state.triggers.register(&me.org, &t.table, t.where_.as_deref(), hits.clone()));
        watermarks.insert(t.table.clone(), state.store.watermarks.get(&me.org, &t.table).await?.into());
    }
    subs.insert(id.clone(), Sub { triggers, _registrations: registrations });
    Ok(Some(json!({"type": "subscribed", "id": id, "watermarks": watermarks})))
}

/// Stores SiliconJSON for the window's current version; `version: null` is dev code, accepted
/// and forgotten. `json` absent means unchanged: only the version and `produced_at` move.
async fn store_state(
    state: &AppState,
    me: &Identity,
    window: Uuid,
    version: Option<String>,
    json: Option<&RawValue>,
) -> Result<Option<Value>, ApiError> {
    let sql = "SELECT w.access, v.name FROM windows w LEFT JOIN window_versions v ON v.id = w.current_version WHERE w.id = $1 AND w.org = $2";
    let row: Option<(Value, Option<String>)> =
        sqlx::query_as(sql).bind(window).bind(&me.org).fetch_optional(&state.store.pg).await?;
    let (access, current) = row.ok_or_else(ApiError::forbidden)?;
    if !access::matches(me, &access::list(&access)) {
        return Err(ApiError::forbidden());
    }
    let Some(version) = version else {
        return Ok(None);
    };
    if current.as_deref() != Some(version.as_str()) {
        return Err(ApiError::bad_request("version_not_current", format!("the current version is {current:?}")));
    }
    if json.is_some_and(|j| j.get().len() > SILICON_JSON_MAX) {
        return Err(ApiError::bad_request("too_large", format!("SiliconJSON is over {SILICON_JSON_MAX} bytes")));
    }
    let json: Option<Value> = json
        .map(|j| serde_json::from_str(j.get()))
        .transpose()
        .map_err(|e| ApiError::bad_request("bad_frame", e.to_string()))?;
    sqlx::query(
        "UPDATE windows SET state = COALESCE($2, state), state_version = $3, produced_at = now() WHERE id = $1",
    )
    .bind(window)
    .bind(json)
    .bind(version)
    .execute(&state.store.pg)
    .await?;
    Ok(None)
}
