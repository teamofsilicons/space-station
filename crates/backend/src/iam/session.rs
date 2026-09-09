//! Sessions. A session secret is 32 random bytes; the row is keyed by their sha256, bound to one
//! org, and holds the Application's `oat_`/`ort_` sealed under `SS_KEY`. Three doors open one:
//! `GET /auth/login` sends a browser to IAM and `GET /auth/callback` exchanges the `slt` IAM brings
//! back into a cookie (ending the row the browser's previous cookie named, so a re-login or an org
//! switch leaves nothing behind) — or, when `cli=` asked, forwards the slt itself to the terminal's
//! loopback listener, which spends it at `POST /auth/session` like an `slt` the `iam` CLI minted.
//! Opening a session introspects the fresh token: it proves the actor, org and membership, and its
//! `authorization` snapshot seeds the row's tags and the directory mirror. `load` is every later
//! request: the refresh under the row lock when the access token is about to expire, and a re-proof
//! at most once a minute — which refreshes first when IAM reports the held token inactive, because
//! a revoked sibling family or a tag change can flip it while the family is still good. `POST
//! /auth/logout` ends whichever row is presented.

use std::collections::HashMap;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use space_station_shared::secrets::sha256_hex;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Postgres, Row, Transaction};
use url::form_urlencoded;
use uuid::Uuid;

use super::Kind;
use super::client::{Authorization, Introspection, Tokens};
use crate::http::auth::{bad_origin, bearer, invalid_org, origin_ok};
use crate::http::{ApiError, AppState, Json, Query};
use crate::sql::valid_org;
use crate::{access, crypto, iam};

const COOKIE: &str = "ss_session";
const LOGIN_COOKIE: &str = "ss_login";
/// Refresh when less than this is left on the access token.
const REFRESH_MARGIN: TimeDelta = TimeDelta::seconds(60);
/// Introspect again when the last proof is older than this.
const CHECK_EVERY: TimeDelta = TimeDelta::seconds(60);
const COLUMNS: &str = "actor, kind, org, membership_id, oat_enc, ort_enc, expires_at, refresh_key, checked_at, tags";
/// What marks a session secret as a terminal's, presented as `Authorization: Bearer`.
pub const CLI_PREFIX: &str = "sscli-";
/// A login `state` is a nonce, so it fits in the login cookie and in a redirect header as it is.
const STATE_MAX: usize = 128;

/// A session row, decrypted. `tags` is the snapshot's tags for this session — `None` until a
/// snapshot discloses them (Identity then falls back to the mirror), `Some([])` when the member
/// has none.
#[derive(Clone)]
pub struct Session {
    pub id_hash: String,
    pub actor: String,
    pub kind: Kind,
    pub org: String,
    pub membership_id: Option<String>,
    pub oat: String,
    pub ort: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_key: Option<Uuid>,
    pub checked_at: DateTime<Utc>,
    pub tags: Option<Vec<String>>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/session", post(session))
        .route("/auth/logout", post(logout))
}

/// The `ss_session` cookie on this request, if any.
pub fn cookie_value(headers: &HeaderMap) -> Option<String> {
    read_cookie(headers, COOKIE)
}

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let pairs = headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';'));
    pairs.filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v.to_owned())
}

/// HttpOnly, SameSite=Lax, Secure when the origin is https; `max_age` 0 deletes.
fn set_cookie(
    state: &AppState,
    name: &str,
    value: &str,
    path: &str,
    max_age: u32,
) -> (header::HeaderName, HeaderValue) {
    cookie_header(&state.cfg.origin, state.cfg.cookie_domain.as_deref(), name, value, path, max_age)
}

fn cookie_header(
    origin: &str,
    cookie_domain: Option<&str>,
    name: &str,
    value: &str,
    path: &str,
    max_age: u32,
) -> (header::HeaderName, HeaderValue) {
    let secure = if origin.starts_with("https://") { "; Secure" } else { "" };
    // Browser login may start at the API hostname (the CLI's default) and return through the
    // frontend proxy. Both cookies must span those two hosts; login state is confined to /api/auth.
    let domain = cookie_domain.map(|d| format!("; Domain={d}")).unwrap_or_default();
    let cookie = format!("{name}={value}; Max-Age={max_age}; Path={path}; HttpOnly; SameSite=Lax{secure}{domain}");
    (header::SET_COOKIE, HeaderValue::from_str(&cookie).expect("cookie values are ASCII"))
}

