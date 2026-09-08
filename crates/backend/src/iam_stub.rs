//! A local Silicon IAM look-alike for development and tests, written against the live contract
//! observed on 2026-09-05 (docs/ARCHITECTURE.md, "IAM stub"). One seeded Application signs
//! people in through short-lived tokens: `GET /api/v1/login` stands in for IAM's login page,
//! `POST /api/v1/app-auth/short-lived-tokens` mints one for a signed-in carbon or silicon, and
//! `POST /api/v1/app-auth/tokens` exchanges or refreshes it with rotating, reuse-fatal refresh
//! tokens. Introspection, revocation, the carbon and silicon logins and the directory reads answer
//! with the real service's statuses, codes and wording, including the `403 forbidden` every
//! directory route gives an Application token and a silicon's own bearer gets at `/logout`.
//! Introspecting an org-bound access token returns the `authorization` snapshot (public id, role,
//! tags, membership version); a refresh token, an unscoped token or a dead one carries none. Like
//! the live service, introspection answers `400 invalid_request` to a malformed or duplicated
//! `X-Org-ID` (or an unsupported `token_type_hint`) and `{"active": false}` to a well-formed one
//! that is not the token's org. `POST /_stub/deliver` signs and posts a webhook in either
//! envelope — full member rows, or for a `*.removed.v1` event the `authorization: "removed"`
//! tombstones IAM really sends — so the directory mirror is testable without a tunnel; what a
//! delivery says about a member (status, tags, version) is what the snapshot reports from then on,
//! because IAM's snapshot is never older than its webhooks. Identity comes from a JSON seed
//! (`fixtures/iam-seed.json` by default); tokens, idempotency records and delivered changes live in
//! memory and die with the process. `X-Testing-Environment-Key` is accepted and ignored: there is
//! one plane. The official `silicon-iam-client` drives it end to end (a test proves it).
//! `cargo run -p space-station-backend --example iam-stub`.

use std::{
    collections::{BTreeMap, HashMap},
    io,
    net::SocketAddr,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use space_station_shared::secrets::sha256_hex;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::crypto::hmac_sha256_hex;

/// Everything the stub knows about the world. Loaded once, never mutated.
#[derive(Clone, Serialize, Deserialize)]
pub struct Seed {
    pub app: App,
    pub orgs: Vec<Org>,
    pub carbons: Vec<Carbon>,
    pub silicons: Vec<Silicon>,
    /// How long an access token really lives. Responses always claim IAM's 1800 s; tests shorten
    /// this to exercise expiry.
    #[serde(default = "default_ttl")]
    pub access_ttl_secs: i64,
}

fn default_ttl() -> i64 {
    1800
}

/// The one registered Application: its canonical `{org}>{handle}` id, the `ask_` secret it
/// presents as HTTP Basic, the secret its webhooks are signed with, and the key a testing
/// environment stamps into the `test` envelope (without one the stub cannot send that envelope).
#[derive(Clone, Serialize, Deserialize)]
pub struct App {
    pub app_id: String,
    pub secret: String,
    pub webhook_secret: String,
    #[serde(default)]
    pub testing_key: Option<String>,
}

/// An organization and its tag catalogue.
#[derive(Clone, Serialize, Deserialize)]
pub struct Org {
    pub org_id: String,
    pub name: String,
    pub tags: Vec<String>,
}

/// A human; `memberships` is keyed by `org_id`. Any `{carbon_id}@…` address signs them in.
#[derive(Clone, Serialize, Deserialize)]
pub struct Carbon {
    pub carbon_id: String,
    pub name: String,
    pub memberships: BTreeMap<String, Membership>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Membership {
    pub tags: Vec<String>,
    pub org_role: String,
}

/// A machine identity `handle:org_id`: its org is the suffix, its role is always `member`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Silicon {
    pub silicon_id: String,
    pub name: String,
    pub token: String,
    pub tags: Vec<String>,
}

/// The seed embedded from `fixtures/iam-seed.json`.
pub fn default_seed() -> Seed {
    serde_json::from_str(include_str!("../fixtures/iam-seed.json")).expect("the seed is valid")
}

pub fn seed_from_file(path: impl AsRef<std::path::Path>) -> io::Result<Seed> {
    serde_json::from_str(&std::fs::read_to_string(path)?).map_err(io::Error::other)
}

/// Binds `addr` (port 0 picks a free one) and serves on a background task until that task is
/// aborted. Returns the bound address.
pub async fn serve(seed: Seed, addr: SocketAddr) -> io::Result<(SocketAddr, JoinHandle<()>)> {
    let stub = Stub { seed, http: reqwest::Client::new(), state: Mutex::default() };
    listen(router(Arc::new(stub)), addr).await
}

async fn listen(app: Router, addr: SocketAddr) -> io::Result<(SocketAddr, JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("iam stub stopped: {e}");
        }
    });
    Ok((addr, task))
}

fn router(stub: Shared) -> Router {
    let directory = "/api/v1/organizations/{org}/directory";
    Router::new()
        .route("/api/version", get(version))
        .route("/api/v1/login", get(login))
        .route("/api/v1/login/challenges", post(challenge))
        .route("/api/v1/login/challenges/{session}/verify", post(verify))
        .route("/api/v1/auth/tokens/refresh", post(refresh))
        .route("/api/v1/logout", post(logout))
        .route("/api/v1/silicon-auth/token", post(silicon_auth))
        .route("/api/v1/app-auth/short-lived-tokens", post(short_lived))
        .route("/api/v1/app-auth/tokens", post(tokens))
        .route("/api/v1/oauth/introspect", post(introspect))
        .route("/api/v1/oauth/revoke", post(revoke))
        .route("/api/v1/me", get(me))
        .route("/api/v1/organizations", get(organizations))
        .route(&format!("{directory}/self"), get(directory_self))
        .route(&format!("{directory}/members"), get(directory_members))
        .route("/_stub/deliver", post(deliver))
        .fallback(|| async { NOT_FOUND })
        .with_state(stub)
}

type Shared = Arc<Stub>;
type Params = Query<HashMap<String, String>>;
/// A fresh (access token, refresh token, family) triple.
type Minted = (String, String, Uuid);
/// Token prefixes of a family: (access, refresh).
type Pair = (&'static str, &'static str);

struct Stub {
    seed: Seed,
    http: reqwest::Client,
    state: Mutex<Ledger>,
}

/// Everything minted since boot. A `Family` is one IAM session (`cat_`/`sat_`) or one Application
/// session (`oat_`/`ort_`) descended from an IAM session; deleting it revokes every token that
/// points at it, so a token found in `access` or `refresh` always has a live family.
#[derive(Default)]
struct Ledger {
    /// Login challenge → (carbon, expires_at).
    challenges: HashMap<Uuid, (String, i64)>,
    families: HashMap<Uuid, Family>,
    access: HashMap<String, Token>,
    refresh: HashMap<String, Token>,
    /// Short-lived login tokens, each good for one exchange.
    slts: HashMap<String, Slt>,
    /// `Idempotency-Key` → (request digest, status, body) of the answer it got.
    replays: HashMap<String, (String, StatusCode, Value)>,
    /// (org, actor) → what the latest `/_stub/deliver` row said about that member.
    delivered: HashMap<(String, String), Delivered>,
}

struct Family {
    actor: String,
    org: Option<String>,
    /// The IAM session this family is, or descends from: what introspection reports.
    session: Uuid,
    prefixes: Pair,
    /// When the refresh family ends: 900 days after it opened.
    until: i64,
}

struct Token {
    family: Uuid,
    issued_at: i64,
    expires_at: i64,
    /// A refresh token is consumed by the rotation that replaced it.
    consumed: bool,
}

struct Slt {
    actor: String,
    org: Option<String>,
    session: Uuid,
    expires_at: i64,
}

/// A membership as a webhook last described it; introspection's snapshot repeats it.
struct Delivered {
    status: String,
    tags: Vec<String>,
    version: u64,
}

const CARBON: Pair = ("cat_", "rft_");
const SILICON: Pair = ("sat_", "rft_");
const OAUTH: Pair = ("oat_", "ort_");
/// The whole catalogue: an Application login has no scope to negotiate.
const SCOPE: &str = "email memberships.read obo.issue offline_access organizations.read phone profile roles.read";
/// When everything seeded was "created".
const EPOCH: &str = "2026-01-01T00:00:00Z";
const NO_STORE: [(header::HeaderName, &str); 2] = [(header::CACHE_CONTROL, "no-store"), (header::PRAGMA, "no-cache")];
const DIRECTORY_FIELDS: [&str; 6] = ["name", "id", "role", "org", "tags", "trust"];

/// An IAM error, `{error: {code, message, request_id}}`, with the live service's status, code and
/// wording.
struct Fail(StatusCode, &'static str, &'static str);

const UNAUTHENTICATED: Fail = Fail(StatusCode::UNAUTHORIZED, "unauthenticated", "Authentication is required.");
const INVALID_CLIENT: Fail = Fail(StatusCode::UNAUTHORIZED, "invalid_client", "Application authentication failed.");
const FORBIDDEN: Fail = Fail(StatusCode::FORBIDDEN, "forbidden", "The actor is not authorized for this action.");
const ORG_FORBIDDEN: Fail =
    Fail(StatusCode::FORBIDDEN, "organization_context_forbidden", "The actor is not authorized for this action.");
const NOT_FOUND: Fail = Fail(StatusCode::NOT_FOUND, "not_found", "The requested resource was not found.");
const REJECTED: Fail = Fail(StatusCode::UNSUPPORTED_MEDIA_TYPE, "request_rejected", "The HTTP request was rejected.");
const INVALID: Fail = Fail(StatusCode::UNPROCESSABLE_ENTITY, "validation_failed", "The request contains invalid data.");
const UNKNOWN_APP: Fail = Fail(StatusCode::BAD_REQUEST, "invalid_request", "The application is unknown.");
const ONE_GRANT: Fail =
    Fail(StatusCode::BAD_REQUEST, "invalid_request", "Present exactly one of slt and refresh_token.");
const BAD_CONTEXT: Fail = Fail(
    StatusCode::BAD_REQUEST,
    "invalid_request",
    "The token-type hint or X-Org-ID header is malformed or duplicated.",
);
const BAD_SLT: Fail = Fail(StatusCode::BAD_REQUEST, "invalid_grant", "The short-lived token is invalid.");
const BAD_REFRESH: Fail = Fail(StatusCode::BAD_REQUEST, "invalid_grant", "The refresh token is invalid.");
const REUSED_REFRESH: Fail =
    Fail(StatusCode::BAD_REQUEST, "invalid_grant", "Refresh-token reuse was detected and the family was revoked.");
const CHALLENGE_GONE: Fail =
    Fail(StatusCode::GONE, "challenge_expired", "The requested resource is no longer available.");
const KEY_REQUIRED: Fail =
    Fail(StatusCode::PRECONDITION_REQUIRED, "precondition_required", "A required request precondition is missing.");
const CONFLICT: Fail =
    Fail(StatusCode::CONFLICT, "idempotency_conflict", "The request conflicts with the current resource state.");
const NOT_ACCEPTABLE: Fail = Fail(
    StatusCode::NOT_ACCEPTABLE,
    "api_version_not_acceptable",
    "The client and server do not support a common API version.",
);
const DELIVERY_FAILED: Fail =
    Fail(StatusCode::BAD_GATEWAY, "delivery_failed", "The webhook receiver could not be reached.");

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        let Fail(status, code, message) = self;
        let body = json!({"error": {"code": code, "message": message, "request_id": Uuid::now_v7()}});
        let mut res = (status, Json(body)).into_response();
        let challenge = match code {
            "unauthenticated" => Some("Bearer"),
            "invalid_client" => Some("Basic realm=\"silicon-iam\""),
            _ => None,
        };
        if let Some(challenge) = challenge {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
        }
        res
    }
}

