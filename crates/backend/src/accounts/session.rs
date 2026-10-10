//! Account sessions survive browser and CLI restarts until Accounts expires or revokes them.
//! Upstream refresh is serialized under a database lock; transient failures never erase a login.
use super::{Identity, Kind, client::Tokens};
use crate::{
    crypto,
    http::{
        ApiError, AppState, Json, Query,
        auth::{bad_origin, bearer, origin_ok},
    },
};
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{AppendHeaders, Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use space_station_shared::secrets::sha256_hex;
use sqlx::{PgConnection, Row, postgres::PgRow};
use std::collections::HashMap;
use uuid::Uuid;
const COOKIE: &str = "ss_session";
const LOGIN_COOKIE: &str = "ss_login";
const GROUP_COOKIE: &str = "ss_accounts";
pub const CLI_PREFIX: &str = "sscli-";
const MARGIN: TimeDelta = TimeDelta::seconds(60);
const COLUMNS: &str = "s.*, o.namespace, o.actor AS current_actor";
#[derive(Clone)]
pub struct Session {
    pub id_hash: String,
    pub uuid: Uuid,
    pub actor: String,
    pub kind: Kind,
    pub org: String,
    pub oat: String,
    pub ort: String,
    pub access_expires_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub checked_at: DateTime<Utc>,
}
impl Session {
    pub fn identity(&self) -> Identity {
        Identity { uuid: self.uuid, id: self.actor.clone(), kind: self.kind, org: self.org.clone() }
    }
    pub fn public(&self, state: &AppState) -> Value {
        json!({"context_id":self.id_hash,"id":self.actor,"uuid":self.uuid,"kind":self.kind,"app":state.cfg.origin,"expires_at":self.expires_at,"expires_at_unix":self.expires_at.timestamp()})
    }
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/session", post(exchange))
        .route("/auth/logout", post(logout))
        .route("/auth/contexts", get(contexts))
        .route("/auth/context", post(select_context))
}
pub fn cookie_value(headers: &HeaderMap) -> Option<String> {
    read_cookie(headers, COOKIE)
}
fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|v| v.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.into())
}
fn cookie(state: &AppState, name: &str, value: &str, path: &str, max_age: i64) -> (header::HeaderName, HeaderValue) {
    let secure = if state.cfg.origin.starts_with("https://") { "; Secure" } else { "" };
    let domain = state.cfg.cookie_domain.as_ref().map(|d| format!("; Domain={d}")).unwrap_or_default();
    (
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{name}={value}; Max-Age={}; Path={path}; HttpOnly; SameSite=Lax{secure}{domain}",
            max_age.max(0)
        ))
        .expect("ASCII cookie"),
    )
}
pub fn renew_cookies(
    state: &AppState,
    headers: &HeaderMap,
    secret: &str,
    session: &Session,
) -> Vec<(header::HeaderName, HeaderValue)> {
    let mut cookies = vec![cookie(state, COOKIE, secret, "/", max_age(session.expires_at))];
    if let Some(group) = read_cookie(headers, GROUP_COOKIE) {
        cookies.push(cookie(state, GROUP_COOKIE, &group, "/", 900 * 86_400));
    }
    cookies
}

