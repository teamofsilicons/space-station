//! Who is calling. One extractor turns the credential on a request into a `Principal`: the
//! `ss_session` cookie (under the Origin rule for anything that mutates or upgrades) and an
//! `sscli-` bearer are the same session row from a browser and from a terminal, `spacewindow-` is
//! an access token, `apikey-` the org itself within its scopes; any other bearer is refused,
//! because Space Station never receives an IAM token. A session is bound to one org, so a path
//! naming another is `not_a_member`. Plus the three "who am I" routes.

use std::collections::HashMap;

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::routing::get;
use axum::{RequestPartsExt, Router};
use serde_json::{Value, json};

use super::{ApiError, AppState, Json};
use crate::iam::session::{self, Session};
use crate::iam::{self, Identity};
use crate::sql::valid_org;
use crate::{access, tokens};

pub enum Principal {
    Actor(Identity),
    Org { org: String, scopes: Vec<String> },
}

pub struct Auth(pub Principal);

impl Auth {
    pub fn org(&self) -> &str {
        match &self.0 {
            Principal::Actor(i) => &i.org,
            Principal::Org { org, .. } => org,
        }
    }

    /// The carbon or silicon; an API key is refused.
    pub fn actor(&self) -> Result<&Identity, ApiError> {
        match &self.0 {
            Principal::Actor(i) => Ok(i),
            Principal::Org { .. } => Err(key_refused()),
        }
    }

    /// An actor, or an API key holding `scope`.
    pub fn allow(&self, scope: &str) -> Result<(), ApiError> {
        match &self.0 {
            Principal::Actor(_) => Ok(()),
            Principal::Org { scopes, .. } if scopes.iter().any(|s| s == scope) => Ok(()),
            Principal::Org { .. } => Err(key_refused()),
        }
    }

    /// The tables this caller may read; `None` for an API key, which sees the whole org.
    pub async fn visible_tables(&self, state: &AppState) -> Result<Option<Vec<String>>, ApiError> {
        match &self.0 {
            Principal::Actor(i) => access::visible_tables(state, i).await.map(Some),
            Principal::Org { .. } => Ok(None),
        }
    }
}

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Auth, ApiError> {
        let path =
            parts.extract::<axum::extract::Path<HashMap<String, String>>>().await.map(|p| p.0).unwrap_or_default();
        let query =
            parts.extract::<axum::extract::Query<HashMap<String, String>>>().await.map(|q| q.0).unwrap_or_default();
        let org = path.get("org").or_else(|| query.get("org")).map(String::as_str).unwrap_or_default();
        let guarded =
            parts.headers.contains_key(header::UPGRADE) || !matches!(parts.method, Method::GET | Method::HEAD);
        authenticate(state, &parts.headers, org, guarded).await.map(Auth)
    }
}

/// The principal behind `headers` for `org`. `guarded` applies the Origin rule to a cookie.
pub async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    org: &str,
    guarded: bool,
) -> Result<Principal, ApiError> {
    if !valid_org(org) {
        return Err(invalid_org());
    }
    if let Some(token) = bearer(headers) {
        return if token.starts_with(session::CLI_PREFIX) {
            // The same session row as the cookie, and the same refresh — but no browser ever
            // attaches an Authorization header on its own, so there is no CSRF here to prevent
            // and the Origin rule does not apply.
            let session = session::load(state, token).await?.ok_or_else(unauthenticated)?;
            Ok(Principal::Actor(bound(state, &session, org).await?))
        } else if token.starts_with("spacewindow-") {
            Ok(Principal::Actor(tokens::by_access_token(state, token, Some(org)).await?))
        } else if token.starts_with("apikey-") {
            Ok(Principal::Org { org: org.to_owned(), scopes: tokens::by_api_key(state, token, org).await? })
        } else {
            Err(unsupported_bearer())
        };
    }
    if guarded && !origin_ok(headers, &state.cfg.origin) {
        return Err(bad_origin());
    }
    let cookie = session::cookie_value(headers).ok_or_else(unauthenticated)?;
    let session = session::load(state, &cookie).await?.ok_or_else(unauthenticated)?;
    Ok(Principal::Actor(bound(state, &session, org).await?))
}