fn now() -> i64 {
    Utc::now().timestamp()
}

fn rfc3339(secs: i64) -> String {
    DateTime::from_timestamp(secs, 0).expect("a timestamp in range").to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A fresh opaque secret: `prefix` plus 43 URL-safe characters from 32 random bytes.
fn mint(prefix: &str) -> String {
    let bytes = [Uuid::new_v4().into_bytes(), Uuid::new_v4().into_bytes()].concat();
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// The same uuid on every boot for the same name: the ids IAM would have minted once.
fn stable_id(name: &str) -> Uuid {
    let digest = Sha256::digest(name.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_sha1_bytes(bytes).into_uuid()
}

fn org_uuid(org: &str) -> Uuid {
    stable_id(&format!("org:{org}"))
}

fn principal_id(actor: &str) -> Uuid {
    stable_id(&format!("principal:{actor}"))
}

/// The membership row IAM would have: the same id from introspection and from a webhook.
fn membership_id(org: &str, actor: &str) -> Uuid {
    stable_id(&format!("membership:{org}:{actor}"))
}

/// The kind comes from the colon, as everywhere in Space Station.
fn kind(actor: &str) -> &'static str {
    if actor.contains(':') { "silicon" } else { "carbon" }
}

/// An `ActorRef`.
fn actor_json(actor: &str) -> Value {
    json!({"principal_id": principal_id(actor), "type": kind(actor), "public_id": actor})
}

/// A `TagSummary`; tags belong to an org.
fn tag_json(org: &str, name: &str) -> Value {
    json!({"id": stable_id(&format!("tag:{org}:{name}")), "name": name})
}

/// The contract's `OrgId`: `^[a-z0-9_-]{3,50}$`.
fn is_org_id(value: &[u8]) -> bool {
    (3..=50).contains(&value.len())
        && value.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

/// The `resource` reference every member row carries: the membership, its principal, its version.
fn resource(org: &str, actor: &str, status: &str, version: u64) -> Value {
    json!({
        "id": membership_id(org, actor), "principal_id": principal_id(actor), "principal_type": kind(actor),
        "status": status, "type": "organization_membership", "version": version,
    })
}

/// A `*.removed.v1` row: the Application may no longer read the member, so IAM sends only
/// `authorization: "removed"` and the resource reference — no principal, organization or membership.
fn tombstone(org: &str, actor: &str, version: u64) -> Value {
    json!({"authorization": "removed", "resource": resource(org, actor, "removed", version)})
}

fn inactive() -> Json<Value> {
    Json(json!({"active": false}))
}

fn page(items: Vec<Value>) -> Json<Value> {
    Json(json!({"items": items, "page": {"next_cursor": null, "has_more": false}}))
}

fn escape(html: &str) -> String {
    html.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// A route-scoped digest of a request body, so replays never keep the body itself.
fn digest(route: &str, body: &[u8]) -> String {
    sha256_hex(&format!("{route} {}", String::from_utf8_lossy(body)))
}

fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    let sent = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default();
    sent.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case(expected))
}

/// A JSON body, or the service's `415 request_rejected` for any other media type.
fn json_body<T: DeserializeOwned>(headers: &HeaderMap, body: &[u8]) -> Result<T, Fail> {
    if !content_type_is(headers, "application/json") {
        return Err(REJECTED);
    }
    serde_json::from_slice(body).map_err(|_| INVALID)
}

/// A form-encoded body, or `415 request_rejected`: JSON to a token route is the classic mistake.
fn form_body(headers: &HeaderMap, body: &[u8]) -> Result<HashMap<String, String>, Fail> {
    let form = || url::form_urlencoded::parse(body).into_owned().collect();
    content_type_is(headers, "application/x-www-form-urlencoded").then(form).ok_or(REJECTED)
}

/// 16–255 printable ASCII bytes; absent is `428 precondition_required`, malformed is `422`.
fn idempotency_key(headers: &HeaderMap) -> Result<&str, Fail> {
    let key = headers.get("idempotency-key").and_then(|v| v.to_str().ok()).ok_or(KEY_REQUIRED)?;
    let well_formed = (16..=255).contains(&key.len()) && key.bytes().all(|b| (0x21..=0x7e).contains(&b));
    well_formed.then_some(key).ok_or(INVALID)
}

/// The directory's sparse projection list, validated like the service does.
fn fields(q: &HashMap<String, String>) -> Result<Option<&str>, Fail> {
    let fields = q.get("fields").map(String::as_str);
    let valid = fields.is_none_or(|f| f.split(',').all(|name| DIRECTORY_FIELDS.contains(&name)));
    valid.then_some(fields).ok_or(INVALID)
}

fn respond(status: StatusCode, body: &Value, replayed: bool) -> Response {
    let mut res = if body.is_null() { status.into_response() } else { (status, Json(body)).into_response() };
    if replayed {
        res.headers_mut().insert("idempotency-replayed", HeaderValue::from_static("true"));
    }
    res
}

impl Stub {
    fn lock(&self) -> MutexGuard<'_, Ledger> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// HTTP Basic with the canonical app id and the `ask_` secret; anything else is `invalid_client`.
    fn app_auth(&self, headers: &HeaderMap) -> Result<(), Fail> {
        let creds = format!("{}:{}", self.seed.app.app_id, self.seed.app.secret);
        let expected = format!("Basic {}", STANDARD.encode(creds));
        let sent = headers.get(header::AUTHORIZATION).map(|v| v.as_bytes());
        (sent == Some(expected.as_bytes())).then_some(()).ok_or(INVALID_CLIENT)
    }

    /// The actor behind a live IAM bearer. An Application token is `forbidden` everywhere the
    /// directory is read; no token, an expired one or an unknown one is `unauthenticated`.
    fn iam_actor(&self, headers: &HeaderMap) -> Result<String, Fail> {
        let st = self.lock();
        let (_, family) = st.bearer(headers).ok_or(UNAUTHENTICATED)?;
        if family.prefixes == OAUTH {
            return Err(FORBIDDEN);
        }
        Ok(family.actor.clone())
    }

    fn org(&self, org_id: &str) -> Option<&Org> {
        self.seed.orgs.iter().find(|o| o.org_id == org_id)
    }

    fn carbon(&self, carbon_id: &str) -> Option<&Carbon> {
        self.seed.carbons.iter().find(|c| c.carbon_id == carbon_id)
    }

    /// Every seeded actor id, carbons first.
    fn actors(&self) -> impl Iterator<Item = &str> {
        let carbons = self.seed.carbons.iter().map(|c| c.carbon_id.as_str());
        carbons.chain(self.seed.silicons.iter().map(|s| s.silicon_id.as_str()))
    }

    /// `actor`'s seeded membership in `org`: display name, org role and tag names.
    fn member(&self, org: &str, actor: &str) -> Option<(&str, &str, &[String])> {
        match actor.rsplit_once(':') {
            Some((_, suffix)) => {
                let s = self.seed.silicons.iter().find(|s| s.silicon_id == actor)?;
                (suffix == org).then_some((s.name.as_str(), "member", s.tags.as_slice()))
            }
            None => {
                let c = self.carbon(actor)?;
                let m = c.memberships.get(org)?;
                Some((c.name.as_str(), m.org_role.as_str(), m.tags.as_slice()))
            }
        }
    }

    /// An `Organization`: the seeded facts plus the fixed fields the schema requires.
    fn org_json(&self, o: &Org) -> Value {
        let owns = |c: &&Carbon| c.memberships.get(&o.org_id).is_some_and(|m| m.org_role == "owner");
        let owner = self.seed.carbons.iter().find(owns).map(|c| membership_id(&o.org_id, &c.carbon_id));
        json!({
            "id": org_uuid(&o.org_id), "org_id": o.org_id, "name": o.name, "logo": null, "description": null,
            "owner_membership_id": owner, "join_method": "email", "sso_status": "disabled", "status": "active",
            "version": 1, "created_at": EPOCH, "updated_at": EPOCH,
        })
    }

    /// `actor`'s `DirectoryMember` row in `org`, sparse when `fields` is given; `None` for non-members.
    fn directory_row(&self, org: &Org, actor: &str, fields: Option<&str>) -> Option<Value> {
        let (name, role, tags) = self.member(&org.org_id, actor)?;
        let mut row = json!({
            "name": name, "id": actor, "role": {"org_role": role, "job_role": ""},
            "org": {"id": org.org_id, "name": org.name}, "trust": null,
            "tags": tags.iter().map(|t| tag_json(&org.org_id, t)).collect::<Vec<_>>(),
        });
        if let (Some(fields), Some(map)) = (fields, row.as_object_mut()) {
            map.retain(|k, _| fields.split(',').any(|f| f == k));
        }
        Some(row)
    }

    /// One `data.current.members[]` row of a webhook, shaped like the live deliveries: seeded
    /// facts fill in what the request leaves out, so a row about a stranger still verifies.
    fn member_row(&self, org: &str, row: &Row, version: u64, at: &str) -> Value {
        let seeded = self.member(org, &row.actor);
        let name = row.display_name.as_deref().or(seeded.map(|m| m.0)).unwrap_or(&row.actor);
        let role = seeded.map_or("member", |m| m.1);
        let (k, pid, mid) = (kind(&row.actor), principal_id(&row.actor), membership_id(org, &row.actor));
        let trust = json!({"boundary": "internal", "level": "not_trusted"});
        json!({
            "membership": {
                "authorization_epoch": version, "created_at": EPOCH, "extra_silicon_membership_ids": [],
                "first_silicon_membership_id": null, "hierarchy_level": (k == "silicon").then_some(1), "id": mid,
                "removed_at": (row.status == "removed").then_some(at), "reports_to_membership_id": null,
                "status": row.status, "tags": row.tags.iter().map(|t| tag_json(org, t)).collect::<Vec<_>>(),
                "trust": {"applicable_rules": [], "organization_default": trust, "subject_default": trust},
                "updated_at": at, "version": version,
            },
            "organization": {
                "created_at": EPOCH, "description": null, "id": org_uuid(org), "join_method": "email", "logo": null,
                "name": self.org(org).map_or(org, |o| o.name.as_str()), "org_id": org, "status": "active",
                "updated_at": EPOCH, "version": 1,
            },
            "principal": {
                "created_at": EPOCH, "description": null, "display_name": name, "principal_id": pid,
                "profile_photo": format!("https://iam.invalid/pfp/{k}?id={}", row.actor), "public_id": row.actor,
                "status": "active", "timezone": "UTC", "type": k, "updated_at": at, "version": version,
            },
            "resource": resource(org, &row.actor, &row.status, version),
            "roles": {"capabilities": [], "job_role": "", "org_role": role},
        })
    }
}

impl Ledger {
    /// Opens a token family for `actor` and mints its first pair. `session` is the IAM session an
    /// Application family descends from; an IAM family is its own.
    fn open(&mut self, actor: &str, org: Option<String>, session: Option<Uuid>, prefixes: Pair, ttl: i64) -> Minted {
        let id = Uuid::now_v7();
        let until = now() + 900 * 86_400;
        let family = Family { actor: actor.to_owned(), org, session: session.unwrap_or(id), prefixes, until };
        self.families.insert(id, family);
        self.issue(id, ttl)
    }

    fn issue(&mut self, family: Uuid, ttl: i64) -> Minted {
        let f = self.family(family);
        let (access, refresh, until) = (mint(f.prefixes.0), mint(f.prefixes.1), f.until);
        let issued_at = now();
        let token = |expires_at| Token { family, issued_at, expires_at, consumed: false };
        self.access.insert(access.clone(), token(issued_at + ttl));
        self.refresh.insert(refresh.clone(), token(until));
        (access, refresh, family)
    }

    /// Rotates a refresh token. A consumed one means somebody replayed it: the family dies, and
    /// `Err(true)` says so; `Err(false)` is a token this ledger does not know.
    fn rotate(&mut self, token: &str, ttl: i64) -> Result<Minted, bool> {
        let t = self.refresh.get_mut(token).ok_or(false)?;
        let (family, consumed) = (t.family, std::mem::replace(&mut t.consumed, true));
        if consumed {
            self.revoke(family);
            return Err(true);
        }
        Ok(self.issue(family, ttl))
    }

    fn revoke(&mut self, family: Uuid) {
        self.families.remove(&family);
        self.access.retain(|_, t| t.family != family);
        self.refresh.retain(|_, t| t.family != family);
    }

    fn family(&self, id: Uuid) -> &Family {
        self.families.get(&id).expect("every minted token points at a live family")
    }

    /// An unexpired access token and its family.
    fn live(&self, token: &str) -> Option<(&Token, &Family)> {
        let t = self.access.get(token).filter(|t| now() < t.expires_at)?;
        Some((t, self.families.get(&t.family)?))
    }

    fn bearer(&self, headers: &HeaderMap) -> Option<(&Token, &Family)> {
        let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
        self.live(auth.strip_prefix("Bearer ")?)
    }

    /// What introspection calls active: an unexpired access token or an unconsumed refresh token.
    fn active(&self, token: &str) -> Option<(&Token, &Family)> {
        let t = self.access.get(token).or_else(|| self.refresh.get(token))?;
        if t.consumed || now() >= t.expires_at {
            return None;
        }
        Some((t, self.families.get(&t.family)?))
    }

    /// A two-minute, single-use `oac_` token bound to an actor, an org and an IAM session.
    fn slt(&mut self, actor: &str, org: Option<String>, session: Uuid) -> String {
        let token = mint("oac_");
        self.slts.retain(|_, s| now() < s.expires_at);
        self.slts.insert(token.clone(), Slt { actor: actor.to_owned(), org, session, expires_at: now() + 120 });
        token
    }

    /// Spends a short-lived token on a new Application family.
    fn exchange(&mut self, slt: &str, ttl: i64) -> Result<Minted, Fail> {
        let s = self.slts.remove(slt).filter(|s| now() < s.expires_at).ok_or(BAD_SLT)?;
        Ok(self.open(&s.actor, s.org, Some(s.session), OAUTH, ttl))
    }

    /// Remembers what a delivery said about a member; an older version never overwrites a newer one.
    fn record(&mut self, org: &str, actor: &str, state: Delivered) {
        let key = (org.to_owned(), actor.to_owned());
        if self.delivered.get(&key).is_none_or(|d| d.version <= state.version) {
            self.delivered.insert(key, state);
        }
    }

    /// `IamTokenResponse`, the shape of both logins and `/auth/tokens/refresh`.
    fn iam_body(&self, (access, refresh, family): Minted) -> Value {
        let f = self.family(family);
        json!({
            "access_token": access, "refresh_token": refresh, "token_type": "Bearer", "expires_in": 1800,
            "refresh_expires_at": rfc3339(f.until), "actor": actor_json(&f.actor), "session_id": family,
        })
    }

    /// `OAuthTokenResponse`; `org_id` is present only for an org-bound login.
    fn oauth_body(&self, (access, refresh, family): Minted) -> Value {
        let f = self.family(family);
        let mut body = json!({
            "access_token": access, "refresh_token": refresh, "token_type": "Bearer", "expires_in": 1800,
            "scope": SCOPE, "actor": actor_json(&f.actor),
        });
        if let Some(org) = &f.org {
            body["org_id"] = json!(org);
        }
        body
    }

    /// Runs `op` once per `Idempotency-Key`: the same key with the same request replays the stored
    /// answer under `Idempotency-Replayed: true`, a different request is a conflict. Only
    /// successes are stored, so a retry after an error is a new attempt.
    fn idempotent(
        &mut self,
        headers: &HeaderMap,
        digest: String,
        op: impl FnOnce(&mut Self) -> Result<(StatusCode, Value), Fail>,
    ) -> Response {
        let key = match idempotency_key(headers) {
            Ok(key) => key.to_owned(),
            Err(fail) => return fail.into_response(),
        };
        if let Some((seen, status, body)) = self.replays.get(&key) {
            return if *seen == digest { respond(*status, body, true) } else { CONFLICT.into_response() };
        }
        match op(self) {
            Ok((status, body)) => {
                let res = respond(status, &body, false);
                self.replays.insert(key, (digest, status, body));
                res
            }
            Err(fail) => fail.into_response(),
        }
    }
}

/// Version negotiation: the client names the majors it speaks, the service picks the highest in
/// common or refuses.
async fn version(headers: HeaderMap) -> Response {
    let Some(supported) = headers.get("silicon-iam-supported-api-versions").and_then(|v| v.to_str().ok()) else {
        return INVALID.into_response();
    };
    if !supported.split(',').any(|v| v.trim() == "v1") {
        return NOT_ACCEPTABLE.into_response();
    }
    let body = json!({
        "service": "silicon-iam", "selected_api_version": "v1", "supported_api_versions": ["v1"],
        "build": "stub", "commit": "stub",
    });
    ([("silicon-iam-api-version", "v1"), ("vary", "Silicon-IAM-Supported-API-Versions")], Json(body)).into_response()
}

/// IAM's login page, minus the login: a list of the seeded carbons, or with `?as=<carbon>` the
/// short-lived token appended to `redirect_uri` as `slt` (shown on a page when there is no
/// `redirect_uri`, as for a terminal). `org_id` binds the token to a membership the carbon must hold.
async fn login(State(stub): State<Shared>, uri: Uri, Query(q): Params) -> Response {
    let get = |k: &str| q.get(k).map(String::as_str);
    let app = escape(&stub.seed.app.app_id);
    if get("app_id") != Some(&stub.seed.app.app_id) {
        return UNKNOWN_APP.into_response();
    }
    let Some(carbon) = get("as") else {
        let query = escape(uri.query().unwrap_or_default());
        let link = |c: &Carbon| {
            format!("<li><a href=\"?{query}&amp;as={0}\">{1} (@{0})</a></li>", c.carbon_id, escape(&c.name))
        };
        let links: String = stub.seed.carbons.iter().map(link).collect();
        let page = format!("<!doctype html><title>Silicon IAM stub</title><h1>Sign in to {app}</h1><ul>{links}</ul>");
        return Html(page).into_response();
    };
    if stub.carbon(carbon).is_none() {
        return NOT_FOUND.into_response();
    }
    let org = get("org_id").map(str::to_owned);
    if let Some(org) = &org
        && stub.member(org, carbon).is_none()
    {
        return ORG_FORBIDDEN.into_response();
    }
    let slt = stub.lock().slt(carbon, org, Uuid::now_v7());
    match get("redirect_uri") {
        Some(redirect_uri) => {
            let separator = if redirect_uri.contains('?') { '&' } else { '?' };
            (StatusCode::FOUND, [(header::LOCATION, format!("{redirect_uri}{separator}slt={slt}"))]).into_response()
        }
        None => {
            let page = format!(
                "<!doctype html><title>Silicon IAM stub</title><p>Short-lived token for {app}:</p><code>{slt}</code>"
            );
            Html(page).into_response()
        }
    }
}

/// `POST /login/challenges {carbon_id | email}`: a ten-minute challenge whose code is `000000`.
async fn challenge(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    stub.lock().idempotent(&headers, digest("challenges", &body), |st| {
        let input: Value = json_body(&headers, &body)?;
        let id = input["carbon_id"].as_str().or_else(|| input["email"].as_str()?.split('@').next());
        let carbon = id.and_then(|id| stub.carbon(id)).ok_or(NOT_FOUND)?;
        let (session, expires_at) = (Uuid::now_v7(), now() + 600);
        st.challenges.insert(session, (carbon.carbon_id.clone(), expires_at));
        Ok((StatusCode::CREATED, json!({"session_id": session, "expires_at": rfc3339(expires_at)})))
    })
}

/// `POST /login/challenges/{session}/verify {code}`: the right code consumes the challenge and
/// opens a carbon session; a wrong one is `422` and the challenge survives.
async fn verify(State(stub): State<Shared>, headers: HeaderMap, Path(session): Path<String>, body: Bytes) -> Response {
    stub.lock().idempotent(&headers, digest(&format!("verify {session}"), &body), |st| {
        let input: Value = json_body(&headers, &body)?;
        let session: Uuid = session.parse().map_err(|_| CHALLENGE_GONE)?;
        let live = st.challenges.get(&session).filter(|(_, expires_at)| now() < *expires_at);
        let (carbon, _) = live.ok_or(CHALLENGE_GONE)?.clone();
        if input["code"] != "000000" {
            return Err(INVALID);
        }
        st.challenges.remove(&session);
        let minted = st.open(&carbon, None, None, CARBON, stub.seed.access_ttl_secs);
        Ok((StatusCode::OK, st.iam_body(minted)))
    })
}

/// `POST /auth/tokens/refresh {refresh_token}` for the IAM families; reuse is `401` here.
async fn refresh(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    stub.lock().idempotent(&headers, digest("refresh", &body), |st| {
        let input: Value = json_body(&headers, &body)?;
        let token = input["refresh_token"].as_str().filter(|t| t.starts_with("rft_")).ok_or(UNAUTHENTICATED)?;
        let minted = st.rotate(token, stub.seed.access_ttl_secs).map_err(|_| UNAUTHENTICATED)?;
        Ok((StatusCode::OK, st.iam_body(minted)))
    })
}

/// `POST /logout`: ends a carbon's IAM session. A silicon's own bearer is `403 forbidden` — the
/// route "accepts Carbon authority", so a silicon logout is local — and an Application token or
/// an unknown one is `401`.
async fn logout(State(stub): State<Shared>, headers: HeaderMap) -> Result<StatusCode, Fail> {
    let mut st = stub.lock();
    let (family, prefixes) = st.bearer(&headers).map(|(t, f)| (t.family, f.prefixes)).ok_or(UNAUTHENTICATED)?;
    match prefixes {
        CARBON => {
            st.revoke(family);
            Ok(StatusCode::NO_CONTENT)
        }
        SILICON => Err(FORBIDDEN),
        _ => Err(UNAUTHENTICATED),
    }
}

/// `POST /silicon-auth/token {silicon_id, silicon_token}` → a silicon session.
async fn silicon_auth(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    stub.lock().idempotent(&headers, digest("silicon-auth", &body), |st| {
        let input: Value = json_body(&headers, &body)?;
        let known = |s: &&Silicon| input["silicon_id"] == s.silicon_id && input["silicon_token"] == s.token;
        let silicon = stub.seed.silicons.iter().find(known).ok_or(UNAUTHENTICATED)?;
        let minted = st.open(&silicon.silicon_id, None, None, SILICON, stub.seed.access_ttl_secs);
        Ok((StatusCode::OK, st.iam_body(minted)))
    })
}

/// `POST /app-auth/short-lived-tokens {app_id, org_id?}` with an IAM bearer: how a silicon, or a
/// carbon who is already signed in, gets a short-lived token. A silicon's own org is the default.
async fn short_lived(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let mut st = stub.lock();
    let iam = st.bearer(&headers).filter(|(_, f)| f.prefixes != OAUTH);
    let Some((actor, session)) = iam.map(|(_, f)| (f.actor.clone(), f.session)) else {
        return UNAUTHENTICATED.into_response();
    };
    st.idempotent(&headers, digest("short-lived-tokens", &body), |st| {
        let input: Value = json_body(&headers, &body)?;
        if input["app_id"] != stub.seed.app.app_id {
            return Err(UNKNOWN_APP);
        }
        let own = || actor.rsplit_once(':').map(|(_, org)| org.to_owned());
        let org = input["org_id"].as_str().map_or_else(own, |o| Some(o.to_owned()));
        if let Some(org) = &org
            && stub.member(org, &actor).is_none()
        {
            return Err(ORG_FORBIDDEN);
        }
        Ok((StatusCode::CREATED, json!({"slt": st.slt(&actor, org, session), "expires_in": 120})))
    })
}

/// `POST /app-auth/tokens`: HTTP Basic, an `Idempotency-Key`, a form body naming the app and
/// exactly one of `slt` (spent once) or `refresh_token` (rotated; a replay kills the family).
async fn tokens(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let res = match stub.app_auth(&headers) {
        Err(fail) => fail.into_response(),
        Ok(()) => stub.lock().idempotent(&headers, digest("tokens", &body), |st| {
            let form = form_body(&headers, &body)?;
            if form.get("app_id") != Some(&stub.seed.app.app_id) {
                return Err(INVALID_CLIENT);
            }
            let ttl = stub.seed.access_ttl_secs;
            let minted = match (form.get("slt"), form.get("refresh_token")) {
                (Some(slt), None) => st.exchange(slt, ttl)?,
                (None, Some(token)) if token.starts_with("ort_") => {
                    st.rotate(token, ttl).map_err(|reused| if reused { REUSED_REFRESH } else { BAD_REFRESH })?
                }
                (None, Some(_)) => return Err(BAD_REFRESH),
                _ => return Err(ONE_GRANT),
            };
            Ok((StatusCode::OK, st.oauth_body(minted)))
        }),
    };
    (NO_STORE, res).into_response()
}

/// `POST /oauth/introspect` (Basic, form `token=`, optional `token_type_hint=` and `X-Org-ID`):
/// the real field set for a live Application token, plus the `authorization` snapshot when it is
/// an org-bound access token; exactly `{"active": false}` for anything else, including a
/// well-formed `X-Org-ID` that is not the token's org and a member a delivery has since removed.
/// A malformed or duplicated `X-Org-ID`, or an unknown hint, is `400 invalid_request`.
async fn introspect(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, Fail> {
    stub.app_auth(&headers)?;
    let form = form_body(&headers, &body)?;
    let context: Vec<&HeaderValue> = headers.get_all("x-org-id").iter().collect();
    let hint_known = form.get("token_type_hint").is_none_or(|h| h == "access_token" || h == "refresh_token");
    if context.len() > 1 || context.first().is_some_and(|c| !is_org_id(c.as_bytes())) || !hint_known {
        return Err(BAD_CONTEXT);
    }
    let st = stub.lock();
    let token = form.get("token").map_or("", String::as_str);
    let in_context =
        |f: &Family| context.first().is_none_or(|c| f.org.as_deref().is_some_and(|o| c.as_bytes() == o.as_bytes()));
    let Some((t, f)) = st.active(token).filter(|(_, f)| f.prefixes == OAUTH && in_context(f)) else {
        return Ok(inactive());
    };
    let delivered = f.org.as_ref().and_then(|org| st.delivered.get(&(org.clone(), f.actor.clone())));
    if delivered.is_some_and(|d| d.status != "active") {
        return Ok(inactive());
    }
    let version = delivered.map_or(1, |d| d.version);
    let app = &stub.seed.app.app_id;
    let mut body = json!({
        "active": true, "principal_id": principal_id(&f.actor), "actor_type": kind(&f.actor), "client_id": app,
        "session_id": f.session, "scope": SCOPE, "audience": app, "issued_at": t.issued_at,
        "expires_at": t.expires_at, "authorization_epoch": version,
    });
    if let Some(org) = &f.org {
        body["org_id"] = json!(org);
        body["membership_id"] = json!(membership_id(org, &f.actor));
        if let (true, Some((_, role, seeded))) = (st.access.contains_key(token), stub.member(org, &f.actor)) {
            let tags = delivered.map_or(seeded, |d| d.tags.as_slice());
            body["authorization"] = json!({
                "principal_id": principal_id(&f.actor), "actor_type": kind(&f.actor), "public_id": f.actor,
                "organization_id": org_uuid(org), "org_id": org, "membership_id": membership_id(org, &f.actor),
                "membership_version": version, "authorization_epoch": version, "audience": app,
                "testing_environment_id": stub.seed.app.testing_key.as_ref().map(|_| stable_id("testing-environment")),
                "scopes": SCOPE.split(' ').collect::<Vec<_>>(), "org_role": role,
                "tags": tags.iter().map(|t| tag_json(org, t)).collect::<Vec<_>>(),
            });
        }
    }
    Ok(Json(body))
}

/// `POST /oauth/revoke` (Basic, form `token=`, `Idempotency-Key`): an access token dies alone, a
/// refresh token takes its family; unknown tokens are fine too. Answers `200` with no body.
async fn revoke(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(fail) = stub.app_auth(&headers) {
        return fail.into_response();
    }
    stub.lock().idempotent(&headers, digest("revoke", &body), |st| {
        let form = form_body(&headers, &body)?;
        let token = form.get("token").map_or("", String::as_str);
        if st.access.remove(token).is_none()
            && let Some(family) = st.refresh.get(token).map(|t| t.family)
        {
            st.revoke(family);
        }
        Ok((StatusCode::OK, Value::Null))
    })
}

/// `GET /me`: `CarbonSelf`; silicons are forbidden, as in IAM.
async fn me(State(stub): State<Shared>, headers: HeaderMap) -> Result<Response, Fail> {
    let actor = stub.iam_actor(&headers)?;
    let c = stub.carbon(&actor).ok_or(FORBIDDEN)?;
    let body = json!({
        "principal_id": principal_id(&c.carbon_id), "carbon_id": c.carbon_id, "display_name": c.name,
        "description": null, "profile_photo": format!("https://iam.invalid/pfp/carbon?id={}", c.carbon_id),
        "timezone": "UTC", "email": format!("{}@example.invalid", c.carbon_id), "phone_number": "",
        "status": "active", "version": 1, "created_at": EPOCH, "updated_at": EPOCH,
    });
    Ok(([(header::ETAG, "\"1\"")], Json(body)).into_response())
}

/// `GET /organizations`: the carbon's orgs; silicons are forbidden, as in IAM.
async fn organizations(State(stub): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, Fail> {
    let actor = stub.iam_actor(&headers)?;
    let c = stub.carbon(&actor).ok_or(FORBIDDEN)?;
    let mine = stub.seed.orgs.iter().filter(|o| c.memberships.contains_key(&o.org_id));
    Ok(page(mine.map(|o| stub.org_json(o)).collect()))
}

async fn directory_self(
    State(stub): State<Shared>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Params,
) -> Result<Json<Value>, Fail> {
    let actor = stub.iam_actor(&headers)?;
    let fields = fields(&q)?;
    let org = stub.org(&org).ok_or(NOT_FOUND)?;
    stub.directory_row(org, &actor, fields).map(Json).ok_or(NOT_FOUND)
}

async fn directory_members(
    State(stub): State<Shared>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Params,
) -> Result<Json<Value>, Fail> {
    let actor = stub.iam_actor(&headers)?;
    let fields = fields(&q)?;
    let org = stub.org(&org).ok_or(NOT_FOUND)?;
    stub.member(&org.org_id, &actor).ok_or(NOT_FOUND)?;
    Ok(page(stub.actors().filter_map(|a| stub.directory_row(org, a, fields)).collect()))
}

/// What `POST /_stub/deliver` takes: where to post, which event, and the member rows it carries.
#[derive(Deserialize)]
struct Delivery {
    url: String,
    event_type: String,
    org: String,
    #[serde(default)]
    members: Vec<Row>,
    #[serde(default)]
    envelope: Envelope,
    /// Fixed by the caller when a test needs to deliver the same event twice.
    #[serde(default = "Uuid::now_v7")]
    event_id: Uuid,
    /// The aggregate (and every row's membership) version; the receiver orders on it.
    #[serde(default = "one")]
    version: u64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Envelope {
    #[default]
    Production,
    Test,
}

#[derive(Deserialize)]
struct Row {
    actor: String,
    #[serde(default = "active")]
    status: String,
    #[serde(default)]
    tags: Vec<String>,
    display_name: Option<String>,
}

fn one() -> u64 {
    1
}

fn active() -> String {
    "active".into()
}

/// Signs and posts the webhook IAM would send for `event_type` about `members` of `org`, in the
/// production or the `test` envelope: full member rows, or tombstones for a `*.removed.v1` event.
/// Remembers what it said so introspection agrees, and answers the receiver's status so a test can
/// assert on it.
async fn deliver(State(stub): State<Shared>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, Fail> {
    let d: Delivery = json_body(&headers, &body)?;
    let removal = d.event_type.ends_with(".removed.v1");
    let ts = now();
    let at = rfc3339(ts);
    let row = |m: &Row| {
        if removal { tombstone(&d.org, &m.actor, d.version) } else { stub.member_row(&d.org, m, d.version, &at) }
    };
    let rows: Vec<Value> = d.members.iter().map(row).collect();
    {
        let mut st = stub.lock();
        for m in &d.members {
            let status = if removal { "removed".to_owned() } else { m.status.clone() };
            st.record(&d.org, &m.actor, Delivered { status, tags: m.tags.clone(), version: d.version });
        }
    }
    let aggregate = d.members.first().map_or(org_uuid(&d.org), |m| membership_id(&d.org, &m.actor));
    let metadata = json!({
        "spec_version": "1.0", "event_id": d.event_id, "event_type": d.event_type, "occurred_at": at,
        "organization_id": org_uuid(&d.org),
        "aggregate": {"type": "organization_membership", "id": aggregate, "version": d.version},
    });
    let data = json!({"changed_fields": [], "current": {"members": rows}});
    let event = match d.envelope {
        Envelope::Production => {
            let mut event = metadata;
            event["data"] = data;
            event
        }
        Envelope::Test => {
            let key = stub.seed.app.testing_key.as_deref().ok_or(INVALID)?;
            json!({"test": {"testing_key": key, "metadata": metadata, "data": data}})
        }
    };
    let bytes = serde_json::to_vec(&event).expect("a json value serialises");
    let signed = [format!("{ts}.").as_bytes(), bytes.as_slice()].concat();
    let signature = format!("v1={}", hmac_sha256_hex(stub.seed.app.webhook_secret.as_bytes(), &signed));
    let sent = stub
        .http
        .post(&d.url)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-silicon-iam-event-id", d.event_id.to_string())
        .header("x-silicon-iam-timestamp", ts.to_string())
        .header("x-silicon-iam-key-version", "1")
        .header("x-silicon-iam-signature", signature)
        .body(bytes)
        .send()
        .await;
    match sent {
        Ok(res) => Ok(Json(json!({"status": res.status().as_u16(), "event_id": d.event_id}))),
        Err(e) => {
            tracing::warn!("stub delivery to {} failed: {e}", d.url);
            Err(DELIVERY_FAILED)
        }
    }
}

#[cfg(test)]
mod tests {
    use hmac::{Hmac, KeyInit as _, Mac};

    use super::*;

    const APP: &str = "tos>spacestation";
    const APP_Q: &str = "tos%3Espacestation";
    const SECRET: &str = "ask_stubstubstubstubstubstubstubstubstubstubabc";
    const WEBHOOK_SECRET: &str = "whs_stubstubstubstubstubstubstubstubstubstubabc";
    const STK: &str = "stk-0123456789abcdef0123456789abcdef";
    const GARBAGE: &str = "oat_nopenopenopenopenopenopenopenopenopenopenopeno";

    async fn start(seed: Seed) -> String {
        let (addr, _) = serve(seed, "127.0.0.1:0".parse().unwrap()).await.unwrap();
        format!("http://{addr}")
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
    }

    fn key() -> String {
        Uuid::now_v7().to_string()
    }

    /// `"<status> <error code>"` of a failed response.
    async fn err(res: reqwest::Response) -> String {
        let status = res.status().as_u16();
        let body: Value = res.json().await.unwrap();
        format!("{status} {}", body["error"]["code"].as_str().unwrap())
    }

    async fn get(base: &str, path: &str, bearer: &str) -> reqwest::Response {
        http().get(format!("{base}{path}")).bearer_auth(bearer).send().await.unwrap()
    }

    async fn get_json(base: &str, path: &str, bearer: &str) -> Value {
        let res = get(base, path, bearer).await;
        assert_eq!(res.status(), 200, "{path}");
        res.json().await.unwrap()
    }

    async fn post_json(
        base: &str,
        path: &str,
        key: Option<&str>,
        bearer: Option<&str>,
        body: &Value,
    ) -> reqwest::Response {
        let req = http().post(format!("{base}{path}")).json(body);
        let req = key.into_iter().fold(req, |r, k| r.header("idempotency-key", k));
        bearer.into_iter().fold(req, reqwest::RequestBuilder::bearer_auth).send().await.unwrap()
    }

    /// A Basic-authenticated form post, as the Application makes them.
    async fn post_form(base: &str, path: &str, key: Option<&str>, form: &[(&str, &str)]) -> reqwest::Response {
        let req = http().post(format!("{base}{path}")).basic_auth(APP, Some(SECRET)).form(form);
        key.into_iter().fold(req, |r, k| r.header("idempotency-key", k)).send().await.unwrap()
    }

    async fn exchange(base: &str, form: &[(&str, &str)]) -> reqwest::Response {
        post_form(base, "/api/v1/app-auth/tokens", Some(&key()), form).await
    }

    async fn introspect(base: &str, token: &str) -> Value {
        let res = post_form(base, "/api/v1/oauth/introspect", None, &[("token", token)]).await;
        assert_eq!(res.status(), 200);
        res.json().await.unwrap()
    }

    async fn revoke(base: &str, token: &str) -> reqwest::Response {
        post_form(base, "/api/v1/oauth/revoke", Some(&key()), &[("token", token)]).await
    }

    /// The Application renewing its session.
    async fn refresh_app(base: &str, token: &str) -> reqwest::Response {
        exchange(base, &[("app_id", APP), ("refresh_token", token)]).await
    }

    /// A carbon or silicon renewing its own IAM session.
    async fn refresh_iam(base: &str, token: &str) -> reqwest::Response {
        let body = json!({"refresh_token": token});
        post_json(base, "/api/v1/auth/tokens/refresh", Some(&key()), None, &body).await
    }

    /// Carbon login: a challenge by carbon id, then the fixed code.
    async fn carbon_login(base: &str, carbon: &str) -> Value {
        let body = json!({"carbon_id": carbon});
        let res = post_json(base, "/api/v1/login/challenges", Some(&key()), None, &body).await;
        assert_eq!(res.status(), 201);
        let session = res.json::<Value>().await.unwrap()["session_id"].as_str().unwrap().to_owned();
        let path = format!("/api/v1/login/challenges/{session}/verify");
        let res = post_json(base, &path, Some(&key()), None, &json!({"code": "000000"})).await;
        assert_eq!(res.status(), 200);
        res.json().await.unwrap()
    }

    async fn silicon_login(base: &str, token: &str) -> reqwest::Response {
        let body = json!({"silicon_id": "bot:tos", "silicon_token": token});
        post_json(base, "/api/v1/silicon-auth/token", Some(&key()), None, &body).await
    }

    /// A short-lived token minted by a signed-in actor, bound to `org` when given.
    async fn slt(base: &str, bearer: &str, org: Option<&str>) -> String {
        let mut body = json!({"app_id": APP});
        if let Some(org) = org {
            body["org_id"] = json!(org);
        }
        let res = post_json(base, "/api/v1/app-auth/short-lived-tokens", Some(&key()), Some(bearer), &body).await;
        assert_eq!(res.status(), 201);
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["expires_in"], 120);
        body["slt"].as_str().unwrap().to_owned()
    }

    fn token<'a>(body: &'a Value, field: &str, prefix: &str) -> &'a str {
        let token = body[field].as_str().unwrap();
        assert!(token.starts_with(prefix) && token.len() == 47, "{field}: {token}");
        token
    }

    fn sorted_keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    /// A receiver that keeps every delivery and answers 204.
    async fn receiver() -> (String, Arc<Mutex<Vec<(HeaderMap, Bytes)>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let hook = move |headers: HeaderMap, body: Bytes| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push((headers, body));
                StatusCode::NO_CONTENT
            }
        };
        let (addr, _) = listen(Router::new().route("/hook", post(hook)), "127.0.0.1:0".parse().unwrap()).await.unwrap();
        (format!("http://{addr}/hook"), got)
    }

    /// Recomputes the signature the way a receiver must: HMAC over `{timestamp}.{raw body}`.
    fn assert_signed(headers: &HeaderMap, body: &[u8]) {
        let ts = headers["x-silicon-iam-timestamp"].to_str().unwrap();
        assert!((now() - ts.parse::<i64>().unwrap()).abs() < 5, "the timestamp is unix seconds, now");
        let mut mac = Hmac::<Sha256>::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
        mac.update(format!("{ts}.").as_bytes());
        mac.update(body);
        let hex: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(headers["x-silicon-iam-signature"].to_str().unwrap(), format!("v1={hex}"));
        assert_eq!(headers["x-silicon-iam-key-version"], "1");
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    }

    #[tokio::test]
    async fn version_negotiation_answers_like_the_service() {
        let base = start(default_seed()).await;
        let url = format!("{base}/api/version");
        let res = http().get(&url).header("silicon-iam-supported-api-versions", "v2, v1").send().await.unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["silicon-iam-api-version"], "v1");
        assert_eq!(res.headers()["vary"], "Silicon-IAM-Supported-API-Versions");
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["service"], "silicon-iam");
        assert_eq!(body["selected_api_version"], "v1");
        assert_eq!(body["supported_api_versions"], json!(["v1"]));
        assert_eq!(err(http().get(&url).send().await.unwrap()).await, "422 validation_failed");
        let v9 = http().get(&url).header("silicon-iam-supported-api-versions", "v9").send().await.unwrap();
        assert_eq!(err(v9).await, "406 api_version_not_acceptable");
        assert_eq!(err(http().get(format!("{base}/api/v1/nope")).send().await.unwrap()).await, "404 not_found");
    }

    #[tokio::test]
    async fn login_page_redirects_with_a_single_use_slt_bound_to_the_org() {
        let base = start(default_seed()).await;
        let login = format!(
            "{base}/api/v1/login?app_id={APP_Q}&redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcb%3Fx%3D1&org_id=tos"
        );
        let page = http().get(&login).send().await.unwrap();
        assert_eq!(page.status(), 200);
        let html = page.text().await.unwrap();
        assert!(html.contains("as=alice") && html.contains("as=bob"), "{html}");

        let res = http().get(format!("{login}&as=alice")).send().await.unwrap();
        assert_eq!(res.status(), 302);
        let location = res.headers()[header::LOCATION].to_str().unwrap().to_owned();
        let (redirect_uri, slt) = location.split_once("&slt=").unwrap();
        assert_eq!(redirect_uri, "http://127.0.0.1:1/cb?x=1", "the redirect keeps its own query");
        assert!(slt.starts_with("oac_") && slt.len() == 47, "{slt}");

        let res = exchange(&base, &[("app_id", APP), ("slt", slt)]).await;
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(res.headers()[header::PRAGMA], "no-cache");
        let tokens: Value = res.json().await.unwrap();
        token(&tokens, "access_token", "oat_");
        token(&tokens, "refresh_token", "ort_");
        assert_eq!(tokens["token_type"], "Bearer");
        assert_eq!(tokens["expires_in"], 1800);
        assert_eq!(tokens["scope"], SCOPE);
        assert_eq!(
            tokens["actor"],
            json!({"principal_id": principal_id("alice"), "type": "carbon", "public_id": "alice"})
        );
        assert_eq!(tokens["org_id"], "tos", "the login was bound to tos");
        let again = exchange(&base, &[("app_id", APP), ("slt", slt)]).await;
        assert_eq!(err(again).await, "400 invalid_grant", "an slt is good for exactly one exchange");

        let unknown_app = format!("{base}/api/v1/login?app_id=tos%3Enope&redirect_uri=http%3A%2F%2Fx%2Fcb&as=alice");
        assert_eq!(err(http().get(unknown_app).send().await.unwrap()).await, "400 invalid_request");
        assert_eq!(err(http().get(format!("{login}&as=mallory")).send().await.unwrap()).await, "404 not_found");
        let outsider =
            format!("{base}/api/v1/login?app_id={APP_Q}&redirect_uri=http%3A%2F%2Fx%2Fcb&org_id=acme&as=bob");
        assert_eq!(err(http().get(outsider).send().await.unwrap()).await, "403 organization_context_forbidden");

        // With nowhere to redirect, the token is shown on a page, as for a terminal.
        let shown = http().get(format!("{base}/api/v1/login?app_id={APP_Q}&as=bob")).send().await.unwrap();
        assert_eq!(shown.status(), 200);
        assert!(shown.text().await.unwrap().contains("<code>oac_"));
    }

    #[tokio::test]
    async fn exchange_wants_a_form_body_basic_auth_and_an_idempotency_key() {
        let base = start(default_seed()).await;
        let alice = carbon_login(&base, "alice").await;
        let cat = alice["access_token"].as_str().unwrap();
        let slt = slt(&base, cat, Some("tos")).await;
        let url = format!("{base}/api/v1/app-auth/tokens");
        let form = [("app_id", APP), ("slt", slt.as_str())];

        let json = http().post(&url).basic_auth(APP, Some(SECRET)).header("idempotency-key", key());
        assert_eq!(
            err(json.json(&json!({"app_id": APP, "slt": slt})).send().await.unwrap()).await,
            "415 request_rejected"
        );
        assert_eq!(
            err(post_form(&base, "/api/v1/app-auth/tokens", None, &form).await).await,
            "428 precondition_required"
        );
        assert_eq!(
            err(post_form(&base, "/api/v1/app-auth/tokens", Some("short"), &form).await).await,
            "422 validation_failed"
        );
        let bad_basic = http().post(&url).basic_auth(APP, Some("ask_wrong")).header("idempotency-key", key());
        let bad_basic = bad_basic.form(&form).send().await.unwrap();
        assert_eq!(bad_basic.headers()[header::WWW_AUTHENTICATE], "Basic realm=\"silicon-iam\"");
        assert_eq!(err(bad_basic).await, "401 invalid_client");
        let mismatch = exchange(&base, &[("app_id", "tos>nope"), ("slt", &slt)]).await;
        assert_eq!(err(mismatch).await, "401 invalid_client", "the form's app_id must be the Basic user");
        let both = exchange(&base, &[("app_id", APP), ("slt", &slt), ("refresh_token", "ort_x")]).await;
        assert_eq!(err(both).await, "400 invalid_request");
        assert_eq!(err(exchange(&base, &[("app_id", APP)]).await).await, "400 invalid_request");
        assert_eq!(err(exchange(&base, &[("app_id", APP), ("slt", GARBAGE)]).await).await, "400 invalid_grant");

        let key = key();
        let first = post_form(&base, "/api/v1/app-auth/tokens", Some(&key), &form).await;
        assert_eq!(first.status(), 200);
        assert_eq!(first.headers().get("idempotency-replayed"), None);
        let first: Value = first.json().await.unwrap();
        let replay = post_form(&base, "/api/v1/app-auth/tokens", Some(&key), &form).await;
        assert_eq!(replay.headers()["idempotency-replayed"], "true");
        assert_eq!(replay.json::<Value>().await.unwrap(), first, "a retry with the same key gets the same tokens");
        let other = post_form(&base, "/api/v1/app-auth/tokens", Some(&key), &[("app_id", APP), ("slt", GARBAGE)]).await;
        assert_eq!(err(other).await, "409 idempotency_conflict");
        assert_eq!(err(exchange(&base, &form).await).await, "400 invalid_grant", "the slt was still spent once");
    }

    #[tokio::test]
    async fn refresh_rotates_and_a_reused_refresh_token_revokes_the_family() {
        let base = start(default_seed()).await;
        let bot: Value = silicon_login(&base, STK).await.json().await.unwrap();
        let sat = token(&bot, "access_token", "sat_");
        let slt = slt(&base, sat, None).await;
        let first: Value = exchange(&base, &[("app_id", APP), ("slt", &slt)]).await.json().await.unwrap();
        assert_eq!(first["org_id"], "tos", "a silicon's login is bound to its own org by default");
        assert_eq!(first["actor"]["public_id"], "bot:tos");
        let (oat1, ort1) = (token(&first, "access_token", "oat_"), token(&first, "refresh_token", "ort_"));
        let intro = introspect(&base, oat1).await;
        assert_eq!(intro["active"], true);
        assert_eq!(intro["actor_type"], "silicon");
        assert_eq!(intro["org_id"], "tos");
        assert_eq!(intro["membership_id"], json!(membership_id("tos", "bot:tos")));
        assert_eq!(intro["session_id"], bot["session_id"], "an Application session descends from the IAM session");

        let second = refresh_app(&base, ort1).await;
        assert_eq!(second.status(), 200);
        let second: Value = second.json().await.unwrap();
        let (oat2, ort2) = (token(&second, "access_token", "oat_"), token(&second, "refresh_token", "ort_"));
        assert!(oat2 != oat1 && ort2 != ort1);
        assert_eq!(introspect(&base, oat1).await["active"], true, "an access token outlives the rotation");
        assert_eq!(introspect(&base, oat2).await["active"], true);

        let reused = refresh_app(&base, ort1).await;
        assert_eq!(reused.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(err(reused).await, "400 invalid_grant");
        assert_eq!(introspect(&base, oat2).await, json!({"active": false}), "the whole family died");
        assert_eq!(introspect(&base, oat1).await, json!({"active": false}));
        assert_eq!(err(refresh_app(&base, ort2).await).await, "400 invalid_grant");
        assert_eq!(err(refresh_app(&base, "rft_notanapptoken").await).await, "400 invalid_grant");
    }

    #[tokio::test]
    async fn introspection_has_the_real_field_set_and_revocation_takes_effect() {
        let base = start(default_seed()).await;
        let alice = carbon_login(&base, "alice").await;
        let mut keys = sorted_keys(&alice);
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["access_token", "actor", "expires_in", "refresh_expires_at", "refresh_token", "session_id", "token_type"]
        );
        let cat = token(&alice, "access_token", "cat_");
        token(&alice, "refresh_token", "rft_");
        assert!(DateTime::parse_from_rfc3339(alice["refresh_expires_at"].as_str().unwrap()).is_ok());

        let slt_acme = slt(&base, cat, Some("acme")).await;
        let tokens: Value = exchange(&base, &[("app_id", APP), ("slt", &slt_acme)]).await.json().await.unwrap();
        assert_eq!(tokens["org_id"], "acme", "one Application, any org the carbon belongs to");
        let (oat, ort) = (token(&tokens, "access_token", "oat_"), token(&tokens, "refresh_token", "ort_"));
        let intro = introspect(&base, oat).await;
        let mut keys = sorted_keys(&intro);
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "active",
                "actor_type",
                "audience",
                "authorization",
                "authorization_epoch",
                "client_id",
                "expires_at",
                "issued_at",
                "membership_id",
                "org_id",
                "principal_id",
                "scope",
                "session_id",
            ]
        );
        assert_eq!(intro["client_id"], APP);
        assert_eq!(intro["audience"], APP);
        assert_eq!(intro["actor_type"], "carbon");
        assert_eq!(intro["org_id"], "acme");
        assert_eq!(intro["membership_id"], json!(membership_id("acme", "alice")));
        assert_eq!(intro["principal_id"], json!(principal_id("alice")));
        assert_eq!(intro["session_id"], alice["session_id"]);
        assert_eq!(intro["expires_at"].as_i64().unwrap() - intro["issued_at"].as_i64().unwrap(), 1800);
        let snapshot = &intro["authorization"];
        let mut keys = sorted_keys(snapshot);
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "actor_type",
                "audience",
                "authorization_epoch",
                "membership_id",
                "membership_version",
                "org_id",
                "org_role",
                "organization_id",
                "principal_id",
                "public_id",
                "scopes",
                "tags",
                "testing_environment_id",
            ]
        );
        assert_eq!(snapshot["public_id"], "alice");
        assert_eq!(snapshot["actor_type"], "carbon");
        assert_eq!(snapshot["org_id"], "acme");
        assert_eq!(snapshot["organization_id"], json!(org_uuid("acme")));
        assert_eq!(snapshot["membership_id"], intro["membership_id"]);
        assert_eq!(snapshot["principal_id"], intro["principal_id"]);
        assert_eq!(
            (snapshot["membership_version"].clone(), snapshot["authorization_epoch"].clone()),
            (json!(1), json!(1))
        );
        assert_eq!(snapshot["audience"], APP);
        assert_eq!(snapshot["org_role"], "owner");
        assert_eq!(snapshot["tags"], json!([tag_json("acme", "sales")]));
        assert_eq!(snapshot["scopes"], json!(SCOPE.split(' ').collect::<Vec<_>>()));
        assert!(snapshot["testing_environment_id"].is_string(), "the seed has a testing key");
        let refresh = introspect(&base, ort).await;
        assert_eq!(refresh["active"], true, "a refresh token introspects too");
        assert_eq!(refresh.get("authorization"), None, "but carries no snapshot");

        assert_eq!(introspect(&base, GARBAGE).await, json!({"active": false}));
        assert_eq!(introspect(&base, cat).await, json!({"active": false}), "an IAM token is not an Application token");
        let with_org = |orgs: &'static [&'static str]| {
            let req = http().post(format!("{base}/api/v1/oauth/introspect")).basic_auth(APP, Some(SECRET));
            orgs.iter().fold(req, |r, o| r.header("x-org-id", *o)).form(&[("token", oat)]).send()
        };
        let mismatch = with_org(&["tos"]).await.unwrap().json::<Value>().await.unwrap();
        assert_eq!(mismatch, json!({"active": false}), "a well-formed X-Org-ID that is not the token's org");
        assert_eq!(with_org(&["acme"]).await.unwrap().json::<Value>().await.unwrap()["active"], true);
        assert_eq!(err(with_org(&["acme", "acme"]).await.unwrap()).await, "400 invalid_request", "duplicated");
        assert_eq!(err(with_org(&["Acme"]).await.unwrap()).await, "400 invalid_request", "malformed");
        let introspect_path = "/api/v1/oauth/introspect";
        let unsupported =
            post_form(&base, introspect_path, None, &[("token", oat), ("token_type_hint", "id_token")]).await;
        assert_eq!(err(unsupported).await, "400 invalid_request", "an unsupported hint");
        let hinted =
            post_form(&base, introspect_path, None, &[("token", oat), ("token_type_hint", "access_token")]).await;
        assert_eq!(hinted.json::<Value>().await.unwrap()["active"], true);

        let unscoped = slt(&base, cat, None).await;
        let tokens: Value = exchange(&base, &[("app_id", APP), ("slt", &unscoped)]).await.json().await.unwrap();
        assert_eq!(tokens.get("org_id"), None, "an unscoped login has no org");
        let intro = introspect(&base, tokens["access_token"].as_str().unwrap()).await;
        assert_eq!(
            (intro["active"].clone(), intro.get("org_id"), intro.get("membership_id"), intro.get("authorization")),
            (json!(true), None, None, None),
            "an unscoped token has no org and no snapshot"
        );

        let revoked = revoke(&base, oat).await;
        assert_eq!(revoked.status(), 200);
        assert_eq!(revoked.content_length(), Some(0));
        assert_eq!(introspect(&base, oat).await, json!({"active": false}));
        let renewed = exchange(&base, &[("app_id", APP), ("refresh_token", ort)]).await;
        assert_eq!(renewed.status(), 200, "revoking an access token leaves its family alive");
        let renewed: Value = renewed.json().await.unwrap();
        let (oat3, ort3) = (token(&renewed, "access_token", "oat_"), token(&renewed, "refresh_token", "ort_"));
        assert_eq!(revoke(&base, ort3).await.status(), 200);
        assert_eq!(
            introspect(&base, oat3).await,
            json!({"active": false}),
            "revoking a refresh token takes the family"
        );
        assert_eq!(err(exchange(&base, &[("app_id", APP), ("refresh_token", ort3)]).await).await, "400 invalid_grant");
        assert_eq!(revoke(&base, GARBAGE).await.status(), 200, "unknown tokens deliberately succeed");
        let no_key = post_form(&base, "/api/v1/oauth/revoke", None, &[("token", GARBAGE)]).await;
        assert_eq!(err(no_key).await, "428 precondition_required");
        let json = http().post(format!("{base}/api/v1/oauth/introspect")).basic_auth(APP, Some(SECRET));
        assert_eq!(err(json.json(&json!({"token": GARBAGE})).send().await.unwrap()).await, "415 request_rejected");
        let no_basic = http().post(format!("{base}/api/v1/oauth/introspect")).form(&[("token", GARBAGE)]);
        assert_eq!(err(no_basic.send().await.unwrap()).await, "401 invalid_client");
    }

    #[tokio::test]
    async fn directory_routes_forbid_application_tokens_and_serve_iam_bearers() {
        let base = start(default_seed()).await;
        let alice = carbon_login(&base, "alice").await;
        let cat = alice["access_token"].as_str().unwrap();
        let bob = carbon_login(&base, "bob").await;
        let bot: Value = silicon_login(&base, STK).await.json().await.unwrap();
        let sat = bot["access_token"].as_str().unwrap();
        let slt = slt(&base, cat, Some("tos")).await;
        let tokens: Value = exchange(&base, &[("app_id", APP), ("slt", &slt)]).await.json().await.unwrap();
        let oat = tokens["access_token"].as_str().unwrap();

        let directory = "/api/v1/organizations/tos/directory";
        for path in
            ["/api/v1/me", "/api/v1/organizations", &format!("{directory}/self"), &format!("{directory}/members")]
        {
            assert_eq!(err(get(&base, path, oat).await).await, "403 forbidden", "{path}: an Application reads nothing");
            let none = http().get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(none.headers()[header::WWW_AUTHENTICATE], "Bearer");
            assert_eq!(err(none).await, "401 unauthenticated", "{path}");
            assert_eq!(err(get(&base, path, "cat_nope").await).await, "401 unauthenticated", "{path}");
        }

        let me = get(&base, "/api/v1/me", cat).await;
        assert_eq!(me.headers()[header::ETAG], "\"1\"");
        let me: Value = me.json().await.unwrap();
        assert_eq!((me["carbon_id"].clone(), me["display_name"].clone()), (json!("alice"), json!("Alice")));
        let orgs = get_json(&base, "/api/v1/organizations?limit=100", cat).await;
        let ids: Vec<_> = orgs["items"].as_array().unwrap().iter().map(|o| o["org_id"].clone()).collect();
        assert_eq!(ids, ["tos", "acme"]);
        assert_eq!(orgs["items"][0]["owner_membership_id"], json!(membership_id("tos", "alice")));
        assert_eq!(orgs["page"], json!({"next_cursor": null, "has_more": false}));
        let sparse = get_json(&base, &format!("{directory}/self?fields=id,tags"), cat).await;
        assert_eq!(sparse, json!({"id": "alice", "tags": [tag_json("tos", "tech")]}));
        let full = get_json(&base, &format!("{directory}/self"), cat).await;
        assert_eq!(full["role"], json!({"org_role": "owner", "job_role": ""}));
        assert_eq!(full["org"], json!({"id": "tos", "name": "Team of Silicons"}));
        assert_eq!((full["name"].clone(), full["trust"].clone()), (json!("Alice"), Value::Null));
        let members = get_json(&base, &format!("{directory}/members?fields=id"), cat).await;
        assert_eq!(members["items"], json!([{"id": "alice"}, {"id": "bob"}, {"id": "bot:tos"}]));
        assert_eq!(
            err(get(&base, &format!("{directory}/self?fields=bogus"), cat).await).await,
            "422 validation_failed"
        );
        let bob = bob["access_token"].as_str().unwrap();
        assert_eq!(err(get(&base, "/api/v1/organizations/acme/directory/self", bob).await).await, "404 not_found");
        assert_eq!(err(get(&base, "/api/v1/organizations/nope/directory/self", cat).await).await, "404 not_found");

        assert_eq!(err(get(&base, "/api/v1/me", sat).await).await, "403 forbidden", "silicons have no /me");
        assert_eq!(err(get(&base, "/api/v1/organizations", sat).await).await, "403 forbidden");
        let own = get_json(&base, &format!("{directory}/self"), sat).await;
        assert_eq!(own["id"], "bot:tos");
        assert_eq!(own["tags"], json!([tag_json("tos", "ops")]));
        assert_eq!(own["role"]["org_role"], "member");
        assert_eq!(err(get(&base, "/api/v1/organizations/acme/directory/self", sat).await).await, "404 not_found");
    }

    #[tokio::test]
    async fn short_lived_tokens_need_an_iam_bearer_a_known_app_and_a_membership() {
        let base = start(default_seed()).await;
        let alice = carbon_login(&base, "alice").await;
        let cat = alice["access_token"].as_str().unwrap();
        let bot: Value = silicon_login(&base, STK).await.json().await.unwrap();
        let sat = bot["access_token"].as_str().unwrap();
        let path = "/api/v1/app-auth/short-lived-tokens";
        let body = json!({"app_id": APP, "org_id": "tos"});
        let res = post_json(&base, path, Some(&key()), None, &body).await;
        assert_eq!(err(res).await, "401 unauthenticated");
        let res = post_json(&base, path, Some(&key()), Some("cat_nope"), &body).await;
        assert_eq!(err(res).await, "401 unauthenticated");
        let res = post_json(&base, path, None, Some(cat), &body).await;
        assert_eq!(err(res).await, "428 precondition_required");
        let res =
            post_json(&base, path, Some(&key()), Some(cat), &json!({"app_id": "tos>nope", "org_id": "tos"})).await;
        assert_eq!(err(res).await, "400 invalid_request", "an unknown application");
        let res = post_json(&base, path, Some(&key()), Some(cat), &json!({"app_id": APP, "org_id": "nope"})).await;
        assert_eq!(err(res).await, "403 organization_context_forbidden");
        let res = post_json(&base, path, Some(&key()), Some(sat), &json!({"app_id": APP, "org_id": "acme"})).await;
        assert_eq!(err(res).await, "403 organization_context_forbidden", "bot:tos is not in acme");
        let form = http().post(format!("{base}{path}")).bearer_auth(cat).header("idempotency-key", key());
        assert_eq!(err(form.form(&[("app_id", APP)]).send().await.unwrap()).await, "415 request_rejected");

        let slt = slt(&base, cat, Some("tos")).await;
        let tokens: Value = exchange(&base, &[("app_id", APP), ("slt", &slt)]).await.json().await.unwrap();
        let oat = tokens["access_token"].as_str().unwrap();
        let res = post_json(&base, path, Some(&key()), Some(oat), &body).await;
        assert_eq!(err(res).await, "401 unauthenticated", "an Application token cannot mint a login");

        let key = key();
        let first: Value = post_json(&base, path, Some(&key), Some(cat), &body).await.json().await.unwrap();
        let replay = post_json(&base, path, Some(&key), Some(cat), &body).await;
        assert_eq!(replay.headers()["idempotency-replayed"], "true");
        assert_eq!(replay.json::<Value>().await.unwrap(), first);
        let other = post_json(&base, path, Some(&key), Some(cat), &json!({"app_id": APP, "org_id": "acme"})).await;
        assert_eq!(err(other).await, "409 idempotency_conflict");
    }

    #[tokio::test]
    async fn carbon_and_silicon_logins_answer_with_the_real_codes() {
        let base = start(default_seed()).await;
        let challenges = "/api/v1/login/challenges";
        let res = post_json(&base, challenges, Some(&key()), None, &json!({"carbon_id": "mallory"})).await;
        assert_eq!(err(res).await, "404 not_found");
        let res = post_json(&base, challenges, None, None, &json!({"carbon_id": "alice"})).await;
        assert_eq!(err(res).await, "428 precondition_required");
        let res = post_json(&base, challenges, Some(&key()), None, &json!({"email": "alice@spacestation.test"})).await;
        assert_eq!(res.status(), 201, "an email signs in the carbon it is addressed to");
        let session = res.json::<Value>().await.unwrap()["session_id"].as_str().unwrap().to_owned();
        let verify = format!("{challenges}/{session}/verify");
        let wrong = post_json(&base, &verify, Some(&key()), None, &json!({"code": "111111"})).await;
        assert_eq!(err(wrong).await, "422 validation_failed");
        let form = http().post(format!("{base}{verify}")).header("idempotency-key", key()).form(&[("code", "000000")]);
        assert_eq!(err(form.send().await.unwrap()).await, "415 request_rejected");
        let right = post_json(&base, &verify, Some(&key()), None, &json!({"code": "000000"})).await;
        assert_eq!(right.status(), 200, "a wrong code does not spend the challenge");
        assert_eq!(right.json::<Value>().await.unwrap()["actor"]["public_id"], "alice");
        let spent = post_json(&base, &verify, Some(&key()), None, &json!({"code": "000000"})).await;
        assert_eq!(err(spent).await, "410 challenge_expired");
        let nope = format!("{challenges}/{}/verify", Uuid::now_v7());
        assert_eq!(
            err(post_json(&base, &nope, Some(&key()), None, &json!({"code": "000000"})).await).await,
            "410 challenge_expired"
        );

        let wrong_stk = silicon_login(&base, "stk-ffffffffffffffffffffffffffffffff").await;
        assert_eq!(wrong_stk.headers()[header::WWW_AUTHENTICATE], "Bearer");
        assert_eq!(err(wrong_stk).await, "401 unauthenticated");
        let bot: Value = silicon_login(&base, STK).await.json().await.unwrap();
        assert_eq!(
            bot["actor"],
            json!({"principal_id": principal_id("bot:tos"), "type": "silicon", "public_id": "bot:tos"})
        );
        let rft = token(&bot, "refresh_token", "rft_");
        let rotated: Value = refresh_iam(&base, rft).await.json().await.unwrap();
        assert_eq!(rotated["session_id"], bot["session_id"]);
        let sat2 = token(&rotated, "access_token", "sat_");
        assert_eq!(
            err(refresh_iam(&base, rft).await).await,
            "401 unauthenticated",
            "reuse is fatal for IAM families too"
        );
        let dead = get(&base, "/api/v1/organizations/tos/directory/self", sat2).await;
        assert_eq!(err(dead).await, "401 unauthenticated", "the replay revoked the rotated tokens too");

        let bot: Value = silicon_login(&base, STK).await.json().await.unwrap();
        let sat = bot["access_token"].as_str().unwrap();
        let logout = |bearer: Option<&str>| {
            let req = http().post(format!("{base}/api/v1/logout"));
            bearer.into_iter().fold(req, reqwest::RequestBuilder::bearer_auth).send()
        };
        let own = "/api/v1/organizations/tos/directory/self";
        assert_eq!(err(logout(Some(sat)).await.unwrap()).await, "403 forbidden", "a silicon logout is local");
        assert_eq!(get(&base, own, sat).await.status(), 200, "so the silicon's session lives on");
        let alice = carbon_login(&base, "alice").await;
        let cat = alice["access_token"].as_str().unwrap();
        assert_eq!(logout(Some(cat)).await.unwrap().status(), 204, "a carbon's logout ends the session");
        assert_eq!(err(get(&base, own, cat).await).await, "401 unauthenticated");
        assert_eq!(err(logout(None).await.unwrap()).await, "401 unauthenticated");
        assert_eq!(err(logout(Some(GARBAGE)).await.unwrap()).await, "401 unauthenticated");
    }

    #[tokio::test]
    async fn what_a_delivery_said_is_what_introspection_reports_next() {
        let base = start(default_seed()).await;
        let (url, _got) = receiver().await;
        let deliver = |event_type: &'static str, version: u64, members: Value| {
            let body =
                json!({"url": url, "event_type": event_type, "org": "tos", "version": version, "members": members});
            http().post(format!("{base}/_stub/deliver")).json(&body).send()
        };
        let bot: Value = silicon_login(&base, STK).await.json().await.unwrap();
        let slt = slt(&base, bot["access_token"].as_str().unwrap(), None).await;
        let tokens: Value = exchange(&base, &[("app_id", APP), ("slt", &slt)]).await.json().await.unwrap();
        let oat = tokens["access_token"].as_str().unwrap();
        let snapshot = |intro: Value| {
            (intro["authorization"]["tags"].clone(), intro["authorization"]["membership_version"].clone())
        };
        let seeded = (json!([tag_json("tos", "ops")]), json!(1));
        assert_eq!(snapshot(introspect(&base, oat).await), seeded, "the seed, before any delivery");

        let updated = "organization.membership.updated.v1";
        deliver(updated, 5, json!([{"actor": "bot:tos", "tags": ["ops", "tech"]}])).await.unwrap();
        let retagged = (json!([tag_json("tos", "ops"), tag_json("tos", "tech")]), json!(5));
        assert_eq!(snapshot(introspect(&base, oat).await), retagged, "the snapshot repeats the delivered row");
        deliver(updated, 2, json!([{"actor": "bot:tos", "tags": []}])).await.unwrap();
        assert_eq!(snapshot(introspect(&base, oat).await), retagged, "an older version never wins");
        deliver("organization.silicon.removed.v1", 6, json!([{"actor": "bot:tos"}])).await.unwrap();
        assert_eq!(introspect(&base, oat).await, json!({"active": false}), "a removed member's token is inactive");
    }

    /// The official client, end to end: negotiation, login, the snapshot, the error envelope,
    /// both webhook envelopes through its verifier, refresh with a replay, revocation and reuse.
    #[tokio::test]
    async fn the_official_client_accepts_the_stub_end_to_end() {
        use silicon_iam_client::{
            Client, Credential, EnvironmentKey, Error, IdempotencyKey, Mutation,
            models::{AuthorizationTag, OAuthRevocationRequest, TokenIntrospectionRequest},
            webhook::{WebhookError, WebhookSecret, WebhookSecretKeyring, WebhookVerifier},
        };
        let base = start(default_seed()).await;
        let credential = Credential::application(APP, SECRET);
        let client = Client::builder(&base).unwrap().credential(credential).auto_update(false).build().unwrap();
        let negotiated = client.system().negotiate().await.unwrap();
        assert_eq!(negotiated.selected_api_version, "v1");
        assert_eq!(negotiated.supported_api_versions, ["v1"]);

        // The browser's half by hand: IAM's login page 302s back with the slt.
        let login = format!(
            "{base}/api/v1/login?app_id={APP_Q}&redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcb&org_id=tos&as=alice"
        );
        let res = http().get(&login).send().await.unwrap();
        let location = res.headers()[header::LOCATION].to_str().unwrap().to_owned();
        let (_, slt) = location.split_once("?slt=").unwrap();
        let tokens = client.oauth().login(APP, slt, &Mutation::new()).await.unwrap();
        assert_eq!((tokens.actor.public_id.as_str(), tokens.org_id.as_deref()), ("alice", Some("tos")));
        assert_eq!(tokens.expires_in, 1800);
        let oat = tokens.access_token.as_str();
        let Err(Error::Api(api)) = client.oauth().login(APP, slt, &Mutation::new()).await else {
            panic!("a spent slt is an API error");
        };
        assert_eq!((api.status, api.code.as_str()), (400, "invalid_grant"));
        assert!(api.request_id.is_some(), "the envelope carries a request id");

        let names =
            |tags: Option<Vec<AuthorizationTag>>| tags.map(|t| t.into_iter().map(|t| t.name).collect::<Vec<_>>());
        let snapshot = client.oauth().authorization(oat, None).await.unwrap().expect("an org-bound access token");
        assert_eq!((snapshot.public_id.as_str(), snapshot.org_id.as_str()), ("alice", "tos"));
        assert_eq!(snapshot.org_role.as_deref(), Some("owner"));
        assert_eq!(names(snapshot.tags), Some(vec!["tech".to_owned()]));
        assert_eq!((snapshot.membership_id, snapshot.membership_version), (membership_id("tos", "alice"), 1));
        assert!(client.oauth().authorization(&tokens.refresh_token, None).await.unwrap().is_none());
        let request = TokenIntrospectionRequest { token: tokens.refresh_token.clone(), token_type_hint: None };
        let inspected = client.oauth().introspect(&request, None).await.unwrap();
        assert!(inspected.active && inspected.authorization.is_none(), "a live refresh token, no snapshot");
        assert!(client.oauth().authorization(oat, Some("acme")).await.unwrap().is_none(), "a mismatched context");
        assert!(client.oauth().authorization(oat, Some("tos")).await.unwrap().is_some());
        let Err(Error::Api(api)) = client.oauth().authorization(oat, Some("Acme")).await else {
            panic!("a malformed context is refused");
        };
        assert_eq!((api.status, api.code.as_str()), (400, "invalid_request"));

        // Webhooks through the crate's verifier, in both envelopes; the snapshot follows the delivery.
        let (url, got) = receiver().await;
        let keyring = WebhookSecretKeyring::new(1, WebhookSecret::new(WEBHOOK_SECRET).unwrap()).unwrap();
        let verifier = WebhookVerifier::new(keyring);
        let key = EnvironmentKey::new(default_seed().app.testing_key.unwrap()).unwrap();
        let deliver = |body: Value| http().post(format!("{base}/_stub/deliver")).json(&body).send();
        let updated = json!({"url": url, "event_type": "organization.membership.updated.v1", "org": "tos", "version": 2, "members": [{"actor": "alice", "tags": []}]});
        assert_eq!(deliver(updated).await.unwrap().status(), 200);
        let (headers, bytes) = got.lock().unwrap().remove(0);
        let verified = verifier.verify(&headers, &bytes).unwrap();
        assert!(!verified.is_testing());
        assert_eq!(verified.event().event_type, "organization.membership.updated.v1");
        assert_eq!(verified.event().data["current"]["members"][0]["membership"]["tags"], json!([]));
        assert_eq!(verified.verify_testing_environment(&key), Err(WebhookError::ProductionEvent));
        let snapshot = client.oauth().authorization(oat, None).await.unwrap().unwrap();
        assert_eq!((names(snapshot.tags), snapshot.membership_version), (Some(vec![]), 2), "as the webhook said");

        let removed = json!({"url": url, "event_type": "organization.silicon.removed.v1", "org": "tos", "envelope": "test", "version": 3, "members": [{"actor": "bot:tos"}]});
        assert_eq!(deliver(removed).await.unwrap().status(), 200);
        let (headers, bytes) = got.lock().unwrap().remove(0);
        let verified = verifier.verify(&headers, &bytes).unwrap();
        assert!(verified.is_testing());
        verified.verify_testing_environment(&key).unwrap();
        let other = EnvironmentKey::new("x".repeat(32)).unwrap();
        assert_eq!(verified.verify_testing_environment(&other), Err(WebhookError::TestingEnvironmentMismatch));
        assert_eq!(verified.event().data["current"]["members"], json!([tombstone("tos", "bot:tos", 3)]));

        // Refresh rotates, the same key replays, revocation shows at once, reuse kills the family.
        let key = IdempotencyKey::generate();
        let renewed =
            client.oauth().refresh(APP, &tokens.refresh_token, &Mutation::with_key(key.clone())).await.unwrap();
        let replayed = client.oauth().refresh(APP, &tokens.refresh_token, &Mutation::with_key(key)).await.unwrap();
        assert_eq!(replayed.access_token, renewed.access_token, "a retry with the same key gets the same answer");
        assert!(client.oauth().authorization(oat, None).await.unwrap().is_some(), "the old access token outlives it");
        let revoke = |token: String| {
            let client = &client;
            async move {
                client.oauth().revoke(&OAuthRevocationRequest { token, token_type_hint: None }, &Mutation::new()).await
            }
        };
        revoke(renewed.access_token.clone()).await.unwrap();
        assert!(client.oauth().authorization(&renewed.access_token, None).await.unwrap().is_none());
        let Err(Error::Api(api)) = client.oauth().refresh(APP, &tokens.refresh_token, &Mutation::new()).await else {
            panic!("refresh-token reuse is an API error");
        };
        assert_eq!((api.status, api.code.as_str()), (400, "invalid_grant"));
        assert!(client.oauth().authorization(oat, None).await.unwrap().is_none(), "the family is gone");
        revoke(renewed.refresh_token).await.unwrap();
        revoke(GARBAGE.to_owned()).await.unwrap();
    }

    #[tokio::test]
    async fn stub_deliver_posts_a_signed_webhook_in_both_envelopes() {
        let base = start(default_seed()).await;
        let (url, got) = receiver().await;
        let deliver = |body: Value| http().post(format!("{base}/_stub/deliver")).json(&body).send();
        let members = json!([
            {"actor": "bot:tos", "status": "active", "tags": ["ops"]},
            {"actor": "alice", "status": "removed", "display_name": "Alice A."},
        ]);
        let body = json!({"url": url, "event_type": "organization.membership.updated.v1", "org": "tos", "members": members, "version": 4});
        let res = deliver(body).await.unwrap();
        assert_eq!(res.status(), 200);
        let reply: Value = res.json().await.unwrap();
        assert_eq!(reply["status"], 204, "the receiver's answer comes back");
        let (headers, bytes) = got.lock().unwrap().remove(0);
        assert_signed(&headers, &bytes);
        assert_eq!(headers["x-silicon-iam-event-id"].to_str().unwrap(), reply["event_id"].as_str().unwrap());
        let event: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(event["spec_version"], "1.0");
        assert_eq!(event["event_type"], "organization.membership.updated.v1");
        assert_eq!(event["event_id"], reply["event_id"]);
        assert_eq!(event["organization_id"], json!(org_uuid("tos")));
        assert_eq!(
            event["aggregate"],
            json!({"type": "organization_membership", "id": membership_id("tos", "bot:tos"), "version": 4})
        );
        assert!(DateTime::parse_from_rfc3339(event["occurred_at"].as_str().unwrap()).is_ok());
        let rows = event["data"]["current"]["members"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let bot = &rows[0];
        assert_eq!(bot["membership"]["id"], json!(membership_id("tos", "bot:tos")));
        assert_eq!(bot["membership"]["status"], "active");
        assert_eq!(bot["membership"]["removed_at"], Value::Null);
        assert_eq!(bot["membership"]["tags"], json!([tag_json("tos", "ops")]));
        assert_eq!(bot["membership"]["version"], 4);
        assert_eq!(
            (bot["organization"]["org_id"].clone(), bot["organization"]["name"].clone()),
            (json!("tos"), json!("Team of Silicons"))
        );
        assert_eq!(bot["principal"]["public_id"], "bot:tos");
        assert_eq!(bot["principal"]["type"], "silicon");
        assert_eq!(bot["principal"]["display_name"], "Bot", "the seeded name fills in");
        assert_eq!(bot["principal"]["principal_id"], json!(principal_id("bot:tos")));
        assert_eq!(bot["resource"]["type"], "organization_membership");
        assert_eq!(bot["resource"]["principal_type"], "silicon");
        assert_eq!(bot["roles"]["org_role"], "member");
        let alice = &rows[1];
        assert_eq!(alice["membership"]["status"], "removed");
        assert!(alice["membership"]["removed_at"].is_string());
        assert_eq!(alice["membership"]["tags"], json!([]));
        assert_eq!(alice["principal"]["display_name"], "Alice A.", "the request's name wins");
        assert_eq!(alice["roles"]["org_role"], "owner");
        assert_eq!(alice["resource"]["status"], "removed");

        let event_id = Uuid::now_v7();
        let body = json!({"url": url, "event_type": "organization.silicon.removed.v1", "org": "tos", "envelope": "test", "event_id": event_id, "members": [{"actor": "bot:tos", "status": "removed"}]});
        let reply: Value = deliver(body).await.unwrap().json().await.unwrap();
        assert_eq!(reply["event_id"], json!(event_id));
        let (headers, bytes) = got.lock().unwrap().remove(0);
        assert_signed(&headers, &bytes);
        assert_eq!(headers["x-silicon-iam-event-id"].to_str().unwrap(), event_id.to_string());
        let event: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(sorted_keys(&event), ["test"]);
        let mut keys = sorted_keys(&event["test"]);
        keys.sort_unstable();
        assert_eq!(keys, ["data", "metadata", "testing_key"]);
        assert_eq!(event["test"]["testing_key"], json!(default_seed().app.testing_key));
        assert_eq!(event["test"]["metadata"]["event_type"], "organization.silicon.removed.v1");
        assert_eq!(event["test"]["metadata"]["event_id"], json!(event_id));
        assert_eq!(event["test"]["metadata"]["aggregate"]["version"], 1);
        assert_eq!(event["test"]["metadata"]["aggregate"]["id"], json!(membership_id("tos", "bot:tos")));
        let rows = event["test"]["data"]["current"]["members"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(sorted_keys(&rows[0]), ["authorization", "resource"], "a removal carries tombstones, not rows");
        assert_eq!(
            rows[0],
            json!({"authorization": "removed", "resource": {
                "id": membership_id("tos", "bot:tos"), "principal_id": principal_id("bot:tos"),
                "principal_type": "silicon", "status": "removed", "type": "organization_membership", "version": 1,
            }})
        );

        let unreachable =
            json!({"url": "http://127.0.0.1:1/hook", "event_type": "organization.membership.updated.v1", "org": "tos"});
        assert_eq!(err(deliver(unreachable).await.unwrap()).await, "502 delivery_failed");
        let mut seed = default_seed();
        seed.app.testing_key = None;
        let keyless = start(seed).await;
        let body =
            json!({"url": url, "event_type": "organization.membership.updated.v1", "org": "tos", "envelope": "test"});
        let res = http().post(format!("{keyless}/_stub/deliver")).json(&body).send().await.unwrap();
        assert_eq!(err(res).await, "422 validation_failed", "no testing key, no test envelope");
        assert!(got.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_expired_access_token_is_unauthenticated() {
        let mut seed = default_seed();
        seed.access_ttl_secs = 0;
        let base = start(seed).await;
        let body: Value = silicon_login(&base, STK).await.json().await.unwrap();
        let res = get(&base, "/api/v1/organizations/tos/directory/self", body["access_token"].as_str().unwrap());
        assert_eq!(err(res.await).await, "401 unauthenticated");
    }

    #[test]
    fn the_embedded_seed_loads_from_a_file_too() {
        let path = std::env::temp_dir().join(format!("iam-seed-{}.json", Uuid::new_v4()));
        std::fs::write(&path, serde_json::to_vec(&default_seed()).unwrap()).unwrap();
        let seed = seed_from_file(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(seed.app.app_id, APP);
        assert_eq!(seed.app.secret, SECRET);
        assert_eq!(seed.app.webhook_secret, WEBHOOK_SECRET);
        assert_eq!(seed.app.testing_key.as_deref().map(str::len), Some(32));
        assert_eq!(seed.carbons.len(), 2);
        assert_eq!(seed.silicons[0].token, STK);
        assert_eq!(seed.access_ttl_secs, 1800);
    }
}