fn callback_url(state: &AppState) -> String {
    format!("{}/api/auth/callback", state.cfg.origin)
}

fn expired() -> ApiError {
    ApiError::unauthorized("session_expired", "log in again")
}

/// The first refusal after a live session: help a human tell "someone signed in again as me" from
/// "my session simply aged out". Same code, so callers still branch on `session_expired`.
fn ended_elsewhere() -> ApiError {
    ApiError::unauthorized(
        "session_expired",
        "IAM ended this session (signed in again elsewhere as this actor, or revoked); sign in again",
    )
}

/// IAM issued an org-bound token but does not agree with itself about it: discard and refuse.
fn inconsistent() -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, "iam_inconsistent", "IAM does not recognise the session it just issued")
}

/// What a login asked for, carried through IAM in the sealed login cookie: the org the session
/// will be bound to and where to land. `cli` is a bare port number and never a URL, so loopback is
/// the only reachable redirect target; `state` is the nonce that listener will check.
#[derive(Debug, Serialize, Deserialize)]
struct Landing {
    org: Option<String>,
    next: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cli: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
}

impl Landing {
    /// The query as a landing, or the one clear reason it is not one.
    fn read(q: &HashMap<String, String>) -> Result<Landing, ApiError> {
        let org = q.get("org").filter(|o| valid_org(o)).cloned();
        // A same-origin path that fits in a Location header; anything else lands on `/`.
        let path = |n: &&String| n.starts_with('/') && !n.starts_with("//") && n.len() <= 1024;
        let next = q.get("next").filter(path).filter(|n| n.bytes().all(|b| b.is_ascii_graphic()));
        let port = |p: &String| p.parse::<u16>().ok().filter(|p| *p >= 1024);
        let cli = match q.get("cli") {
            Some(cli) => Some(port(cli).ok_or_else(|| {
                ApiError::bad_request("invalid_cli", "cli is a loopback port number between 1024 and 65535")
            })?),
            None => None,
        };
        // A nonce, so it survives a cookie and a redirect header untouched and needs no escaping.
        let nonce = |s: &&String| {
            (1..=STATE_MAX).contains(&s.len())
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
        };
        let state = match q.get("state") {
            Some(s) if !nonce(&s) => {
                let message = format!("state is 1 to {STATE_MAX} characters of [A-Za-z0-9._~-]");
                return Err(ApiError::bad_request("invalid_state", message));
            }
            other => other.cloned(),
        };
        Ok(Landing { org: org.clone(), next: next.cloned().unwrap_or_else(|| "/".into()), cli, state })
    }
}

/// Sends the browser to IAM to sign in on this Application's behalf, bound to `org`; the landing
/// rides in a ten-minute cookie sealed under `SS_KEY`, so the callback trusts nothing in the URL.
async fn login(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Result<Response, ApiError> {
    let landing = Landing::read(&q)?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("app_id", &state.cfg.iam_app_id);
    query.append_pair("redirect_uri", &callback_url(&state));
    let url = format!("{}/api/v1/login?{}", state.cfg.iam_url, query.finish());
    let location = HeaderValue::from_str(&url).map_err(|e| ApiError::internal("login_redirect", e))?;
    let sealed = crypto::seal(&state.cfg.key, &json!(landing).to_string());
    let cookie = set_cookie(&state, LOGIN_COOKIE, &sealed, "/api/auth", 600);
    Ok((StatusCode::FOUND, [cookie, (header::LOCATION, location)]).into_response())
}

/// IAM brings the browser back with `?slt=`. A browser login exchanges it, ends the row the
/// request's own `ss_session` cookie still named — a re-login or an org switch replaces a session
/// rather than piling one on it — and lands on `next`. A terminal login (`cli=`) exchanges
/// nothing: the slt is forwarded to the loopback listener, which spends it at `POST /auth/session`
/// itself, so what crosses the browser (and its history) is a token IAM minted single-use for two
/// minutes, never the long-lived `sscli-` secret.
async fn callback(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let landing: Landing = read_cookie(&headers, LOGIN_COOKIE)
        .and_then(|sealed| crypto::open(&state.cfg.key, &sealed))
        .and_then(|json| serde_json::from_str(&json).ok())
        .ok_or_else(|| ApiError::unauthorized("login_expired", "start again at /api/auth/login"))?;
    let slt = q
        .get("slt")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("slt_required", "IAM did not bring back a short-lived token"))?;
    let done = set_cookie(&state, LOGIN_COOKIE, "", "/api/auth", 0);
    if let Some(port) = landing.cli {
        // RFC 8252 loopback: `port` is a bare number, so 127.0.0.1 is the only reachable target.
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.append_pair("slt", slt);
        if let Some(nonce) = &landing.state {
            query.append_pair("state", nonce);
        }
        let listener = format!("http://127.0.0.1:{port}/?{}", query.finish());
        return Ok(([done], Redirect::to(&listener)).into_response());
    }
    let id = open(&state, slt, landing.org.as_deref(), false).await?;
    if let Some(previous) = cookie_value(&headers) {
        end(&state, &previous).await?;
    }
    let cookies = [set_cookie(&state, COOKIE, &id, "/", 30 * 86_400), done];
    let next = format!("{}{}", state.cfg.origin, landing.next);
    Ok((AppendHeaders(cookies), Redirect::to(&next)).into_response())
}

