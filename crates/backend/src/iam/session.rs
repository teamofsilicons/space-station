//! IAM 5 sessions bind one account, organization and IAM world. Tokens and browser session
//! references are sealed under SS_KEY. Browser accounts keep independent rows; selection changes
//! only the cookie. Public context markers prevent a stale tab from following a changed cookie.
//! Login and refresh receipts survive interrupted responses; row locks serialize token rotation.
//! Logout ends only the selected ordinary session and preserves other saved accounts.

use std::collections::HashMap;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{AppendHeaders, Html, IntoResponse, Redirect, Response};
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
const GROUP_COOKIE: &str = "ss_accounts";
const CONTEXT_HEADER: &str = "x-spacestation-context";
/// Refresh when less than this is left on the access token.
const REFRESH_MARGIN: TimeDelta = TimeDelta::seconds(60);
/// Introspect again when the last proof is older than this.
const CHECK_EVERY: TimeDelta = TimeDelta::seconds(60);
const COLUMNS: &str = "world, browser_group, actor, kind, org, membership_id, oat_enc, ort_enc, expires_at, refresh_key, checked_at, tags";
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
    pub browser_group: Option<String>,
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
        .route("/auth/contexts", get(contexts))
        .route("/auth/context", post(select_context))
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
    nonce: String,
    expires_at: i64,
    identity_kind: Kind,
    attempt_id: Option<Uuid>,
    browser_group: Option<String>,
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
        let identity_kind = match q.get("identity_kind").map(String::as_str).unwrap_or("carbon") {
            "carbon" => Kind::Carbon,
            "silicon" => Kind::Silicon,
            _ => return Err(ApiError::bad_request("invalid_identity_kind", "choose carbon or silicon")),
        };
        let attempt_id = match (q.get("display").map(String::as_str), q.get("attempt_id")) {
            (None, None) => None,
            (Some("popup"), Some(id)) => Some(
                Uuid::parse_str(id)
                    .map_err(|_| ApiError::bad_request("invalid_attempt", "a popup attempt_id must be a UUID"))?,
            ),
            _ => return Err(ApiError::bad_request("invalid_attempt", "a popup needs display=popup and attempt_id")),
        };
        let org = q.get("org").filter(|o| valid_org(o)).cloned();
        // A same-origin path that fits in a Location header; anything else lands on `/`.
        let path = |n: &&String| n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') && n.len() <= 1024;
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
        Ok(Landing {
            nonce: URL_SAFE_NO_PAD.encode(crypto::random::<32>()),
            expires_at: Utc::now().timestamp() + 600,
            identity_kind,
            attempt_id,
            browser_group: None,
            org: org.clone(),
            next: next.cloned().unwrap_or_else(|| "/".into()),
            cli,
            state,
        })
    }
}

