//! Every credential resolves to one immutable Carbon or Silicon account owner.
use super::{ApiError, AppState, Json};
use crate::accounts::{Identity, session};
use crate::{access, tokens};
use axum::Router;
use axum::extract::{FromRequestParts, State};
use axum::http::{HeaderMap, Method, StatusCode, header, request::Parts};
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::get;
use serde_json::json;

pub enum Principal {
    Actor(Identity),
    Key { org: String, scopes: Vec<String> },
}
pub struct Auth(pub Principal);
impl Auth {
    pub fn org(&self) -> &str {
        match &self.0 {
            Principal::Actor(i) => &i.org,
            Principal::Key { org, .. } => org,
        }
    }
    pub fn actor(&self) -> Result<&Identity, ApiError> {
        match &self.0 {
            Principal::Actor(i) => Ok(i),
            _ => Err(key_refused()),
        }
    }
    pub fn allow(&self, scope: &str) -> Result<(), ApiError> {
        match &self.0 {
            Principal::Actor(_) => Ok(()),
            Principal::Key { scopes, .. } if scopes.iter().any(|s| s == scope) => Ok(()),
            _ => Err(key_refused()),
        }
    }
    pub async fn visible_tables(&self, state: &AppState) -> Result<Option<Vec<String>>, ApiError> {
        match &self.0 {
            Principal::Actor(i) => access::visible_tables(state, i).await.map(Some),
            _ => Ok(None),
        }
    }
}
impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let guarded =
            parts.headers.contains_key(header::UPGRADE) || !matches!(parts.method, Method::GET | Method::HEAD);
        authenticate(state, &parts.headers, "", guarded).await.map(Auth)
    }
}
pub async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    _namespace: &str,
    guarded: bool,
) -> Result<Principal, ApiError> {
    if let Some(token) = bearer(headers) {
        if token.starts_with(session::CLI_PREFIX) {
            let s = session::load(state, token).await?.ok_or_else(unauthenticated)?;
            return Ok(Principal::Actor(s.identity()));
        }
        if token.starts_with("spacewindow-") {
            return Ok(Principal::Actor(tokens::by_access_token(state, token, None).await?));
        }
        if token.starts_with("apikey-") {
            let (org, scopes) = tokens::by_api_key(state, token).await?;
            return Ok(Principal::Key { org, scopes });
        }
        return Err(unsupported_bearer());
    }
    if guarded && !origin_ok(headers, &state.cfg.origin) {
        return Err(bad_origin());
    }
    let cookie = session::cookie_value(headers).ok_or_else(unauthenticated)?;
    session::guard_context(headers, &cookie)?;
    let s = session::load(state, &cookie).await?.ok_or_else(unauthenticated)?;
    Ok(Principal::Actor(s.identity()))
}
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}
pub fn origin_ok(headers: &HeaderMap, origin: &str) -> bool {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    get("origin") == Some(origin) || get("sec-fetch-site") == Some("same-origin")
}
pub fn bad_origin() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "bad_origin", "Origin does not match SS_ORIGIN")
}
fn unauthenticated() -> ApiError {
    ApiError::unauthorized("unauthenticated", "log in")
}
fn unsupported_bearer() -> ApiError {
    ApiError::unauthorized("unsupported_bearer", "only sscli-, spacewindow- and apikey- bearers are accepted")
}
fn key_refused() -> ApiError {
    ApiError::unauthorized("unauthorized", "this route is not available to API keys")
}
pub fn routes() -> Router<AppState> {
    Router::new().route("/me", get(me))
}
async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    if let Some(token) = bearer(&headers) {
        if token.starts_with("spacewindow-") {
            let i = tokens::by_access_token(&state, token, None).await?;
            return Ok(Json(json!({"id":i.id,"uuid":i.uuid,"kind":i.kind,"app":state.cfg.origin,"context_id":null,"expires_at":null})).into_response());
        }
        if !token.starts_with(session::CLI_PREFIX) {
            return Err(unsupported_bearer());
        }
    }
    let secret =
        bearer(&headers).map(str::to_owned).or_else(|| session::cookie_value(&headers)).ok_or_else(unauthenticated)?;
    let s = session::load(&state, &secret).await?.ok_or_else(unauthenticated)?;
    let cookies =
        if bearer(&headers).is_none() { session::renew_cookies(&state, &headers, &secret, &s) } else { vec![] };
    Ok((AppendHeaders(cookies), [(header::CACHE_CONTROL, "no-store")], Json(s.public(&state))).into_response())
}