#[derive(Deserialize)]
struct Exchange {
    slt: String,
    org: Option<String>,
}

/// A terminal hands over the slt it minted with the `iam` CLI and gets an `sscli-` bearer on a
/// row of its own.
async fn session(State(state): State<AppState>, Json(body): Json<Exchange>) -> Result<Json<Value>, ApiError> {
    if body.org.as_deref().is_some_and(|o| !valid_org(o)) {
        return Err(invalid_org());
    }
    Ok(Json(json!({"token": open(&state, &body.slt, body.org.as_deref(), true).await?})))
}

/// Exchanges `slt`, proves the membership and reads the snapshot once, and writes the row and the
/// mirror in one transaction. The secret handed back is the cookie value, or an `sscli-` bearer
/// when `cli`.
async fn open(state: &AppState, slt: &str, requested_org: Option<&str>, cli: bool) -> Result<String, ApiError> {
    let tokens = state.iam.exchange(slt).await.map_err(|e| match e.is_invalid_grant() {
        true => ApiError::unauthorized(
            "invalid_slt",
            "IAM refused the short-lived token: it is single-use and lives two minutes",
        ),
        false => e.into(),
    })?;
    let org = tokens.org.as_deref().or(requested_org).ok_or_else(|| ApiError::bad_request("org_required", "IAM did not select an organization"))?;
    if let Err(e) = bound_to(&tokens, org) {
        discard(state, &tokens).await;
        return Err(e);
    }
    // The freshly minted family is proved before it is stored; every refusal below discards it, so
    // a family this server will never use is not left alive at IAM.
    let proof = match state.iam.introspect(&tokens.oat).await {
        Ok(proof) => proof,
        Err(e) => {
            discard(state, &tokens).await;
            return Err(e.into());
        }
    };
    if let Err(e) = agrees(&proof, org, proof.membership_id.as_deref()) {
        discard(state, &tokens).await;
        tracing::warn!("IAM: a token just minted for @{} bound to {org} introspects as {proof:?}", tokens.actor);
        return Err(e);
    }
    let secret = URL_SAFE_NO_PAD.encode(crypto::random::<32>());
    let id = if cli { format!("{CLI_PREFIX}{secret}") } else { secret };
    let tags = proof.authorization.as_ref().and_then(|a| a.tags.clone());
    let mut tx = state.store.pg.begin().await?;
    sqlx::query(
        "INSERT INTO sessions (id_hash, actor, kind, org, membership_id, oat_enc, ort_enc, expires_at, tags) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(sha256_hex(&id))
    .bind(&tokens.actor)
    .bind(Kind::of(&tokens.actor).as_str())
    .bind(org)
    .bind(&proof.membership_id)
    .bind(crypto::seal(&state.cfg.key, &tokens.oat))
    .bind(crypto::seal(&state.cfg.key, &tokens.ort))
    .bind(Utc::now() + TimeDelta::seconds(tokens.expires_in))
    .bind(tags.as_ref().map(|t| Value::from(t.clone())))
    .execute(&mut *tx)
    .await?;
    if let Some(auth) = &proof.authorization {
        mirror(&mut tx, org, &tokens.actor, auth).await?;
    }
    tx.commit().await?;
    if proof.authorization.is_some() {
        // The snapshot changed what this actor can see; drop the cached visibility so it is re-read.
        access::forget(state, org);
    }
    Ok(id)
}

/// The exchange must have been bound to `org`: everything here lives inside one, and a session
/// speaks for exactly the org its login named.
fn bound_to(tokens: &Tokens, org: &str) -> Result<(), ApiError> {
    match tokens.org.as_deref() {
        Some(bound) if bound == org => Ok(()),
        Some(bound) => Err(ApiError::bad_request("org_mismatch", format!("the login was bound to {bound}, not {org}"))),
        None => Err(ApiError::bad_request(
            "org_required",
            "the login was not bound to an organization; start it with ?org= or `iam login --org`",
        )),
    }
}

/// Introspection proves the token is live, bound to `org`, and holds `membership`; a present
/// snapshot must name the same org and membership. `membership` is `None` only for the initial
/// exchange, whose membership introspection itself supplies.
fn agrees(proof: &Introspection, org: &str, membership: Option<&str>) -> Result<(), ApiError> {
    if !proof.active || proof.org.as_deref() != Some(org) {
        return Err(inconsistent());
    }
    if let Some(want) = membership
        && proof.membership_id.as_deref() != Some(want)
    {
        return Err(inconsistent());
    }
    if let Some(auth) = &proof.authorization
        && (auth.org != org || Some(auth.membership_id.as_str()) != proof.membership_id.as_deref())
    {
        return Err(inconsistent());
    }
    Ok(())
}

/// Upserts the directory mirror from a snapshot: the same version-ordered write the webhook
/// receiver uses, so a `spacewindow-` token resolves this member's tags right after login and a
/// removal tombstone finds this membership by its ids.
async fn mirror(tx: &mut PgConnection, org: &str, actor: &str, auth: &Authorization) -> sqlx::Result<()> {
    let member = iam::webhook::Member {
        org: org.to_owned(),
        org_uuid: Some(auth.org_uuid.clone()),
        org_name: None,
        actor: actor.to_owned(),
        kind: Kind::of(actor),
        membership_id: auth.membership_id.clone(),
        principal_id: Some(auth.principal_id.clone()),
        status: "active".into(),
        tags: auth.tags.clone(),
        version: auth.membership_version,
    };
    iam::webhook::upsert(tx, &member).await
}

/// A token family this server will never use is not left alive at IAM.
async fn discard(state: &AppState, tokens: &Tokens) {
    if let Err(e) = state.iam.revoke(&tokens.ort).await {
        tracing::info!("discarding an unused token family: {e:?}");
    }
}

/// Ends the session presented: an `sscli-` bearer ends the terminal's own row and leaves the
/// browser signed in (no browser attaches a bearer, so the Origin rule does not apply); the
/// cookie ends the browser's under that rule and is cleared.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    if let Some(token) = bearer(&headers) {
        if !token.starts_with(CLI_PREFIX) {
            return Err(ApiError::unauthorized(
                "unsupported_bearer",
                "only an sscli- bearer or the cookie can log out",
            ));
        }
        end(&state, token).await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    if !origin_ok(&headers, &state.cfg.origin) {
        return Err(bad_origin());
    }
    if let Some(cookie) = cookie_value(&headers) {
        end(&state, &cookie).await?;
    }
    Ok(([set_cookie(&state, COOKIE, "", "/", 0)], StatusCode::NO_CONTENT).into_response())
}