/// Sends the browser to IAM to sign in on this Application's behalf, bound to `org`; the landing
/// rides in a ten-minute cookie sealed under `SS_KEY`, so the callback trusts nothing in the URL.
async fn login(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let mut landing = Landing::read(&q)?;
    landing.browser_group = Some(
        read_cookie(&headers, GROUP_COOKIE)
            .filter(|s| valid_group(s))
            .unwrap_or_else(|| URL_SAFE_NO_PAD.encode(crypto::random::<32>())),
    );
    // IAM owns account and organization selection. A caller-selected org is checked against
    // the single org IAM returned; it never retargets an existing credential.
    let url = login_url(&state.cfg.iam_auth_url, &state.cfg.iam_app_id, &callback_url(&state), &landing);
    let location = HeaderValue::from_str(&url).map_err(|e| ApiError::internal("login_redirect", e))?;
    let sealed = crypto::seal(&state.cfg.key, &json!(landing).to_string());
    sqlx::query("DELETE FROM iam_login_attempts WHERE expires_at <= now()").execute(&state.store.pg).await?;
    sqlx::query("INSERT INTO iam_login_attempts (state_hash, identity_kind, expires_at) VALUES ($1, $2, $3)")
        .bind(sha256_hex(&landing.nonce))
        .bind(landing.identity_kind.as_str())
        .bind(DateTime::from_timestamp(landing.expires_at, 0).ok_or_else(expired)?)
        .execute(&state.store.pg)
        .await?;
    let cookie = set_cookie(&state, LOGIN_COOKIE, &sealed, "/api/auth", 600);
    Ok((
        StatusCode::FOUND,
        [cookie, (header::LOCATION, location), (header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
    )
        .into_response())
}

fn login_url(auth_url: &str, app_id: &str, callback: &str, landing: &Landing) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("app_id", app_id);
    let mut callback = url::Url::parse(callback).expect("configured callback URL");
    callback.query_pairs_mut().append_pair("state", &landing.nonce);
    query.append_pair("redirect_uri", callback.as_str());
    query.append_pair("identity_kind", landing.identity_kind.as_str());
    query.append_pair("display", "popup");
    format!("{auth_url}?{}", query.finish())
}

/// IAM brings the browser back with `?slt=` and the state embedded in its callback URL. A
/// browser login adds an independent account context and lands on `next`. A terminal login verifies
/// the selected identity before forwarding the SLT; its loopback exchange recovers the same session
/// receipt. The browser never receives the terminal's long-lived `sscli-` secret.
async fn callback(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let landing: Landing = read_cookie(&headers, LOGIN_COOKIE)
        .and_then(|sealed| crypto::open(&state.cfg.key, &sealed))
        .and_then(|json| serde_json::from_str(&json).ok())
        .ok_or_else(|| ApiError::unauthorized("login_expired", "start again at /api/auth/login"))?;
    if landing.expires_at <= Utc::now().timestamp()
        || !q.get("state").is_some_and(|state| crypto::ct_eq(state.as_bytes(), landing.nonce.as_bytes()))
    {
        return Err(ApiError::unauthorized("login_expired", "login state expired or did not match; start again"));
    }
    let result = complete_login(&state, &landing, &q).await;
    let mut response = match result {
        Err(error) if landing.attempt_id.is_some() => {
            tracing::info!(code = %error.code, "popup login did not complete");
            Ok(popup_completion(&state.cfg.origin, &landing, false))
        }
        other => other,
    }?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    Ok(response)
}

async fn complete_login(
    state: &AppState,
    landing: &Landing,
    q: &HashMap<String, String>,
) -> Result<Response, ApiError> {
    let slt = q
        .get("slt")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("slt_required", "IAM did not bring back a short-lived token"))?;
    let reserved = sqlx::query(
        "UPDATE iam_login_attempts SET slt_hash = $2 WHERE state_hash = $1 AND expires_at > now() \
         AND identity_kind = $3 AND (slt_hash IS NULL OR slt_hash = $2)",
    )
    .bind(sha256_hex(&landing.nonce))
    .bind(sha256_hex(slt))
    .bind(landing.identity_kind.as_str())
    .execute(&state.store.pg)
    .await?;
    if reserved.rows_affected() != 1 {
        return Err(ApiError::unauthorized("login_expired", "this login attempt expired or was already used"));
    }
    let done = set_cookie(state, LOGIN_COOKIE, "", "/api/auth", 0);
    if let Some(port) = landing.cli {
        open(state, slt, landing.org.as_deref(), true, None, Some(landing.identity_kind)).await?;
        // RFC 8252 loopback: `port` is a bare number, so 127.0.0.1 is the only reachable target.
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.append_pair("slt", slt);
        if let Some(nonce) = &landing.state {
            query.append_pair("state", nonce);
        }
        let listener = format!("http://127.0.0.1:{port}/?{}", query.finish());
        return Ok(([done], Redirect::to(&listener)).into_response());
    }
    let group = landing.browser_group.as_deref().filter(|s| valid_group(s)).ok_or_else(expired)?;
    let id = open(state, slt, landing.org.as_deref(), false, Some(group), Some(landing.identity_kind)).await?;
    let cookies = [
        set_cookie(state, COOKIE, &id, "/", 30 * 86_400),
        set_cookie(state, GROUP_COOKIE, group, "/", 30 * 86_400),
        done,
    ];
    if landing.attempt_id.is_some() {
        return Ok((AppendHeaders(cookies), popup_completion(&state.cfg.origin, landing, true)).into_response());
    }
    let next = format!("{}{}", state.cfg.origin, landing.next);
    Ok((AppendHeaders(cookies), Redirect::to(&next)).into_response())
}