fn max_age(expires: DateTime<Utc>) -> i64 {
    (expires - Utc::now()).num_seconds().max(0)
}
fn expired() -> ApiError {
    ApiError::unauthorized("session_expired", "this session expired or was revoked; log in again")
}
fn group(headers: &HeaderMap) -> String {
    read_cookie(headers, GROUP_COOKIE)
        .filter(|v| v.len() == 43 && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
        .unwrap_or_else(|| URL_SAFE_NO_PAD.encode(crypto::random::<32>()))
}
fn callback_url(state: &AppState) -> String {
    format!("{}/api/auth/callback", state.cfg.origin)
}
#[derive(Serialize, Deserialize)]
struct Landing {
    nonce: String,
    verifier: String,
    expires_at: i64,
    next: String,
    attempt_id: Option<Uuid>,
    group: String,
    cli: Option<u16>,
    state: Option<String>,
}
async fn login(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if q.get("identity_kind").is_some_and(|v| v != "carbon") {
        return Err(ApiError::bad_request(
            "slt_required",
            "Silicons sign in with a short-lived token from silicon-accounts",
        ));
    }
    let cli =
        q.get("cli")
            .map(|v| {
                v.parse::<u16>().ok().filter(|v| *v >= 1024).ok_or_else(|| {
                    ApiError::bad_request("invalid_cli", "cli must be a loopback port from 1024 to 65535")
                })
            })
            .transpose()?;
    let client_state = q.get("state").cloned();
    if cli.is_some()
        && client_state.as_ref().is_none_or(|s| {
            s.is_empty()
                || s.len() > 128
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
        })
    {
        return Err(ApiError::bad_request("invalid_state", "CLI login requires a nonce state"));
    }
    let next = q
        .get("next")
        .filter(|n| {
            n.starts_with('/')
                && !n.starts_with("//")
                && !n.contains('\\')
                && n.len() <= 1024
                && n.bytes().all(|b| b.is_ascii_graphic())
        })
        .cloned()
        .unwrap_or_else(|| "/".into());
    let attempt_id = q
        .get("attempt_id")
        .map(|v| Uuid::parse_str(v).map_err(|_| ApiError::bad_request("invalid_attempt", "attempt_id must be a UUID")))
        .transpose()?;
    let landing = Landing {
        nonce: URL_SAFE_NO_PAD.encode(crypto::random::<32>()),
        verifier: URL_SAFE_NO_PAD.encode(crypto::random::<32>()),
        expires_at: Utc::now().timestamp() + 600,
        next,
        attempt_id,
        group: group(&headers),
        cli,
        state: client_state,
    };
    let mut url = url::Url::parse(&state.cfg.accounts_auth_url).map_err(|e| ApiError::internal("accounts_url", e))?;
    url.query_pairs_mut()
        .append_pair("app_id", &state.cfg.accounts_app_id)
        .append_pair("redirect_uri", &callback_url(&state))
        .append_pair("state", &landing.nonce)
        .append_pair("response_type", "code")
        .append_pair("scope", "profile")
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &URL_SAFE_NO_PAD.encode(Sha256::digest(landing.verifier.as_bytes())))
        .append_pair("prompt", "select_account");
    let sealed = crypto::seal(&state.cfg.key, &serde_json::to_string(&landing).expect("landing JSON"));
    Ok(([cookie(&state, LOGIN_COOKIE, &sealed, "/api/auth", 600)], Redirect::to(url.as_str())).into_response())
}
async fn callback(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let landing: Landing = read_cookie(&headers, LOGIN_COOKIE)
        .and_then(|v| crypto::open(&state.cfg.key, &v))
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(expired)?;
    if landing.expires_at <= Utc::now().timestamp()
        || !q.get("state").is_some_and(|s| crypto::ct_eq(s.as_bytes(), landing.nonce.as_bytes()))
    {
        return Err(expired());
    }
    let result = complete_callback(&state, &landing, &q).await;
    let mut response = match result {
        Err(_) if landing.attempt_id.is_some() => popup(&state, &landing, false),
        other => other?,
    };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    Ok(response)
}
async fn complete_callback(
    state: &AppState,
    landing: &Landing,
    q: &HashMap<String, String>,
) -> Result<Response, ApiError> {
    let code = q.get("code").filter(|v| !v.is_empty()).ok_or_else(|| {
        ApiError::bad_request("login_refused", "Silicon Accounts did not return an authorization code")
    })?;
    let cli = landing.cli.is_some();
    let secret =
        open(state, code, Some(landing), cli, if cli { None } else { Some(&landing.group) }, Some(Kind::Carbon))
            .await?;
    let s = load(state, &secret).await?.ok_or_else(expired)?;
    let done = cookie(state, LOGIN_COOKIE, "", "/api/auth", 0);
    if let Some(port) = landing.cli {
        let handoff = URL_SAFE_NO_PAD.encode(crypto::random::<32>());
        sqlx::query("INSERT INTO account_handoffs (code_hash,state_hash,secret_enc,expires_at) VALUES ($1,$2,$3,now()+interval '2 minutes')")
            .bind(sha256_hex(&handoff)).bind(sha256_hex(landing.state.as_deref().unwrap_or_default())).bind(crypto::seal(&state.cfg.key,&secret)).execute(&state.store.pg).await?;
        let mut url = url::Url::parse(&format!("http://127.0.0.1:{port}/")).expect("loopback URL");
        url.query_pairs_mut()
            .append_pair("code", &handoff)
            .append_pair("state", landing.state.as_deref().unwrap_or_default());
        return Ok(([done], Redirect::to(url.as_str())).into_response());
    }
    let response = if landing.attempt_id.is_some() {
        popup(state, landing, true)
    } else {
        Redirect::to(&format!("{}{}", state.cfg.origin, landing.next)).into_response()
    };
    Ok((
        AppendHeaders([
            cookie(state, COOKIE, &secret, "/", max_age(s.expires_at)),
            cookie(state, GROUP_COOKIE, &landing.group, "/", 900 * 86_400),
            done,
        ]),
        response,
    )
        .into_response())
}
fn popup(state: &AppState, landing: &Landing, success: bool) -> Response {
    let nonce = URL_SAFE_NO_PAD.encode(crypto::random::<16>());
    let message = json!({"type":"spacestation:login","attempt_id":landing.attempt_id,"status":if success {"success"}else{"error"}});
    let origin = json!(state.cfg.origin).to_string().replace('<', "\\u003c");
    let heading = if success { "Signed in" } else { "Sign-in did not complete" };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Space Station</title><h1>{heading}</h1><p>Return to Space Station. You can close this window.</p><script nonce=\"{nonce}\">if(window.opener){{window.opener.postMessage({message},{origin});window.close();}}</script>"
    );
    (
        [(
            header::CONTENT_SECURITY_POLICY,
            format!("default-src 'none'; script-src 'nonce-{nonce}'; frame-ancestors 'none'"),
        )],
        Html(body),
    )
        .into_response()
}
#[derive(Deserialize)]
struct Exchange {
    slt: Option<String>,
    code: Option<String>,
    state: Option<String>,
    #[serde(default)]
    browser: bool,
    identity_kind: Option<Kind>,
}
async fn exchange(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Exchange>,
) -> Result<Response, ApiError> {
    if body.browser && !origin_ok(&headers, &state.cfg.origin) {
        return Err(bad_origin());
    }
    let browser_group = body.browser.then(|| group(&headers));
    let secret = match (body.slt, body.code) {
        (Some(slt), None) if !slt.is_empty() => {
            open(&state, &slt, None, !body.browser, browser_group.as_deref(), body.identity_kind).await?
        }
        (None, Some(code)) if !body.browser => {
            let sealed:Option<String>=sqlx::query_scalar("DELETE FROM account_handoffs WHERE code_hash=$1 AND state_hash=$2 AND expires_at>now() RETURNING secret_enc")
                .bind(sha256_hex(&code)).bind(sha256_hex(body.state.as_deref().unwrap_or_default())).fetch_optional(&state.store.pg).await?;
            sealed.and_then(|s| crypto::open(&state.cfg.key, &s)).ok_or_else(expired)?
        }
        _ => {
            return Err(ApiError::bad_request("invalid_exchange", "provide one short-lived token or CLI handoff code"));
        }
    };
    let s = load(&state, &secret).await?.ok_or_else(expired)?;
    let mut response = s.public(&state);
    if let Some(group) = browser_group {
        return Ok((
            AppendHeaders([
                cookie(&state, COOKIE, &secret, "/", max_age(s.expires_at)),
                cookie(&state, GROUP_COOKIE, &group, "/", 900 * 86_400),
            ]),
            Json(response),
        )
            .into_response());
    }
    response["session_token"] = json!(secret);
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(response)).into_response())
}
async fn open(
    state: &AppState,
    credential: &str,
    landing: Option<&Landing>,
    cli: bool,
    group: Option<&str>,
    kind: Option<Kind>,
) -> Result<String, ApiError> {
    let login_hash =
        sha256_hex(&format!("accounts:{}:{credential}", landing.map(|l| l.nonce.as_str()).unwrap_or("slt")));
    let secret = crypto::hmac_sha256_hex(&state.cfg.key, format!("accounts-session:{cli}:{login_hash}").as_bytes());
    let secret = if cli { format!("{CLI_PREFIX}{secret}") } else { secret };
    let id_hash = sha256_hex(&secret);
    let mut tx = state.store.pg.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))").bind(&login_hash).execute(&mut *tx).await?;
    let receipt: Option<(String, DateTime<Utc>)> =
        sqlx::query_as("SELECT id_hash,created_at FROM account_login_receipts WHERE login_hash=$1")
            .bind(&login_hash)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((id, at)) = receipt {
        // Only the sealed browser landing can recover a callback response. An SLT stays single-use.
        if landing.is_none() || id != id_hash || Utc::now() - at >= TimeDelta::minutes(2) {
            return Err(expired());
        }
        let matched:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM account_sessions WHERE id_hash=$1 AND browser_group IS NOT DISTINCT FROM $2 AND expires_at>now())").bind(&id).bind(group.map(sha256_hex)).fetch_one(&mut *tx).await?;
        return if matched { Ok(secret) } else { Err(expired()) };
    }
    let tokens = if let Some(landing) = landing {
        state.accounts.code(credential, &callback_url(state), &landing.verifier).await
    } else {
        state.accounts.exchange(credential).await
    }
    .map_err(|e| {
        if e.is_invalid_grant() {
            ApiError::unauthorized("invalid_slt", "Silicon Accounts refused the expired or already used login token")
        } else {
            e.into()
        }
    })?;
    if kind.is_some_and(|kind| kind != tokens.account.kind) {
        let _ = state.accounts.revoke(&tokens.ort).await;
        return Err(ApiError::unauthorized(
            "identity_kind_mismatch",
            "Silicon Accounts returned a different account kind",
        ));
    }
    sqlx::query("INSERT INTO account_owners (uuid,namespace,actor,kind) VALUES ($1,$2,$3,$4) ON CONFLICT (uuid) DO UPDATE SET actor=EXCLUDED.actor,kind=EXCLUDED.kind,active=true,updated_at=now()")
        .bind(tokens.account.uuid).bind(tokens.account.uuid.to_string()).bind(&tokens.account.id).bind(tokens.account.kind.as_str()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO account_sessions (id_hash,account_uuid,actor,kind,access_token_enc,refresh_token_enc,access_expires_at,expires_at,browser_group,secret_enc,world) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(&id_hash).bind(tokens.account.uuid).bind(&tokens.account.id).bind(tokens.account.kind.as_str()).bind(crypto::seal(&state.cfg.key,&tokens.oat)).bind(crypto::seal(&state.cfg.key,&tokens.ort)).bind(tokens.access_expires_at).bind(tokens.expires_at).bind(group.map(sha256_hex)).bind(crypto::seal(&state.cfg.key,&secret)).bind(&state.accounts.world).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO account_login_receipts (login_hash,id_hash) VALUES ($1,$2)")
        .bind(login_hash)
        .bind(&id_hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::notifications::engine::changed();
    Ok(secret)
}
fn decode(state: &AppState, row: PgRow) -> Option<Session> {
    let actor: String = row.get("current_actor");
    let kind = Kind::of(&actor)?;
    Some(Session {
        id_hash: row.get("id_hash"),
        uuid: row.get("account_uuid"),
        actor,
        kind,
        org: row.get("namespace"),
        oat: crypto::open(&state.cfg.key, &row.get::<String, _>("access_token_enc"))?,
        ort: crypto::open(&state.cfg.key, &row.get::<String, _>("refresh_token_enc"))?,
        access_expires_at: row.get("access_expires_at"),
        expires_at: row.get("expires_at"),
        checked_at: row.get("checked_at"),
    })
}
pub async fn load(state: &AppState, secret: &str) -> Result<Option<Session>, ApiError> {
    let id = sha256_hex(secret);
    let query = format!(
        "SELECT {COLUMNS} FROM account_sessions s JOIN account_owners o ON o.uuid=s.account_uuid AND o.active WHERE s.id_hash=$1 AND s.world=$2"
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(&id)
        .bind(&state.accounts.world)
        .fetch_optional(&state.store.pg)
        .await?;
    let Some(s) = row.and_then(|r| decode(state, r)) else { return Ok(None) };
    if s.expires_at <= Utc::now() {
        return Err(expired());
    }
    if s.access_expires_at - Utc::now() >= MARGIN && Utc::now() - s.checked_at < MARGIN {
        return Ok(Some(s));
    }
    let mut tx = state.store.pg.begin().await?;
    let query = format!(
        "SELECT {COLUMNS} FROM account_sessions s JOIN account_owners o ON o.uuid=s.account_uuid AND o.active WHERE s.id_hash=$1 AND s.world=$2 FOR UPDATE OF s"
    );
    let row =
        sqlx::query(sqlx::AssertSqlSafe(query)).bind(&id).bind(&state.accounts.world).fetch_optional(&mut *tx).await?;
    let mut current = row.and_then(|r| decode(state, r)).ok_or_else(expired)?;
    if current.expires_at <= Utc::now() {
        return Err(expired());
    }
    if current.access_expires_at - Utc::now() < MARGIN {
        match state.accounts.refresh(&current.ort).await {
            Ok(tokens) => {
                if tokens.account.uuid != current.uuid || tokens.account.kind != current.kind {
                    return Err(ApiError::new(
                        StatusCode::BAD_GATEWAY,
                        "accounts_inconsistent",
                        "Silicon Accounts returned a different account during refresh",
                    ));
                }
                save_tokens(state, &mut tx, &current.id_hash, &tokens).await?;
                current.actor = tokens.account.id;
                current.oat = tokens.oat;
                current.ort = tokens.ort;
                current.access_expires_at = tokens.access_expires_at;
                current.expires_at = tokens.expires_at;
                current.checked_at = Utc::now();
            }
            Err(e) if e.is_invalid_grant() => {
                sqlx::query("DELETE FROM account_sessions WHERE id_hash=$1").bind(&id).execute(&mut *tx).await?;
                tx.commit().await?;
                return Err(expired());
            }
            Err(e) => return Err(e.into()),
        }
    } else if Utc::now() - current.checked_at >= MARGIN {
        let proof = state.accounts.introspect(&current.oat).await?;
        if !proof.active {
            sqlx::query("DELETE FROM account_sessions WHERE id_hash=$1").bind(&id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Err(expired());
        }
        if proof.sub != Some(current.uuid) || proof.kind != Some(current.kind) {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "accounts_inconsistent",
                "Silicon Accounts returned a different token owner",
            ));
        }
        sqlx::query("UPDATE account_sessions SET checked_at=now() WHERE id_hash=$1")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        current.checked_at = Utc::now();
    }
    tx.commit().await?;
    Ok(Some(current))
}
async fn save_tokens(state: &AppState, tx: &mut PgConnection, id: &str, tokens: &Tokens) -> Result<(), ApiError> {
    sqlx::query("UPDATE account_sessions SET actor=$2,access_token_enc=$3,refresh_token_enc=$4,access_expires_at=$5,expires_at=$6,checked_at=now() WHERE id_hash=$1")
        .bind(id).bind(&tokens.account.id).bind(crypto::seal(&state.cfg.key,&tokens.oat)).bind(crypto::seal(&state.cfg.key,&tokens.ort)).bind(tokens.access_expires_at).bind(tokens.expires_at).execute(&mut *tx).await?;
    sqlx::query("UPDATE account_owners SET actor=$2,updated_at=now() WHERE uuid=$1")
        .bind(tokens.account.uuid)
        .bind(&tokens.account.id)
        .execute(tx)
        .await?;
    Ok(())
}
pub fn guard_context(headers: &HeaderMap, secret: &str) -> Result<(), ApiError> {
    if headers.get("x-spacestation-context").and_then(|v| v.to_str().ok()) != Some(sha256_hex(secret).as_str()) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "context_changed",
            "The selected account changed. Reload this page before continuing.",
        ));
    }
    Ok(())
}
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let cli = bearer(&headers);
    if cli.is_some_and(|s| !s.starts_with(CLI_PREFIX)) {
        return Err(ApiError::unauthorized("unsupported_bearer", "log out with a session credential"));
    }
    if cli.is_none() && !origin_ok(&headers, &state.cfg.origin) {
        return Err(bad_origin());
    }
    let secret = cli.map(str::to_owned).or_else(|| cookie_value(&headers));
    if let Some(secret) = secret {
        if cli.is_none() {
            guard_context(&headers, &secret)?;
        }
        let token: Option<String> =
            sqlx::query_scalar("DELETE FROM account_sessions WHERE id_hash=$1 RETURNING refresh_token_enc")
                .bind(sha256_hex(&secret))
                .fetch_optional(&state.store.pg)
                .await?;
        if let Some(token) = token.and_then(|t| crypto::open(&state.cfg.key, &t)) {
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(e) = state.accounts.revoke(&token).await {
                    tracing::warn!("account logout revocation unavailable: {e:?}");
                }
            });
        }
    }
    if cli.is_some() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let next:Option<(String,DateTime<Utc>)>=sqlx::query_as("SELECT secret_enc,expires_at FROM account_sessions WHERE browser_group=$1 AND world=$2 AND expires_at>now() ORDER BY created_at DESC LIMIT 1").bind(sha256_hex(&group(&headers))).bind(&state.accounts.world).fetch_optional(&state.store.pg).await?;
    let (secret, age) =
        next.and_then(|(s, e)| crypto::open(&state.cfg.key, &s).map(|s| (s, max_age(e)))).unwrap_or_default();
    Ok(([cookie(&state, COOKIE, &secret, "/", age)], StatusCode::NO_CONTENT).into_response())
}
async fn contexts(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let selected = cookie_value(&headers).map(|s| sha256_hex(&s));
    let rows:Vec<(String,Uuid,String,DateTime<Utc>)>=sqlx::query_as("SELECT s.id_hash,o.uuid,o.actor,s.expires_at FROM account_sessions s JOIN account_owners o ON o.uuid=s.account_uuid AND o.active WHERE s.browser_group=$1 AND s.world=$2 AND s.expires_at>now() ORDER BY s.created_at DESC").bind(sha256_hex(&group(&headers))).bind(&state.accounts.world).fetch_all(&state.store.pg).await?;
    Ok(Json(json!(rows.into_iter().map(|(context_id,uuid,actor,expires_at)|json!({"selected":selected.as_ref()==Some(&context_id),"context_id":context_id,"uuid":uuid,"actor":actor,"expires_at":expires_at})).collect::<Vec<_>>())))
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
    // Selection names its target explicitly; the saved-account cookie proves ownership even
    // after the previously selected session expired or was revoked.
    let secret:Option<String>=sqlx::query_scalar("SELECT secret_enc FROM account_sessions WHERE id_hash=$1 AND browser_group=$2 AND world=$3 AND expires_at>now()").bind(&body.context_id).bind(sha256_hex(&group(&headers))).bind(&state.accounts.world).fetch_optional(&state.store.pg).await?;
    let secret = secret.and_then(|s| crypto::open(&state.cfg.key, &s)).ok_or_else(expired)?;
    let s = load(&state, &secret).await?.ok_or_else(expired)?;
    Ok(([cookie(&state, COOKIE, &secret, "/", max_age(s.expires_at))], StatusCode::NO_CONTENT).into_response())
}