/// Deletes the session behind `secret`, if there is one, and, best effort, its token family at IAM.
async fn end(state: &AppState, secret: &str) -> Result<(), ApiError> {
    let Some(session) = fetch(state, &sha256_hex(secret)).await? else {
        return Ok(());
    };
    sqlx::query("DELETE FROM sessions WHERE id_hash = $1").bind(&session.id_hash).execute(&state.store.pg).await?;
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = state.iam.revoke(&session.ort).await {
            tracing::info!("revoking a refresh token at logout: {e:?}");
        }
    });
    Ok(())
}

/// The session behind a cookie value or an `sscli-` bearer, refreshed when its access token is
/// about to expire and re-proved when its last proof is old; `None` if unknown.
pub async fn load(state: &AppState, secret: &str) -> Result<Option<Session>, ApiError> {
    let Some(mut session) = fetch(state, &sha256_hex(secret)).await? else {
        return Ok(None);
    };
    if session.expires_at - Utc::now() < REFRESH_MARGIN {
        session = refresh(state, &session).await?;
    }
    if Utc::now() - session.checked_at >= CHECK_EVERY {
        recheck(state, &mut session).await?;
    }
    Ok(Some(session))
}

async fn fetch(state: &AppState, id_hash: &str) -> Result<Option<Session>, ApiError> {
    let sql = format!("SELECT {COLUMNS} FROM sessions WHERE id_hash = $1");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id_hash).fetch_optional(&state.store.pg).await?;
    Ok(row.and_then(|r| decode(state, id_hash, &r)))
}