fn popup_completion(origin: &str, landing: &Landing, success: bool) -> Response {
    let nonce = URL_SAFE_NO_PAD.encode(crypto::random::<16>());
    let message = json!({
        "type": "spacestation:login", "attempt_id": landing.attempt_id,
        "status": if success { "success" } else { "error" },
    });
    let origin = json!(origin).to_string().replace('<', "\\u003c");
    let heading = if success { "Signed in" } else { "Sign-in did not complete" };
    let next = landing.next.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Space Station</title><h1>{heading}</h1>\
         <p>You can close this window and return to Space Station.</p><a href=\"{next}\">Return to Space Station</a>\
         <script nonce=\"{nonce}\">if(window.opener){{window.opener.postMessage({message},{origin});window.close();}}</script>"
    );
    let headers = [
        (header::CACHE_CONTROL, "no-store".to_owned()),
        (header::REFERRER_POLICY, "no-referrer".to_owned()),
        (
            header::CONTENT_SECURITY_POLICY,
            format!("default-src 'none'; script-src 'nonce-{nonce}'; frame-ancestors 'none'"),
        ),
    ];
    (headers, Html(body)).into_response()
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
    Ok(Json(json!({"token": open(&state, &body.slt, body.org.as_deref(), true, None, None).await?})))
}