/// The session's actor in `org`, which must be the org the login bound it to. Tags come from the
/// directory mirror, which the session's own snapshot wrote at login and rewrites at every
/// re-proof and a webhook updates in between — so a tag IAM takes away is gone here as soon as the
/// webhook lands, not a minute later. The snapshot the row kept is the answer only while the
/// mirror holds nothing for the member.
async fn bound(state: &AppState, session: &Session, org: &str) -> Result<Identity, ApiError> {
    if session.org != org {
        let message = format!("this session is bound to {}; log in to {org} to work there", session.org);
        return Err(ApiError::new(StatusCode::FORBIDDEN, "not_a_member", message));
    }
    let mirrored = iam::mirrored_tags(state, org, &session.actor).await?;
    let tags = mirrored.or_else(|| session.tags.clone()).unwrap_or_default();
    Ok(Identity { kind: session.kind, id: session.actor.clone(), org: org.to_owned(), tags })
}

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}

/// `Origin` equals `SS_ORIGIN`, or the browser vouches with `Sec-Fetch-Site: same-origin`.
pub fn origin_ok(headers: &HeaderMap, origin: &str) -> bool {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    header("origin") == Some(origin) || header("sec-fetch-site") == Some("same-origin")
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

pub fn invalid_org() -> ApiError {
    ApiError::bad_request("invalid_org", "org ids are 3-50 of [a-z0-9_-]")
}

fn key_refused() -> ApiError {
    ApiError::unauthorized("unauthorized", "this route is not available to API keys")
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/me", get(me)).route("/orgs", get(orgs)).route("/orgs/{org}/me", get(org_me))
}

async fn org_me(auth: Auth) -> Result<Json<Identity>, ApiError> {
    auth.actor().cloned().map(Json)
}

/// The session behind an `sscli-` bearer or the `ss_session` cookie — the two ways an actor
/// speaks for itself, in a terminal and in a browser.
async fn own_session(state: &AppState, headers: &HeaderMap) -> Result<Session, ApiError> {
    let secret = match bearer(headers) {
        Some(token) if token.starts_with(session::CLI_PREFIX) => token.to_owned(),
        Some(_) => return Err(unsupported_bearer()),
        None => session::cookie_value(headers).ok_or_else(unauthenticated)?,
    };
    session::load(state, &secret).await?.ok_or_else(unauthenticated)
}

/// `{id, kind, org, app}` from the session or an access token. `app` is where the UI lives, so
/// the package can build links without configuring it; `org` is the one this credential is bound to.
async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let (id, kind, org) = match bearer(&headers) {
        Some(token) if token.starts_with("spacewindow-") => {
            let i = tokens::by_access_token(&state, token, None).await?;
            (i.id, i.kind, i.org)
        }
        _ => {
            let s = own_session(&state, &headers).await?;
            (s.actor, s.kind, s.org)
        }
    };
    Ok(Json(json!({"id": id, "kind": kind, "org": org, "app": state.cfg.origin})))
}

/// The orgs where the mirror holds an active membership for this actor, plus the session's own:
/// `[{id, name}]`, the name from the mirror when IAM has told us one. The frontend's logged-out
/// signal is this route's 401.
const ORGS: &str = "SELECT o.org, coalesce((SELECT m.org_name FROM iam_members m WHERE m.org = o.org AND m.org_name IS NOT NULL \
                    ORDER BY m.updated_at DESC LIMIT 1), o.org) \
                    FROM (SELECT org FROM iam_members WHERE actor = $1 AND status = 'active' UNION SELECT $2::text) o \
                    ORDER BY 1";

async fn orgs(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<Value>>, ApiError> {
    let s = own_session(&state, &headers).await?;
    let rows: Vec<(String, String)> =
        sqlx::query_as(ORGS).bind(&s.actor).bind(&s.org).fetch_all(&state.store.pg).await?;
    Ok(Json(rows.into_iter().map(|(id, name)| json!({"id": id, "name": name})).collect()))
}