/// The same read under a `FOR UPDATE` lock, inside a transaction.
async fn fetch_locked(state: &AppState, tx: &mut PgConnection, id_hash: &str) -> Result<Session, ApiError> {
    let sql = format!("SELECT {COLUMNS} FROM sessions WHERE id_hash = $1 FOR UPDATE");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id_hash).fetch_optional(&mut *tx).await?;
    row.and_then(|r| decode(state, id_hash, &r)).ok_or_else(expired)
}

/// `None` when the tokens were sealed under another key: the session is unusable.
fn decode(state: &AppState, id_hash: &str, row: &PgRow) -> Option<Session> {
    let open = |column| crypto::open(&state.cfg.key, &row.get::<String, _>(column));
    let kind = if row.get::<String, _>("kind") == "silicon" { Kind::Silicon } else { Kind::Carbon };
    Some(Session {
        id_hash: id_hash.to_owned(),
        actor: row.get("actor"),
        kind,
        org: row.get("org"),
        membership_id: row.get("membership_id"),
        oat: open("oat_enc")?,
        ort: open("ort_enc")?,
        expires_at: row.get("expires_at"),
        refresh_key: row.get("refresh_key"),
        checked_at: row.get("checked_at"),
        tags: row.get::<Option<Value>, _>("tags").map(|v| access::list(&v)),
    })
}

/// The refresh, under the row lock. The row's idempotency key is reused, or minted and written
/// before the request, so IAM sees one key per `ort_` however many times we have to retry.
pub async fn refresh(state: &AppState, seen: &Session) -> Result<Session, ApiError> {
    let mut tx = state.store.pg.begin().await?;
    let current = fetch_locked(state, &mut tx, &seen.id_hash).await?;
    if current.oat != seen.oat {
        tx.commit().await?;
        return Ok(current);
    }
    let outcome = rotate(state, &mut tx, &current).await;
    // The commit persists whichever of rotate's writes happened: the new pair, the deletion of a
    // revoked family, or just the minted idempotency key that the next attempt must reuse.
    tx.commit().await?;
    outcome
}

/// Rotates `current`'s pair against IAM, writing the result into the locked row. Success swaps the
/// tokens; `400 invalid_grant` is the family revoked and the row deleted; anything else keeps the
/// row (and the minted key) for the next attempt. The caller owns the commit.
async fn rotate(state: &AppState, tx: &mut PgConnection, current: &Session) -> Result<Session, ApiError> {
    let key = match current.refresh_key {
        Some(key) => key,
        None => {
            let key = Uuid::new_v4();
            sqlx::query("UPDATE sessions SET refresh_key = $2 WHERE id_hash = $1")
                .bind(&current.id_hash)
                .bind(key)
                .execute(&mut *tx)
                .await?;
            key
        }
    };
    match state.iam.refresh(&current.ort, &key.to_string()).await {
        Ok(tokens) => {
            let expires_at = Utc::now() + TimeDelta::seconds(tokens.expires_in);
            sqlx::query(
                "UPDATE sessions SET oat_enc = $2, ort_enc = $3, expires_at = $4, refresh_key = NULL WHERE id_hash = $1",
            )
            .bind(&current.id_hash)
            .bind(crypto::seal(&state.cfg.key, &tokens.oat))
            .bind(crypto::seal(&state.cfg.key, &tokens.ort))
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
            Ok(Session { oat: tokens.oat, ort: tokens.ort, expires_at, refresh_key: None, ..current.clone() })
        }
        Err(e) if e.is_invalid_grant() => {
            sqlx::query("DELETE FROM sessions WHERE id_hash = $1").bind(&current.id_hash).execute(&mut *tx).await?;
            tracing::info!("session of @{} in {} ended: IAM refused the refresh for good", current.actor, current.org);
            Err(ended_elsewhere())
        }
        Err(e) => Err(e.into()),
    }
}