/// Exchanges `slt`, proves the membership and reads the snapshot once, and writes the row and the
/// mirror in one transaction. The secret handed back is the cookie value, or an `sscli-` bearer
/// when `cli`.
async fn open(
    state: &AppState,
    slt: &str,
    requested_org: Option<&str>,
    cli: bool,
    group: Option<&str>,
    requested_kind: Option<Kind>,
) -> Result<String, ApiError> {
    state.iam.verify_world().await?;
    // Recover both the IAM mutation and the local result after an interrupted response. The
    // secret is derived with the server key, never from public state. A logout leaves a receipt.
    let secret = crypto::hmac_sha256_hex(&state.cfg.key, format!("iam5-session:{cli}:{slt}").as_bytes());
    let id = if cli { format!("{CLI_PREFIX}{secret}") } else { secret };
    let id_hash = sha256_hex(&id);
    let login_key = mutation_key(&format!("iam5-login:{slt}"));
    let mut tx = state.store.pg.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(login_key.to_string())
        .execute(&mut *tx)
        .await?;
    let receipt: Option<(String, DateTime<Utc>)> =
        sqlx::query_as("SELECT id_hash, created_at FROM iam_login_receipts WHERE request_key = $1")
            .bind(login_key)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((receipt, created_at)) = receipt {
        // A spent SLT can recover an interrupted response briefly; it must never become a
        // permanent credential for the unauthenticated terminal exchange endpoint.
        if receipt != id_hash || Utc::now() - created_at >= TimeDelta::minutes(2) {
            return Err(expired());
        }
        let stored = fetch_locked(state, &mut tx, &id_hash).await?;
        if requested_org.is_some_and(|org| org != stored.org) || stored.browser_group != group.map(sha256_hex) {
            return Err(inconsistent());
        }
        check_kind(stored.kind, requested_kind)?;
        return Ok(id);
    }
    let tokens = state.iam.exchange(slt, &login_key.to_string()).await.map_err(|e| match e.is_invalid_grant() {
        true => ApiError::unauthorized(
            "invalid_slt",
            "IAM refused the short-lived token: it is single-use and lives two minutes",
        ),
        false => e.into(),
    })?;
    if let Err(error) = check_kind(tokens.kind, requested_kind) {
        discard(state, &tokens).await;
        return Err(error);
    }
    // The freshly minted family is proved before it is stored; every refusal below discards it, so
    // a family this server will never use is not left alive at IAM.
    let proof = match state.iam.introspect(&tokens.oat).await {
        Ok(proof) => proof,
        // The exchange may already have succeeded at IAM. Keep its idempotent result usable
        // when introspection fails, so retrying the same protected callback can finish login.
        Err(e) => return Err(e.into()),
    };
    let org = tokens
        .org
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("org_required", "IAM did not select an organization"))?;
    if let (Some(bound), Some(requested)) = (tokens.org.as_deref(), requested_org)
        && bound != requested
    {
        discard(state, &tokens).await;
        return Err(ApiError::bad_request("org_mismatch", format!("the login was bound to {bound}, not {requested}")));
    }
    if let Err(e) = bound_to(&tokens, org) {
        discard(state, &tokens).await;
        return Err(e);
    }
    if let Err(e) = agrees(&proof, &tokens.actor, org, proof.membership_id.as_deref()) {
        discard(state, &tokens).await;
        tracing::warn!("IAM: a token just minted for @{} bound to {org} introspects as {proof:?}", tokens.actor);
        return Err(e);
    }
    let tags = proof.authorization.as_ref().and_then(|a| a.tags.clone());
    sqlx::query(
        "INSERT INTO sessions (id_hash, actor, kind, org, membership_id, oat_enc, ort_enc, expires_at, tags, iam_contract, browser_group, browser_secret_enc, world) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8,   $9, 5, $10, $11, $12)",
    )
    .bind(sha256_hex(&id))
    .bind(&tokens.actor)
    .bind(tokens.kind.as_str())
    .bind(org)
    .bind(&proof.membership_id)
    .bind(crypto::seal(&state.cfg.key, &tokens.oat))
    .bind(crypto::seal(&state.cfg.key, &tokens.ort))
    .bind(proof.expires_at.ok_or_else(inconsistent)?)
    .bind(tags.as_ref().map(|t| Value::from(t.clone())))
    .bind(group.map(sha256_hex))
    .bind(group.map(|_| crypto::seal(&state.cfg.key, &id)))
    .bind(&state.iam.world)
    .execute(&mut *tx)
    .await?;
    if let Some(auth) = &proof.authorization {
        mirror(&mut tx, org, &tokens.actor, auth).await?;
    }
    sqlx::query("INSERT INTO iam_login_receipts (request_key, id_hash) VALUES ($1, $2)")
        .bind(login_key)
        .bind(&id_hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if proof.authorization.is_some() {
        // The snapshot changed what this actor can see; drop the cached visibility so it is re-read.
        access::forget(state, org);
    }
    Ok(id)
}

fn check_kind(actual: Kind, requested: Option<Kind>) -> Result<(), ApiError> {
    if requested.is_some_and(|kind| kind != actual) {
        return Err(ApiError::unauthorized(
            "identity_kind_mismatch",
            "IAM returned a different kind of account; start again",
        ));
    }
    Ok(())
}

/// The exchange must have been bound to `org`: everything here lives inside one, and a session
/// speaks for exactly the org its login named.
fn bound_to(tokens: &Tokens, org: &str) -> Result<(), ApiError> {
    match tokens.org.as_deref() {
        Some(bound) if bound == org => Ok(()),
        Some(bound) => Err(ApiError::bad_request("org_mismatch", format!("the login was bound to {bound}, not {org}"))),
        None => Err(inconsistent()),
    }
}