/// Re-proves what the exchange proved. IAM answers `active: false` for a still-good family in two
/// benign cases — a revoked sibling of the same login, and a tag or role change that bumped the
/// authorization epoch — so an inactive answer refreshes first and only ends the session when the
/// refresh is refused or the new token proves a different actor. A live token that already proves
/// the same membership just refreshes its tags. The row is locked so N concurrent requests make one
/// introspection: whoever loses the lock adopts the fresher proof instead of asking again.
async fn recheck(state: &AppState, session: &mut Session) -> Result<(), ApiError> {
    let mut tx = state.store.pg.begin().await?;
    let current = fetch_locked(state, &mut tx, &session.id_hash).await?;
    if Utc::now() - current.checked_at < CHECK_EVERY {
        tx.commit().await?;
        *session = current;
        return Ok(());
    }
    let proof = match state.iam.introspect(&current.oat).await {
        Ok(proof) => proof,
        Err(e) => {
            tx.rollback().await.ok();
            return Err(e.into());
        }
    };
    if proof.active {
        return finish(state, tx, current, proof, session).await;
    }
    // Inactive: refresh first (deletes the row on a real revocation), then re-prove the new token.
    let refreshed = match rotate(state, &mut tx, &current).await {
        Ok(refreshed) => refreshed,
        Err(e) => {
            tx.commit().await?;
            return Err(e);
        }
    };
    let proof = match state.iam.introspect(&refreshed.oat).await {
        Ok(proof) => proof,
        Err(e) => {
            // Keep the rotated pair; the next request retries the re-proof.
            tx.commit().await?;
            return Err(e.into());
        }
    };
    finish(state, tx, refreshed, proof, session).await
}

/// Applies a fresh introspection to the locked `proved` session and commits: on agreement it
/// stores the snapshot's tags and mirror row and stamps `checked_at`; on disagreement it deletes
/// the row and ends the session.
async fn finish(
    state: &AppState,
    mut tx: Transaction<'_, Postgres>,
    proved: Session,
    proof: Introspection,
    session: &mut Session,
) -> Result<(), ApiError> {
    if agrees(&proof, &proved.org, proved.membership_id.as_deref()).is_err() {
        sqlx::query("DELETE FROM sessions WHERE id_hash = $1").bind(&proved.id_hash).execute(&mut *tx).await?;
        tx.commit().await?;
        tracing::info!("session of @{} in {} ended: introspection answered {proof:?}", proved.actor, proved.org);
        return Err(ended_elsewhere());
    }
    let tags = match &proof.authorization {
        Some(auth) => {
            mirror(&mut tx, &proved.org, &proved.actor, auth).await?;
            if let Some(tags) = &auth.tags {
                sqlx::query("UPDATE sessions SET tags = $2 WHERE id_hash = $1")
                    .bind(&proved.id_hash)
                    .bind(Value::from(tags.clone()))
                    .execute(&mut *tx)
                    .await?;
            }
            auth.tags.clone()
        }
        None => None,
    };
    sqlx::query("UPDATE sessions SET checked_at = now() WHERE id_hash = $1")
        .bind(&proved.id_hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if proof.authorization.is_some() {
        access::forget(state, &proved.org);
    }
    *session = Session { checked_at: Utc::now(), tags: tags.or(proved.tags.clone()), ..proved };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_session_crosses_to_websocket_host_and_logout_clears_the_same_cookie() {
        let origin = "https://spacestation.teamofsilicons.com";
        let domain = Some("spacestation.teamofsilicons.com");
        let (_, session) = cookie_header(origin, domain, COOKIE, "secret", "/", 3600);
        assert_eq!(
            session.to_str().unwrap(),
            "ss_session=secret; Max-Age=3600; Path=/; HttpOnly; SameSite=Lax; Secure; Domain=spacestation.teamofsilicons.com"
        );
        let (_, deleted) = cookie_header(origin, domain, COOKIE, "", "/", 0);
        assert_eq!(
            deleted.to_str().unwrap(),
            "ss_session=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax; Secure; Domain=spacestation.teamofsilicons.com"
        );
        let (_, login) = cookie_header(origin, domain, LOGIN_COOKIE, "state", "/api/auth", 600);
        assert!(login.to_str().unwrap().contains("Domain=spacestation.teamofsilicons.com"));
        assert!(login.to_str().unwrap().contains("Path=/api/auth"));
        let (_, local) = cookie_header("http://localhost:3000", None, COOKIE, "local", "/", 3600);
        assert!(!local.to_str().unwrap().contains("Secure"));
        assert!(!local.to_str().unwrap().contains("Domain="));
    }

    fn tokens(org: Option<&str>) -> Tokens {
        Tokens {
            oat: "oat_x".into(),
            ort: "ort_x".into(),
            expires_in: 1800,
            actor: "alice".into(),
            org: org.map(str::to_owned),
        }
    }

    fn proof(active: bool, org: Option<&str>, membership: Option<&str>, auth: Option<Authorization>) -> Introspection {
        Introspection {
            active,
            org: org.map(str::to_owned),
            membership_id: membership.map(str::to_owned),
            authorization: auth,
        }
    }

    fn snapshot(org: &str, membership: &str, tags: Option<Vec<String>>) -> Authorization {
        Authorization {
            public_id: "bot:tos".into(),
            principal_id: "01a0-principal".into(),
            org: org.into(),
            org_uuid: "01a0-org".into(),
            membership_id: membership.into(),
            membership_version: 4,
            tags,
        }
    }

    #[test]
    fn a_session_is_bound_to_exactly_the_org_the_login_named() {
        assert!(bound_to(&tokens(Some("tos")), "tos").is_ok());
        assert_eq!(bound_to(&tokens(Some("acme")), "tos").unwrap_err().code, "org_mismatch");
        assert_eq!(bound_to(&tokens(None), "tos").unwrap_err().code, "org_required");
    }

    #[test]
    fn a_snapshot_must_agree_with_the_introspection_it_rode_in_on() {
        let auth = snapshot("tos", "m1", Some(vec!["ops".into()]));
        assert!(agrees(&proof(true, Some("tos"), Some("m1"), Some(auth)), "tos", Some("m1")).is_ok());
        // Live, org-bound, no snapshot (an IAM too old, or the stub before it learns to send one):
        // still accepted on the introspection's own proof.
        assert!(agrees(&proof(true, Some("tos"), Some("m1"), None), "tos", Some("m1")).is_ok());
        // Inactive, wrong org, or a snapshot naming another membership: refused.
        assert_eq!(
            agrees(&proof(false, Some("tos"), Some("m1"), None), "tos", Some("m1")).unwrap_err().code,
            "iam_inconsistent"
        );
        assert_eq!(
            agrees(&proof(true, Some("acme"), Some("m1"), None), "tos", Some("m1")).unwrap_err().code,
            "iam_inconsistent"
        );
        let wrong = snapshot("tos", "m2", None);
        assert_eq!(
            agrees(&proof(true, Some("tos"), Some("m1"), Some(wrong)), "tos", Some("m1")).unwrap_err().code,
            "iam_inconsistent"
        );
    }

    #[test]
    fn a_landing_needs_an_org_and_keeps_only_safe_values() {
        let q = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        assert_eq!(Landing::read(&q(&[])).unwrap().org, None);
        assert_eq!(Landing::read(&q(&[("org", "Tos")])).unwrap().org, None);
        let landing =
            Landing::read(&q(&[("org", "tos"), ("next", "//evil"), ("cli", "4242"), ("state", "n0nce")])).unwrap();
        assert_eq!((landing.org.as_deref(), landing.next.as_str()), (Some("tos"), "/"), "a protocol-relative next is dropped");
        assert_eq!(
            Landing::read(&q(&[("org", "tos"), ("next", "/o/名前")])).unwrap().next,
            "/",
            "so is one no header can carry"
        );
        assert_eq!(
            Landing::read(&q(&[("org", "tos"), ("next", "/o/tos?tab=tables")])).unwrap().next,
            "/o/tos?tab=tables"
        );
        assert_eq!((landing.cli, landing.state.as_deref()), (Some(4242), Some("n0nce")));
        assert_eq!(Landing::read(&q(&[("org", "tos"), ("cli", "80")])).unwrap_err().code, "invalid_cli");
        assert_eq!(Landing::read(&q(&[("org", "tos"), ("state", "a b")])).unwrap_err().code, "invalid_state");
        let plain = Landing::read(&q(&[("org", "tos"), ("next", "/o/tos")])).unwrap();
        assert_eq!(json!(plain), json!({"org": "tos", "next": "/o/tos"}), "absent options stay absent");
    }
}