/// Introspection proves the token is live, bound to `org`, and holds `membership`; a present
/// snapshot must name the same org and membership. `membership` is `None` only for the initial
/// exchange, whose membership introspection itself supplies.
fn agrees(proof: &Introspection, actor: &str, org: &str, membership: Option<&str>) -> Result<(), ApiError> {
    if !proof.active
        || !proof.authorizations.is_empty()
        || proof.org.as_deref() != Some(org)
        || proof.membership_id.as_deref() != Some(format!("{actor}[{org}]").as_str())
        || proof.authorization.iter().chain(&proof.authorizations).any(|a| {
            a.public_id.as_deref().is_some_and(|id| id != actor)
                || !valid_org(&a.org)
                || a.membership_id != format!("{actor}[{}]", a.org)
        })
    {
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
        kind: Kind::of(actor).expect("validated IAM actor"),
        membership_id: auth.membership_id.clone(),
        // IAM exposes public membership IDs; retain any prior principal UUID for historical tombstones.
        principal_id: None,
        status: "active".into(),
        tags: auth.tags.clone(),
        org_role: auth.org_role.clone(),
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
        guard_context(&headers, &cookie)?;
        end(&state, &cookie).await?;
    }
    let mut next = None;
    if let Some(group) = read_cookie(&headers, GROUP_COOKIE).filter(|s| valid_group(s)) {
        let sealed: Option<String> = sqlx::query_scalar("SELECT browser_secret_enc FROM sessions WHERE browser_group = $1 AND iam_contract = 5 AND world = $2 ORDER BY created_at DESC LIMIT 1")
            .bind(sha256_hex(&group)).bind(&state.iam.world).fetch_optional(&state.store.pg).await?;
        next = sealed.and_then(|s| crypto::open(&state.cfg.key, &s));
    }
    Ok((
        [set_cookie(&state, COOKIE, next.as_deref().unwrap_or(""), "/", if next.is_some() { 30 * 86_400 } else { 0 })],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

/// Deletes the session behind `secret`, if there is one, and, best effort, its token family at IAM.
async fn end(state: &AppState, secret: &str) -> Result<(), ApiError> {
    let Some(session) = fetch(state, &sha256_hex(secret)).await? else {
        return Ok(());
    };
    sqlx::query("DELETE FROM sessions WHERE id_hash = $1 AND iam_contract = 5")
        .bind(&session.id_hash)
        .execute(&state.store.pg)
        .await?;
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
    state.iam.verify_world().await?;
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
    let sql = format!("SELECT {COLUMNS} FROM sessions WHERE id_hash = $1 AND iam_contract = 5");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id_hash).fetch_optional(&state.store.pg).await?;
    Ok(row.and_then(|r| decode(state, id_hash, &r)))
}

/// The same read under a `FOR UPDATE` lock, inside a transaction.
async fn fetch_locked(state: &AppState, tx: &mut PgConnection, id_hash: &str) -> Result<Session, ApiError> {
    let sql = format!("SELECT {COLUMNS} FROM sessions WHERE id_hash = $1 AND iam_contract = 5 FOR UPDATE");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id_hash).fetch_optional(&mut *tx).await?;
    row.and_then(|r| decode(state, id_hash, &r)).ok_or_else(expired)
}

/// `None` when the tokens were sealed under another key: the session is unusable.
fn decode(state: &AppState, id_hash: &str, row: &PgRow) -> Option<Session> {
    if row.get::<String, _>("world") != state.iam.world {
        return None;
    }
    let open = |column| crypto::open(&state.cfg.key, &row.get::<String, _>(column));
    let actor: String = row.get("actor");
    let kind = Kind::of(&actor)?;
    if row.get::<String, _>("kind") != kind.as_str() {
        return None;
    }
    Some(Session {
        id_hash: id_hash.to_owned(),
        browser_group: row.get("browser_group"),
        actor,
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
    if current.oat != seen.oat && current.expires_at - Utc::now() >= REFRESH_MARGIN {
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
            let key = mutation_key(&format!("iam5-refresh:{}:{}", current.id_hash, current.ort));
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
            if tokens.actor != current.actor
                || tokens.kind != current.kind
                || tokens.org.as_deref() != Some(current.org.as_str())
            {
                return Err(inconsistent());
            }
            let expires_at = Utc::now() - REFRESH_MARGIN;
            sqlx::query(
                "UPDATE sessions SET oat_enc = $2, ort_enc = $3, expires_at = $4, refresh_key = NULL WHERE id_hash = $1",
            )
            .bind(&current.id_hash)
            .bind(crypto::seal(&state.cfg.key, &tokens.oat))
            .bind(crypto::seal(&state.cfg.key, &tokens.ort))
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
            let proof = state.iam.introspect(&tokens.oat).await?;
            agrees(&proof, &current.actor, &current.org, current.membership_id.as_deref())?;
            let expires_at = proof.expires_at.ok_or_else(inconsistent)?;
            sqlx::query("UPDATE sessions SET expires_at = $2 WHERE id_hash = $1")
                .bind(&current.id_hash)
                .bind(expires_at)
                .execute(&mut *tx)
                .await?;
            Ok(Session { oat: tokens.oat, ort: tokens.ort, expires_at, refresh_key: None, ..current.clone() })
        }
        Err(e) if e.is_invalid_grant() => {
            sqlx::query("DELETE FROM sessions WHERE id_hash = $1 AND iam_contract = 5")
                .bind(&current.id_hash)
                .execute(&mut *tx)
                .await?;
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
    let authorizations = proof.authorizations.clone();
    if agrees(&proof, &proved.actor, &proved.org, proved.membership_id.as_deref()).is_err() {
        sqlx::query("DELETE FROM sessions WHERE id_hash = $1 AND iam_contract = 5")
            .bind(&proved.id_hash)
            .execute(&mut *tx)
            .await?;
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
    for auth in &authorizations {
        mirror(&mut tx, &auth.org, &proved.actor, auth).await?;
    }
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

fn valid_group(group: &str) -> bool {
    group.len() == 43 && group.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn mutation_key(material: &str) -> Uuid {
    Uuid::parse_str(&sha256_hex(material)[..32]).expect("sha256 prefix is a UUID")
}

/// Public context markers cannot authenticate a request. They bind a cookie-authenticated
/// request to the exact account the page rendered, including two accounts in the same org.
pub fn guard_context(headers: &HeaderMap, secret: &str) -> Result<(), ApiError> {
    let marker = headers.get(CONTEXT_HEADER).and_then(|v| v.to_str().ok());
    if marker != Some(sha256_hex(secret).as_str()) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "context_changed",
            "The selected account changed. Reload this page before continuing.",
        ));
    }
    Ok(())
}

async fn contexts(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    state.iam.verify_world().await?;
    let group = read_cookie(&headers, GROUP_COOKIE).filter(|s| valid_group(s)).ok_or_else(expired)?;
    let selected = cookie_value(&headers).map(|s| sha256_hex(&s));
    let rows: Vec<(String, String, String)> = sqlx::query_as("SELECT id_hash, actor, org FROM sessions WHERE browser_group = $1 AND iam_contract = 5 AND world = $2 ORDER BY created_at DESC")
        .bind(sha256_hex(&group)).bind(&state.iam.world).fetch_all(&state.store.pg).await?;
    Ok(Json(json!(rows.into_iter().map(|(id, actor, org)| json!({"selected": selected.as_ref() == Some(&id), "context_id": id, "actor": actor, "org": org})).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct Selection {
    context_id: String,
}

async fn select_context(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Selection>,
) -> Result<Response, ApiError> {
    if !origin_ok(&headers, &state.cfg.origin) {
        return Err(bad_origin());
    }
    let previous = cookie_value(&headers).ok_or_else(expired)?;
    guard_context(&headers, &previous)?;
    let group = read_cookie(&headers, GROUP_COOKIE).filter(|s| valid_group(s)).ok_or_else(expired)?;
    // Each stored context has its own random secret; the group never retargets IAM credentials.
    let sealed: Option<String> = sqlx::query_scalar("SELECT browser_secret_enc FROM sessions WHERE id_hash = $1 AND browser_group = $2 AND iam_contract = 5 AND world = $3")
        .bind(&body.context_id).bind(sha256_hex(&group)).bind(&state.iam.world).fetch_optional(&state.store.pg).await?;
    let secret = sealed.and_then(|s| crypto::open(&state.cfg.key, &s)).ok_or_else(expired)?;
    load(&state, &secret).await?.ok_or_else(expired)?;
    Ok(([set_cookie(&state, COOKIE, &secret, "/", 30 * 86_400)], StatusCode::NO_CONTENT).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_is_not_authority_to_retarget_a_rendered_account() {
        let mut headers = HeaderMap::new();
        assert!(guard_context(&headers, "account-a").is_err());
        headers.insert(CONTEXT_HEADER, sha256_hex("account-a").parse().unwrap());
        assert!(guard_context(&headers, "account-a").is_ok());
        assert_eq!(guard_context(&headers, "account-b").unwrap_err().status, StatusCode::CONFLICT);
    }

    #[test]
    fn interrupted_refreshes_reconstruct_the_same_mutation_but_new_pairs_do_not() {
        assert_eq!(mutation_key("session:ort-a"), mutation_key("session:ort-a"));
        assert_ne!(mutation_key("session:ort-a"), mutation_key("session:ort-b"));
        assert_ne!(mutation_key("session:ort-a"), mutation_key("other:ort-a"));
    }

    #[test]
    fn iam_chooses_the_organization_while_local_landing_preserves_the_preference() {
        let landing = Landing::read(&HashMap::from([("org".into(), "tos".into())])).unwrap();
        assert_eq!(landing.org.as_deref(), Some("tos"));
        let url =
            login_url("https://auth.iam.example", "spacestation", "https://ss.example/api/auth/callback", &landing);
        let parsed = url::Url::parse(&url).unwrap();
        let query = parsed.query_pairs().collect::<HashMap<_, _>>();
        assert_eq!(query.len(), 4);
        assert_eq!(query["app_id"], "spacestation");
        assert_eq!(query["redirect_uri"], format!("https://ss.example/api/auth/callback?state={}", landing.nonce));
        assert_eq!(query["identity_kind"], "carbon");
        assert_eq!(query["display"], "popup");
        assert!(!query.contains_key("org_id"));
    }

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
            actor: "c:alice".into(),
            kind: Kind::Carbon,
            org: org.map(str::to_owned),
        }
    }

    fn proof(active: bool, org: Option<&str>, membership: Option<&str>, auth: Option<Authorization>) -> Introspection {
        Introspection {
            active,
            expires_at: Some(Utc::now() + TimeDelta::hours(1)),
            org: org.map(str::to_owned),
            membership_id: membership.map(str::to_owned),
            authorization: auth,
            authorizations: Vec::new(),
        }
    }

    fn snapshot(org: &str, membership: &str, tags: Option<Vec<String>>) -> Authorization {
        Authorization {
            public_id: Some("si:bot".into()),
            org: org.into(),
            org_uuid: "01a0-org".into(),
            membership_id: membership.into(),
            membership_version: 4,
            org_role: None,
            tags,
        }
    }

    #[test]
    fn a_session_is_bound_to_exactly_the_org_the_login_named() {
        assert!(bound_to(&tokens(Some("tos")), "tos").is_ok());
        assert_eq!(bound_to(&tokens(Some("acme")), "tos").unwrap_err().code, "org_mismatch");
        assert!(bound_to(&tokens(None), "tos").is_err());
    }

    #[test]
    fn an_unscoped_proof_cannot_select_a_requested_organization() {
        let mut p = proof(true, None, None, None);
        p.authorizations = vec![snapshot("tos", "m1", None), snapshot("acme", "m2", None)];
        assert!(agrees(&p, "si:bot", "acme", None).is_err());
    }

    #[test]
    fn a_snapshot_must_agree_with_the_introspection_it_rode_in_on() {
        let auth = snapshot("tos", "si:bot[tos]", Some(vec!["ops".into()]));
        assert!(
            agrees(&proof(true, Some("tos"), Some("si:bot[tos]"), Some(auth)), "si:bot", "tos", Some("si:bot[tos]"))
                .is_ok()
        );
        // Undisclosed snapshot: the canonical membership still proves this actor and organization.
        assert!(
            agrees(&proof(true, Some("tos"), Some("si:bot[tos]"), None), "si:bot", "tos", Some("si:bot[tos]")).is_ok()
        );
        // Inactive, wrong org, or a snapshot naming another membership: refused.
        assert_eq!(
            agrees(&proof(false, Some("tos"), Some("si:bot[tos]"), None), "si:bot", "tos", Some("si:bot[tos]"))
                .unwrap_err()
                .code,
            "iam_inconsistent"
        );
        assert_eq!(
            agrees(&proof(true, Some("acme"), Some("si:bot[tos]"), None), "si:bot", "tos", Some("si:bot[tos]"))
                .unwrap_err()
                .code,
            "iam_inconsistent"
        );
        let wrong = snapshot("tos", "si:other[tos]", None);
        assert_eq!(
            agrees(&proof(true, Some("tos"), Some("si:bot[tos]"), Some(wrong)), "si:bot", "tos", Some("si:bot[tos]"))
                .unwrap_err()
                .code,
            "iam_inconsistent"
        );
        let other_actor = snapshot("tos", "si:bot[tos]", None);
        assert!(
            agrees(
                &proof(true, Some("tos"), Some("si:bot[tos]"), Some(other_actor)),
                "c:alice",
                "tos",
                Some("si:bot[tos]")
            )
            .is_err()
        );
    }

    #[test]
    fn a_landing_needs_an_org_and_keeps_only_safe_values() {
        let q = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        assert_eq!(Landing::read(&q(&[])).unwrap().org, None);
        assert_eq!(Landing::read(&q(&[("org", "Tos")])).unwrap().org, None);
        let landing =
            Landing::read(&q(&[("org", "tos"), ("next", "//evil"), ("cli", "4242"), ("state", "n0nce")])).unwrap();
        assert_eq!(
            (landing.org.as_deref(), landing.next.as_str()),
            (Some("tos"), "/"),
            "a protocol-relative next is dropped"
        );
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
        assert_eq!(Landing::read(&q(&[("next", "/\\evil.example")])).unwrap().next, "/");
        assert!(Landing::read(&q(&[("identity_kind", "other")])).is_err());
        assert!(Landing::read(&q(&[("display", "popup")])).is_err());
        let silicon = Landing::read(&q(&[
            ("identity_kind", "silicon"),
            ("display", "popup"),
            ("attempt_id", "624606a7-4f46-4123-bb31-3379ee015b97"),
        ]))
        .unwrap();
        assert_eq!(silicon.identity_kind, Kind::Silicon);
        assert!(silicon.attempt_id.is_some());
        let plain = Landing::read(&q(&[("org", "tos"), ("next", "/o/tos")])).unwrap();
        let value = json!(plain);
        assert!(value.get("cli").is_none() && value.get("state").is_none());
        assert_eq!(value["org"], "tos");
        assert!(plain.nonce.len() >= 32);
        assert!(plain.expires_at > Utc::now().timestamp());
    }
}

#[cfg(test)]
#[path = "session_context_tests.rs"]
mod context_tests;
