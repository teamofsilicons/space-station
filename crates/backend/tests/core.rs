//! The whole server in one process against the local docker services and the IAM stub: every
//! login door (browser, terminal loopback, `POST /auth/session`), the org binding, tables, ingest
//! through the real client crate, flush, query, mission control, state, versions, tokens, API keys,
//! the silicon flow, session refresh and re-proof, the IAM webhook feeding the mirror and revoking,
//! and the Origin rule — one flow, in order, because each step needs what the one before produced.
//! Fails loudly when a service is down.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use futures_util::{SinkExt, StreamExt};
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::HeaderName;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use space_station::SpaceClient;
use space_station_backend::config::Config;
use space_station_backend::iam_stub::{self, Seed};
use space_station_backend::{App, crypto, webhooks};
use space_station_shared::secrets::{hex32, sha256_hex};
use space_station_shared::wire::{Batch, Entry, Metadata};
use sqlx::PgPool;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;
use uuid::Uuid;

const APP_ID: &str = "tos>spacestation";
const APP_SECRET: &str = "ask_stubstubstubstubstubstubstubstubstubstubabc";
const WEBHOOK_SECRET: &str = "whs_stubstubstubstubstubstubstubstubstubstubabc";
/// The secret before the last rotation: deliveries signed with it must still verify.
const OLD_WEBHOOK_SECRET: &str = "whs_previouspreviouspreviouspreviouspreviousabc";
/// The testing environment this server pretends to live in; the wire never shows it.
const TEST_KEY: &str = "TkTkTkTkTkTkTkTkTkTkTkTkTkTkTkTk";
const STK: &str = "stk-0123456789abcdef0123456789abcdef";

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn database_url() -> String {
    env("DATABASE_URL", "postgres://dev:dev@localhost:5433/space_station")
}

/// A free port, so the origin (and the login's redirect URI) is known before the server binds.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Two orgs: alice owns `org` (tag tech) and is a member of `other` (tag sales); bob belongs to
/// neither; the silicon lives in `org` with the tag ops. Tags here are what the stub's *directory*
/// knows — Space Station learns them only through webhooks.
fn seed(org: &str, other: &str) -> Seed {
    serde_json::from_value(json!({
        "app": {"app_id": APP_ID, "secret": APP_SECRET, "webhook_secret": WEBHOOK_SECRET},
        "orgs": [
            {"org_id": org, "name": "Test Org", "tags": ["tech", "ops"]},
            {"org_id": other, "name": "Second Org", "tags": ["sales"]},
        ],
        "carbons": [
            {"carbon_id": "alice", "name": "Alice", "memberships": {
                org: {"org_role": "owner", "tags": ["tech"]},
                other: {"org_role": "member", "tags": ["sales"]},
            }},
            {"carbon_id": "bob", "name": "Bob", "memberships": {}},
        ],
        "silicons": [{"silicon_id": format!("bot:{org}"), "name": "Bot", "token": STK, "tags": ["ops"]}],
    }))
    .expect("the stub's seed shape")
}

fn config(port: u16, origin: &str, iam: SocketAddr) -> Config {
    let vars = HashMap::from([
        ("PORT", port.to_string()),
        ("SS_ORIGIN", origin.to_owned()),
        ("SS_KEY", format!("{}{}", hex32(), hex32())),
        ("CLICKHOUSE_URL", env("CLICKHOUSE_URL", "http://dev:dev@localhost:8123/space_station")),
        ("CLICKHOUSE_QUERY_PASSWORD", env("CLICKHOUSE_QUERY_PASSWORD", "ss_query")),
        ("DATABASE_URL", database_url()),
        ("REDIS_URL", env("REDIS_URL", "redis://localhost:6379")),
        ("SILICON_IAM_URL", format!("http://{iam}")),
        ("SILICON_IAM_APP_ID", APP_ID.into()),
        ("SILICON_IAM_APP_SECRET", APP_SECRET.into()),
        ("SILICON_IAM_WEBHOOK_SECRET", WEBHOOK_SECRET.into()),
        ("SILICON_IAM_WEBHOOK_SECRET_PREVIOUS", OLD_WEBHOOK_SECRET.into()),
        ("SILICON_IAM_TEST_KEY", TEST_KEY.into()),
    ]);
    let mut cfg = Config::from_vars(|name| vars.get(name).cloned()).unwrap();
    cfg.bind = SocketAddr::from(([127, 0, 0, 1], port));
    cfg
}

/// The server's HTTP API from a test: a cookie jar, no redirects followed, `Origin` set.
struct Api {
    http: Client,
    jar: Arc<Jar>,
    base: String,
    origin: String,
    bearer: Option<String>,
}

impl Api {
    fn new(origin: &str) -> Api {
        let jar = Arc::new(Jar::default());
        let http =
            Client::builder().cookie_provider(jar.clone()).redirect(reqwest::redirect::Policy::none()).build().unwrap();
        Api { http, jar, base: format!("{origin}/api"), origin: origin.into(), bearer: None }
    }

    fn with_bearer(&self, token: &str) -> Api {
        Api {
            http: self.http.clone(),
            jar: self.jar.clone(),
            base: self.base.clone(),
            origin: self.origin.clone(),
            bearer: Some(token.into()),
        }
    }

    fn cookie(&self) -> String {
        self.jar.cookies(&Url::parse(&self.origin).unwrap()).unwrap().to_str().unwrap().to_owned()
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>, origin: Option<&str>) -> (StatusCode, Value) {
        let mut req =
            self.http.request(method, format!("{}{path}", self.base)).header("Origin", origin.unwrap_or(&self.origin));
        if let Some(bearer) = &self.bearer {
            req = req.bearer_auth(bearer);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        let text = res.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call(Method::GET, path, None, None).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, path, Some(body), None).await
    }

    /// A 2xx body, or a panic naming the failure.
    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let (status, value) = self.call(method, path, body, None).await;
        assert!(status.is_success(), "{path}: {status} {value}");
        value
    }

    /// `(status, error code)` of a refusal.
    async fn refused(&self, method: Method, path: &str, body: Option<Value>) -> (u16, String) {
        let (status, value) = self.call(method, path, body, None).await;
        (status.as_u16(), value["error"]["code"].as_str().unwrap_or("none").to_owned())
    }

    async fn query(&self, org: &str, sql: &str, restrict: Value) -> Value {
        self.ok(Method::POST, &format!("/orgs/{org}/query"), Some(json!({"sql": sql, "restrict": restrict}))).await
    }
}

/// Plays the browser as `carbon`: our login redirects to IAM bound to `org`, the stub signs them
/// in (`?as=`) and comes back with `?slt=`, the callback exchanges it and lands on `next`.
async fn login(api: &Api, carbon: &str, org: &str) {
    let res = api.http.get(format!("{}/auth/login?org={org}&next=/o/x", api.base)).send().await.unwrap();
    assert_eq!(res.status(), 302, "login redirects to IAM");
    let authorize = Url::parse(res.headers()["location"].to_str().unwrap()).unwrap();
    let q: HashMap<String, String> = authorize.query_pairs().into_owned().collect();
    assert!(authorize.path().ends_with("/api/v1/login"), "{authorize}");
    assert_eq!(q["app_id"], APP_ID, "the canonical Application id");
    assert_eq!(q["redirect_uri"], format!("{}/auth/callback", api.base));
    assert_eq!(q["org_id"], org, "the login is bound to the org asked for");
    let res = api.http.get(format!("{authorize}&as={carbon}")).send().await.unwrap();
    assert_eq!(res.status(), 302, "the stub redirects to the callback: {}", res.text().await.unwrap());
    let callback = res.headers()["location"].to_str().unwrap().to_owned();
    assert!(callback.starts_with(&format!("{}/auth/callback?", api.base)), "{callback}");
    assert!(callback.contains("slt="), "{callback}");
    let res = api.http.get(&callback).send().await.unwrap();
    assert!(
        res.status().is_redirection(),
        "the callback lands on next: {} {}",
        res.status(),
        res.text().await.unwrap()
    );
    assert_eq!(res.headers()["location"], format!("{}/o/x", api.origin).as_str());
}

/// The stub, spoken to the way a person, a silicon or the `iam` CLI would.
struct Iam {
    http: Client,
    base: String,
}

impl Iam {
    fn new(addr: SocketAddr) -> Iam {
        Iam {
            http: Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap(),
            base: format!("http://{addr}"),
        }
    }

    async fn post_json(&self, path: &str, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
        let mut req = self
            .http
            .post(format!("{}{path}", self.base))
            .header("idempotency-key", Uuid::new_v4().to_string())
            .json(&body);
        if let Some(bearer) = bearer {
            req = req.bearer_auth(bearer);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        (status, res.json().await.unwrap_or(Value::Null))
    }

    /// A carbon's own IAM bearer: a login challenge answered with the environment's `000000`.
    async fn carbon_bearer(&self, carbon: &str) -> String {
        let (status, challenge) = self.post_json("/api/v1/login/challenges", None, json!({"carbon_id": carbon})).await;
        assert!(status.is_success(), "login challenge: {status} {challenge}");
        let path = format!("/api/v1/login/challenges/{}/verify", challenge["session_id"].as_str().unwrap());
        let (status, tokens) = self.post_json(&path, None, json!({"code": "000000"})).await;
        assert!(status.is_success(), "verify: {status} {tokens}");
        tokens["access_token"].as_str().unwrap().to_owned()
    }

    /// A silicon's own IAM bearer, from the long-lived token Space Station never sees.
    async fn silicon_bearer(&self, silicon: &str) -> String {
        let body = json!({"silicon_id": silicon, "silicon_token": STK});
        let (status, tokens) = self.post_json("/api/v1/silicon-auth/token", None, body).await;
        assert!(status.is_success(), "silicon-auth: {status} {tokens}");
        tokens["access_token"].as_str().unwrap().to_owned()
    }

    /// What `iam login --app-id` / `iam silicon-login --app-id` print: a short-lived token for this
    /// Application, bound to `org` when given.
    async fn slt(&self, bearer: &str, org: Option<&str>) -> String {
        let mut body = json!({"app_id": APP_ID});
        if let Some(org) = org {
            body["org_id"] = org.into();
        }
        let (status, slt) = self.post_json("/api/v1/app-auth/short-lived-tokens", Some(bearer), body).await;
        assert!(status.is_success(), "short-lived token: {status} {slt}");
        assert_eq!(slt["expires_in"], 120);
        slt["slt"].as_str().unwrap().to_owned()
    }

    /// The Application's own call, made from outside the server: Basic auth and a form body.
    async fn app_form(&self, path: &str, form: &str) -> (StatusCode, Value) {
        let res = self
            .http
            .post(format!("{}{path}", self.base))
            .basic_auth(APP_ID, Some(APP_SECRET))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("idempotency-key", Uuid::new_v4().to_string())
            .body(form.to_owned())
            .send()
            .await
            .unwrap();
        let status = res.status();
        (status, res.json().await.unwrap_or(Value::Null))
    }
}

/// One `current.members[]` row the way IAM projects it, down to what the mirror reads.
fn member_row(
    org: &str,
    org_name: &str,
    actor: &str,
    membership: &str,
    status: &str,
    tags: &[&str],
    version: i64,
) -> Value {
    let id = |_: &str| Uuid::now_v7();
    let kind = if actor.contains(':') { "silicon" } else { "carbon" };
    let removed_at = if status == "active" { Value::Null } else { json!("2026-09-05T07:00:00Z") };
    json!({
        "membership": {"id": membership, "status": status, "version": version, "removed_at": removed_at, "authorization_epoch": version,
                       "tags": tags.iter().map(|t| json!({"id": id(t), "name": t})).collect::<Vec<_>>()},
        "organization": {"id": id(org), "org_id": org, "name": org_name, "status": "active", "version": 1},
        "principal": {"principal_id": id(actor), "public_id": actor, "type": kind, "display_name": actor, "status": "active"},
        "resource": {"type": "organization_membership", "id": membership, "status": status, "version": version},
        "roles": {"capabilities": [], "job_role": "", "org_role": "member"},
    })
}

/// A production envelope.
fn event(event_type: &str, members: Vec<Value>) -> Value {
    json!({"spec_version": "1.0", "event_id": Uuid::now_v7(), "event_type": event_type, "occurred_at": Utc::now().to_rfc3339(),
           "organization_id": Uuid::now_v7(), "aggregate": {"type": "organization_membership", "id": Uuid::now_v7(), "version": 1},
           "data": {"changed_fields": ["membership.tags"], "current": {"members": members}}})
}

/// The same event as a testing environment delivers it.
fn test_envelope(key: &str, event: &Value) -> Value {
    let mut metadata = event.clone();
    let data = metadata.as_object_mut().unwrap().remove("data").unwrap();
    json!({"test": {"testing_key": key, "metadata": metadata, "data": data}})
}

/// Signs `body` as IAM would at `ts` with `secret` and posts it to `path` on the server. The
/// event-id header must equal the body's own event id — the official verifier cross-checks them —
/// and every delivery declares key version 1, the one IAM signs with today.
async fn deliver(api: &Api, path: &str, secret: &str, ts: i64, body: &str) -> StatusCode {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let event_id = json
        .get("test")
        .map(|t| &t["metadata"]["event_id"])
        .unwrap_or(&json["event_id"])
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    api.http
        .post(format!("{}{path}", api.origin))
        .header("content-type", "application/json")
        .header("x-silicon-iam-event-id", event_id)
        .header("x-silicon-iam-timestamp", ts.to_string())
        .header("x-silicon-iam-key-version", "1")
        .header("x-silicon-iam-signature", webhooks::sign(secret, ts, body.as_bytes()))
        .body(body.to_owned())
        .send()
        .await
        .unwrap()
        .status()
}

/// The sessions row behind a secret, as the server stores it.
#[derive(sqlx::FromRow)]
struct SessionRow {
    oat_enc: String,
    ort_enc: String,
    refresh_key: Option<Uuid>,
    expires_at: DateTime<Utc>,
    checked_at: DateTime<Utc>,
}

async fn session_row(pg: &PgPool, secret: &str) -> Option<SessionRow> {
    let sql = "SELECT oat_enc, ort_enc, refresh_key, expires_at, checked_at FROM sessions WHERE id_hash = $1";
    sqlx::query_as(sql).bind(sha256_hex(secret)).fetch_optional(pg).await.unwrap()
}

async fn age_session(pg: &PgPool, secret: &str, set: &str) {
    let sql = format!("UPDATE sessions SET {set} WHERE id_hash = $1");
    sqlx::query(sqlx::AssertSqlSafe(sql)).bind(sha256_hex(secret)).execute(pg).await.unwrap();
}

async fn ws(
    url: &str,
    headers: &[(&str, &str)],
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(HeaderName::try_from(*name).unwrap(), value.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, frame: Value) {
    ws.send(Message::text(frame.to_string())).await.unwrap();
}

/// The next text frame within ten seconds.
async fn next_text(ws: &mut Ws) -> Value {
    loop {
        let message = timeout(Duration::from_secs(10), ws.next()).await.expect("a frame within 10 s").unwrap().unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Polls `check` every 200 ms for up to fifteen seconds.
async fn eventually<F: Future<Output = bool>>(what: &str, mut check: impl FnMut() -> F) {
    for _ in 0..75 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("{what} did not happen within 15 s");
}

fn count(rows: &Value) -> u64 {
    rows["rows"][0]["n"].as_str().unwrap_or("0").parse().unwrap()
}

fn ids(tables: &Value) -> Vec<&str> {
    tables.as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect()
}

/// The `/orgs` entries for this run's orgs. The mirror is shared state: earlier runs against the
/// same database left alice active in orgs of their own, and those are not this run's to assert.
fn ours(listed: Value, orgs: &[&str]) -> Value {
    Value::Array(
        listed.as_array().unwrap().iter().filter(|o| orgs.iter().any(|org| o["id"] == *org)).cloned().collect(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_whole_station_end_to_end() {
    let org = format!("t{}", &hex32()[..12]);
    let other = format!("{org}b");
    let bot = format!("bot:{org}");
    let port = free_port();
    let origin = format!("http://127.0.0.1:{port}");
    let (iam_addr, _stub) = iam_stub::serve(seed(&org, &other), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let cfg = config(port, &origin, iam_addr);
    let key = cfg.key;
    let app = App::start(cfg).await.expect("docker services and the stub must be reachable");
    let pg = PgPool::connect(&database_url()).await.unwrap();
    let iam = Iam::new(iam_addr);
    let api = Api::new(&origin);
    let orgs = format!("/orgs/{org}");

    assert_eq!(api.get("/health").await, (StatusCode::OK, json!({"status": "ok"})), "readiness needs no login");

    // The browser: a login bound to `org`, then who am I. Introspection's `authorization` snapshot
    // is the bootstrap — alice's `tech` tag is on her the moment she signs in, with no webhook — and
    // it also seeds the mirror, so the org list holds `org` (its name still just its id until a
    // webhook carries one).
    assert_eq!(api.get("/orgs").await.0, 401, "no session: the frontend's logged-out signal");
    assert_eq!(api.refused(Method::GET, "/auth/login", None).await, (400, "org_required".into()));
    login(&api, "alice", &org).await;
    assert_eq!(
        api.ok(Method::GET, "/me", None).await,
        json!({"id": "alice", "kind": "carbon", "org": org, "app": origin})
    );
    let listed = ours(api.ok(Method::GET, "/orgs", None).await, &[&org, &other]);
    assert_eq!(listed, json!([{"id": org, "name": org}]));
    let me = api.ok(Method::GET, &format!("{orgs}/me"), None).await;
    assert_eq!(me, json!({"kind": "carbon", "id": "alice", "org": org, "tags": ["tech"]}), "tags from the snapshot");
    let elsewhere = api.refused(Method::GET, &format!("/orgs/{other}/me"), None).await;
    assert_eq!(elsewhere, (403, "not_a_member".into()), "a session is bound to one org");
    assert_eq!(api.refused(Method::GET, "/orgs/bad!/me", None).await, (400, "invalid_org".into()));

    // A terminal with a browser at hand: the loopback flow forwards the single-use IAM token,
    // which the terminal exchanges over POST. Long-lived sessions must never travel in a URL.
    let res = api.http.get(format!("{}/auth/login?org={org}&cli=4242&state=n0nce", api.base)).send().await.unwrap();
    let authorize = res.headers()["location"].to_str().unwrap().to_owned();
    let res = api.http.get(format!("{authorize}&as=alice")).send().await.unwrap();
    let callback = res.headers()["location"].to_str().unwrap().to_owned();
    let res = api.http.get(&callback).send().await.unwrap();
    assert_eq!(res.status(), 303);
    let landed = Url::parse(res.headers()["location"].to_str().unwrap()).unwrap();
    assert_eq!((landed.host_str(), landed.port()), (Some("127.0.0.1"), Some(4242)), "{landed}");
    let query: HashMap<String, String> = landed.query_pairs().into_owned().collect();
    assert_eq!(query["state"], "n0nce", "the nonce is echoed unchanged");
    assert!(!query.contains_key("token"), "a session secret must not cross browser history");
    let minted = api.ok(Method::POST, "/auth/session", Some(json!({"slt": query["slt"], "org": org}))).await;
    let terminal = api.with_bearer(minted["token"].as_str().unwrap());
    assert_eq!(terminal.ok(Method::GET, "/me", None).await["id"], "alice");
    let (status, _) = terminal.call(Method::POST, "/auth/logout", None, Some("http://evil.example")).await;
    assert_eq!(status, 204, "no browser attaches a bearer, so the Origin rule does not apply to it");
    assert_eq!(terminal.get("/me").await.0, 401, "the terminal's row is gone");
    assert_eq!(api.get("/me").await.0, 200, "the browser's is untouched");
    let res = api.http.get(&callback).send().await.unwrap();
    assert_eq!(res.status(), 401, "the login cookie was spent with the callback: {}", res.text().await.unwrap());

    // No browser: the slt `iam login --app-id --org` prints, posted to /auth/session. Single-use,
    // bound to the org it names, and refused when it names none or another.
    let cat = iam.carbon_bearer("alice").await;
    let slt = iam.slt(&cat, Some(&org)).await;
    let minted = api.ok(Method::POST, "/auth/session", Some(json!({"slt": slt, "org": org}))).await;
    let token = minted["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("sscli-"));
    let terminal = api.with_bearer(&token);
    assert_eq!(terminal.ok(Method::GET, "/me", None).await["org"], org);
    let replay = api.refused(Method::POST, "/auth/session", Some(json!({"slt": slt, "org": org}))).await;
    assert_eq!(replay, (401, "invalid_slt".into()), "an slt is single-use");
    let unscoped = iam.slt(&cat, None).await;
    let refused = api.refused(Method::POST, "/auth/session", Some(json!({"slt": unscoped, "org": org}))).await;
    assert_eq!(refused, (400, "org_required".into()), "everything here lives inside an org");
    let elsewhere = iam.slt(&cat, Some(&other)).await;
    let refused = api.refused(Method::POST, "/auth/session", Some(json!({"slt": elsewhere, "org": org}))).await;
    assert_eq!(refused, (400, "org_mismatch".into()), "an slt bound to another org does not open this one");
    let refused = api.refused(Method::POST, "/auth/session", Some(json!({"slt": "nope", "org": "Bad Org"}))).await;
    assert_eq!(refused, (400, "invalid_org".into()));

    // A table, its one-time key, and the Origin rule on cookie mutations.
    let (status, body) = api
        .call(Method::POST, &format!("{orgs}/tables"), Some(json!({"id": "orders"})), Some("http://evil.example"))
        .await;
    assert_eq!((status.as_u16(), body["error"]["code"].as_str()), (403, Some("bad_origin")));
    let key_orders = api.ok(Method::POST, &format!("{orgs}/tables"), Some(json!({"id": "orders"}))).await["key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(key_orders.starts_with("table-orders-"));
    let (status, body) = api.post(&format!("{orgs}/tables"), json!({"id": "orders"})).await;
    assert_eq!((status.as_u16(), body["error"]["code"].as_str()), (409, Some("exists")));
    let tables = api.ok(Method::GET, &format!("{orgs}/tables"), None).await;
    assert_eq!(tables[0]["id"], "orders");
    assert_eq!(tables[0]["access"], json!(["@alice"]), "the creator is appended");
    assert_eq!(tables[0]["records"], 0);
    let emptied = api.ok(Method::PUT, &format!("{orgs}/tables/orders"), Some(json!({"access": []}))).await;
    assert_eq!(emptied["access"], json!(["@alice"]), "an access edit cannot leave the table unreachable");

    // Ingest through the real client crate: three records, one with a 40 KB value.
    let home = tempfile::tempdir().unwrap();
    let errors = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    let client = SpaceClient::builder(&key_orders)
        .url(&origin)
        .home(home.path())
        .on_error(move |e| sink.lock().unwrap().push(e.to_string()))
        .build()
        .unwrap();
    client.record(json!({"x": 1.5, "name": "first"}));
    client.record(json!({"x": 2.5, "big": "y".repeat(40_000)}));
    client.record(json!({"x": 3.5}));
    assert!(client.flush(), "the records reached the daemon");
    let count_sql = "SELECT count() AS n FROM orders";
    eventually("the three records to be flushed", || async {
        count(&api.query(&org, count_sql, json!({})).await) == 3
    })
    .await;

    // A raw batch: a bogus key, a fresh record, then the same record again (a duplicate).
    let record_id = Uuid::new_v4();
    let entry = |key: &str, record_id| Entry {
        key: key.into(),
        metadata: Metadata {
            record_id,
            table_id: "orders".into(),
            event_ts_ms: 1_725_000_000_000,
            system: None,
            cpu_pct: None,
            gpu_pct: None,
            ram_pct: None,
            disk_free_mb: None,
        },
        record: json!({"x": 4.5, "raw": true}),
    };
    let mut ingest = ws(&format!("ws://127.0.0.1:{port}/api/ws/ingest"), &[]).await;
    let bogus = Uuid::new_v4();
    let batch = Batch {
        batch_id: Uuid::new_v4(),
        records: vec![entry(&format!("table-orders-{}", hex32()), bogus), entry(&key_orders, record_id)],
    };
    send(&mut ingest, serde_json::to_value(&batch).unwrap()).await;
    let ack = next_text(&mut ingest).await;
    assert_eq!(ack["batch_id"], json!(batch.batch_id));
    assert_eq!(ack["status"], "rejected");
    assert_eq!(
        ack["rejected"],
        json!([{"record_id": bogus, "code": "unauthorized", "reason": "unknown table key"}]),
        "only the bogus key is refused"
    );
    let replay = Batch { batch_id: Uuid::new_v4(), records: vec![entry(&key_orders, record_id)] };
    send(&mut ingest, serde_json::to_value(&replay).unwrap()).await;
    let ack = next_text(&mut ingest).await;
    assert_eq!(ack["rejected"][0]["code"], "duplicate");
    ingest.close(None).await.unwrap();
    eventually("the raw record to be flushed", || async { count(&api.query(&org, count_sql, json!({})).await) == 4 })
        .await;

    // Query: casting, truncation, watermarks.
    let sql = "SELECT record.x::Float64 AS x, record.name AS name, record.big AS big, cursor FROM orders WHERE record.x::Float64 > 2 ORDER BY x";
    let result = api.query(&org, sql, json!({})).await;
    let rows = result["rows"].as_array().unwrap();
    assert_eq!(rows.iter().map(|r| r["x"].as_f64().unwrap()).collect::<Vec<_>>(), [2.5, 3.5, 4.5]);
    let big = rows[0]["big"].as_str().unwrap();
    assert!(big.len() <= 32 * 1024 && big.contains("...[40000]..."), "the 40 KB value was cut in the middle");
    assert!(rows[0]["name"].is_null());
    let watermark = result["watermarks"]["orders"].as_u64().unwrap();
    let last_cursor: u64 = rows[2]["cursor"].as_str().unwrap().parse().unwrap();
    assert_eq!(watermark, last_cursor, "the watermark is the last cursor flushed");
    let table = &api.ok(Method::GET, &format!("{orgs}/tables"), None).await[0];
    assert_eq!((table["records"].as_u64(), table["watermark"].as_u64()), (Some(4), Some(watermark)));
    let overview = api.ok(Method::GET, &format!("{orgs}/tables/overview?window=1h"), None).await;
    assert_eq!((overview["tables"].as_u64(), overview["records"].as_u64()), (Some(1), Some(4)));
    assert_eq!(overview["top"], json!([{"id": "orders", "records": 4}]));
    assert!(overview["avg_lag_ms"].is_number());
    let (status, body) =
        api.post(&format!("{orgs}/query"), json!({"sql": "SELECT * FROM url('http://x', 'CSV', 'a String')"})).await;
    assert_eq!((status.as_u16(), body["error"]["code"].as_str()), (400, Some("table_function")));

    // Mission control: subscribe with a where, record a matching row, receive the trigger.
    let mc_url = format!("ws://127.0.0.1:{port}/api/ws/mission-control?org={org}");
    let mut mc = ws(&mc_url, &[("Cookie", &api.cookie()), ("Origin", &origin)]).await;
    send(&mut mc, json!({"type": "subscribe", "id": "sub1", "triggers": [{"table": "orders", "where": "record.x::Float64 > 4.9"}]})).await;
    assert_eq!(
        next_text(&mut mc).await,
        json!({"type": "subscribed", "id": "sub1", "watermarks": {"orders": watermark}})
    );
    send(&mut mc, json!({"type": "subscribe", "id": "bad", "triggers": [{"table": "nope"}]})).await;
    assert_eq!(
        next_text(&mut mc).await,
        json!({"type": "error", "id": "bad", "code": "forbidden", "message": "table nope is not visible"})
    );
    let peek = json!({"table": "orders", "where": "1 IN (SELECT 1 FROM nope)"});
    send(&mut mc, json!({"type": "subscribe", "id": "peek", "triggers": [peek]})).await;
    let refused = next_text(&mut mc).await;
    assert_eq!(
        (refused["id"].as_str(), refused["code"].as_str()),
        (Some("peek"), Some("invalid_trigger")),
        "a trigger where is planned under the subscriber's own visibility: {refused}"
    );
    client.record(json!({"x": 5.0}));
    client.flush();
    assert_eq!(next_text(&mut mc).await, json!({"type": "trigger", "id": "sub1"}));
    let delta =
        api.query(&org, "SELECT record.x::Float64 AS x FROM orders", json!({"orders": {"from": watermark}})).await;
    let xs: Vec<f64> = delta["rows"].as_array().unwrap().iter().map(|r| r["x"].as_f64().unwrap()).collect();
    assert_eq!(xs, [5.0], "restrict from the watermark returns exactly the new row");
    assert!(delta["watermarks"]["orders"].as_u64().unwrap() > watermark);

    // Windows: versions with the secret check, state with the version rule, is_live.
    let window = api.ok(Method::POST, &format!("{orgs}/windows"), Some(json!({"name": "Orders"}))).await;
    let id = window["id"].as_str().unwrap().to_owned();
    assert_eq!((window["version"].clone(), window["access"].clone()), (Value::Null, json!(["@alice"])));
    let kept = api.ok(Method::PUT, &format!("{orgs}/windows/{id}"), Some(json!({"access": []}))).await;
    assert_eq!(kept["access"], json!(["@alice"]), "nor can it leave the window unreachable");
    send(&mut mc, json!({"type": "state", "window": id, "version": "v1", "json": {"n": 1}})).await;
    assert_eq!(next_text(&mut mc).await["code"], "version_not_current");
    let leaked = json!({"name": "v1", "processor": format!("// token spacewindow-{}", hex32()), "renderer": "<div/>"});
    let (status, body) = api.post(&format!("{orgs}/windows/{id}/versions"), leaked).await;
    assert_eq!((status.as_u16(), body["error"]["code"].as_str()), (422, Some("secret_in_code")));
    // The renderer carries the em dash the docs' own template has: the secret scan is over bytes
    // and must not trip over a character that is more than one of them.
    let clean = json!({"name": "v1", "processor": "export default defineProcessor({})",
                       "renderer": "<div>{{ o.id }} — {{ o.amount }}</div>"});
    let version = api.ok(Method::POST, &format!("{orgs}/windows/{id}/versions"), Some(clean)).await;
    assert_eq!((version["name"].as_str(), version["created_by"].as_str()), (Some("v1"), Some("alice")));
    assert_eq!(api.ok(Method::GET, &format!("{orgs}/windows/{id}"), None).await["version"]["name"], "v1");
    send(&mut mc, json!({"type": "state", "window": id, "version": "v1", "json": {"n": 1}})).await;
    eventually("the state to be stored", || async {
        api.ok(Method::GET, &format!("{orgs}/windows/{id}/state"), None).await["json"] == json!({"n": 1})
    })
    .await;
    let state = api.ok(Method::GET, &format!("{orgs}/windows/{id}/state"), None).await;
    assert_eq!(
        (state["metadata"]["is_live"].as_bool(), state["metadata"]["processor_version"].as_str()),
        (Some(true), Some("v1"))
    );
    mc.close(None).await.unwrap();

    // Access tokens: usable as a bearer, rotation invalidates the old one. IAM tokens of any kind
    // are not bearers here.
    let access = api.ok(Method::GET, &format!("{orgs}/access-token"), None).await["token"].as_str().unwrap().to_owned();
    assert!(access.starts_with("spacewindow-"));
    assert_eq!(api.with_bearer(&access).query(&org, count_sql, json!({})).await["rows"][0]["n"], "5");
    let by_token = api.with_bearer(&access).ok(Method::GET, "/me", None).await;
    assert_eq!(by_token, json!({"id": "alice", "kind": "carbon", "org": org, "app": origin}));
    let rotated = api.ok(Method::POST, &format!("{orgs}/access-token/rotate"), None).await;
    assert_ne!(rotated["token"], access);
    assert_eq!(rotated["last_used_at"], Value::Null);
    assert_eq!(api.with_bearer(&access).post(&format!("{orgs}/query"), json!({"sql": count_sql})).await.0, 401);
    for iam_token in [
        cat.as_str(),
        "oat_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ",
        "sat_nopenopenopenopenopenopenopenopenopenopeno",
    ] {
        let refused = api.with_bearer(iam_token).refused(Method::GET, &format!("{orgs}/me"), None).await;
        assert_eq!(refused, (401, "unsupported_bearer".into()), "Space Station never takes an IAM bearer");
    }
    assert_eq!(api.with_bearer(&cat).refused(Method::GET, "/orgs", None).await, (401, "unsupported_bearer".into()));

    // API keys: scoped reads only.
    let created = api.ok(Method::POST, &format!("{orgs}/api-keys"), Some(json!({"scopes": ["tables"]}))).await;
    let api_key = api.with_bearer(created["key"].as_str().unwrap());
    assert_eq!(api_key.query(&org, count_sql, json!({})).await["rows"][0]["n"], "5");
    let (status, body) = api_key.get(&format!("{orgs}/windows")).await;
    assert_eq!((status.as_u16(), body["error"]["code"].as_str()), (401, Some("unauthorized")));
    assert_eq!(api.ok(Method::GET, &format!("{orgs}/api-keys"), None).await[0]["scopes"], json!(["tables"]));

    // The silicon: `iam silicon-login --app-id` mints the slt from its own IAM session, which
    // Space Station never sees; the exchange opens a session bound to its org.
    let sat = iam.silicon_bearer(&bot).await;
    let slt = iam.slt(&sat, Some(&org)).await;
    let minted = api.ok(Method::POST, "/auth/session", Some(json!({"slt": slt, "org": org}))).await;
    let bot_api = api.with_bearer(minted["token"].as_str().unwrap());
    let me = bot_api.ok(Method::GET, &format!("{orgs}/me"), None).await;
    assert_eq!(me, json!({"kind": "silicon", "id": bot, "org": org, "tags": ["ops"]}), "tags from the snapshot");
    assert_eq!(
        bot_api.ok(Method::GET, "/me", None).await,
        json!({"id": bot, "kind": "silicon", "org": org, "app": origin})
    );
    assert_eq!(bot_api.ok(Method::GET, &format!("{orgs}/windows"), None).await, json!([]), "the window is @alice only");
    let refused = api.with_bearer(&sat).refused(Method::POST, "/auth/logout", None).await;
    assert_eq!(refused, (401, "unsupported_bearer".into()), "no row to end");

    // The tag the login snapshot carried already grants access: a table for `ops` is the bot's
    // without any webhook. Webhooks are the later word — a stale (older-version) delivery changes
    // nothing; a newer one that drops the tag takes the table away — through the registered path,
    // in the test envelope.
    api.ok(Method::POST, &format!("{orgs}/tables"), Some(json!({"id": "shared", "access": ["ops"]}))).await;
    assert_eq!(
        ids(&bot_api.ok(Method::GET, &format!("{orgs}/tables"), None).await),
        ["shared"],
        "the snapshot's tag, no webhook needed"
    );
    let now = Utc::now().timestamp();
    let bot_membership = Uuid::now_v7().to_string();
    let bot_row = |tags: &[&str], version| member_row(&org, "Test Org", &bot, &bot_membership, "active", tags, version);
    let tagged = event("organization.silicon.updated.v1", vec![bot_row(&["ops"], 2)]).to_string();
    assert_eq!(deliver(&api, "/api/iam/webhook", WEBHOOK_SECRET, now, &tagged).await, 204);
    assert_eq!(ids(&bot_api.ok(Method::GET, &format!("{orgs}/tables"), None).await), ["shared"]);
    assert_eq!(bot_api.ok(Method::GET, &format!("{orgs}/me"), None).await["tags"], json!(["ops"]));
    assert_eq!(deliver(&api, "/api/iam/webhook", WEBHOOK_SECRET, now, &tagged).await, 204, "a replay is a 204");
    let stale = event("organization.membership.updated.v1", vec![bot_row(&[], 1)]).to_string();
    assert_eq!(
        deliver(&api, "/webhooks/api", OLD_WEBHOOK_SECRET, now, &stale).await,
        204,
        "the previous secret still verifies"
    );
    assert_eq!(
        bot_api.ok(Method::GET, &format!("{orgs}/me"), None).await["tags"],
        json!(["ops"]),
        "version 1 is older than 2"
    );
    let untagged = event("organization.membership.updated.v1", vec![bot_row(&[], 3)]);
    let wrong_key = test_envelope("KkKkKkKkKkKkKkKkKkKkKkKkKkKkKkKk", &untagged).to_string();
    assert_eq!(
        deliver(&api, "/webhooks/api/", WEBHOOK_SECRET, now, &wrong_key).await,
        401,
        "another environment's event"
    );
    let untagged = test_envelope(TEST_KEY, &untagged).to_string();
    assert_eq!(deliver(&api, "/webhooks/api/", WEBHOOK_SECRET, now, &untagged).await, 204, "the registered form");
    assert_eq!(bot_api.ok(Method::GET, &format!("{orgs}/me"), None).await["tags"], json!([]));
    assert_eq!(ids(&bot_api.ok(Method::GET, &format!("{orgs}/tables"), None).await), Vec::<&str>::new());
    // The refusals: a forged body, a wrong secret, an old timestamp, a body that is not an event.
    let forged = webhooks::sign(WEBHOOK_SECRET, now, b"tampered");
    let res = api
        .http
        .post(format!("{origin}/api/iam/webhook"))
        .header("x-silicon-iam-timestamp", now.to_string())
        .header("x-silicon-iam-signature", forged)
        .body(tagged.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert_eq!(
        deliver(&api, "/api/iam/webhook", "whs_wrongwrongwrongwrongwrongwrongwrongwrongabc", now, &tagged).await,
        401
    );
    assert_eq!(
        deliver(&api, "/api/iam/webhook", WEBHOOK_SECRET, now - 600, &tagged).await,
        401,
        "a replay from the past"
    );
    assert_eq!(deliver(&api, "/api/iam/webhook", WEBHOOK_SECRET, now, r#"{"hello": "world"}"#).await, 400);

    // Alice's memberships arrive: her org list grows to the orgs the mirror knows, named, and her
    // identity in `org` carries the tag — while her session stays bound to `org`.
    let alice_rows = vec![
        member_row(&org, "Test Org", "alice", &Uuid::now_v7().to_string(), "active", &["tech"], 1),
        member_row(&other, "Second Org", "alice", &Uuid::now_v7().to_string(), "active", &["sales"], 1),
    ];
    let joined = event("organization.membership.updated.v1", alice_rows).to_string();
    assert_eq!(deliver(&api, "/api/iam/webhook", WEBHOOK_SECRET, now, &joined).await, 204);
    let listed = ours(api.ok(Method::GET, "/orgs", None).await, &[&org, &other]);
    assert_eq!(listed, json!([{"id": org, "name": "Test Org"}, {"id": other, "name": "Second Org"}]));
    assert_eq!(api.ok(Method::GET, &format!("{orgs}/me"), None).await["tags"], json!(["tech"]));
    assert_eq!(api.refused(Method::GET, &format!("/orgs/{other}/me"), None).await.1, "not_a_member");
    assert_eq!(bot_api.ok(Method::GET, "/orgs", None).await, json!([{"id": org, "name": "Test Org"}]));

    // Refresh: when the access token is about to expire the next request rotates the pair under the
    // row lock and clears the idempotency key. Then the reuse discipline: a refresh token the
    // server still holds is spent elsewhere, so its own refresh is a reuse — IAM answers
    // `invalid_grant`, kills the family, and the session ends.
    let before = session_row(&pg, &token).await.expect("the terminal's row");
    assert!(before.refresh_key.is_none());
    age_session(&pg, &token, "expires_at = now() + interval '30 seconds'").await;
    assert_eq!(terminal.get("/me").await.0, 200);
    let after = session_row(&pg, &token).await.unwrap();
    assert_ne!(after.oat_enc, before.oat_enc, "the pair rotated");
    assert_ne!(after.ort_enc, before.ort_enc);
    assert!(after.refresh_key.is_none(), "the key is cleared once the refresh succeeds");
    assert!(after.expires_at > Utc::now() + TimeDelta::minutes(25), "a fresh 1800 s");
    let ort = crypto::open(&key, &after.ort_enc).expect("sealed under SS_KEY");
    let (status, _) =
        iam.app_form("/api/v1/app-auth/tokens", &format!("app_id=tos%3Espacestation&refresh_token={ort}")).await;
    assert_eq!(status, 200, "spent behind the server's back");
    age_session(&pg, &token, "expires_at = now()").await;
    assert_eq!(terminal.refused(Method::GET, "/me", None).await, (401, "session_expired".into()));
    assert!(session_row(&pg, &token).await.is_none(), "the row is gone with the family");

    // Re-proof: a session whose last proof is old is introspected again. Same membership: it goes
    // on, freshly proved. An inactive access token first refreshes its still-good family; a
    // different membership or an explicitly revoked family ends the session.
    let slt = iam.slt(&cat, Some(&org)).await;
    let proved = api.ok(Method::POST, "/auth/session", Some(json!({"slt": slt, "org": org}))).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let proven = api.with_bearer(&proved);
    age_session(&pg, &proved, "checked_at = now() - interval '2 minutes'").await;
    assert_eq!(proven.get("/me").await.0, 200);
    let row = session_row(&pg, &proved).await.unwrap();
    assert!(Utc::now() - row.checked_at < TimeDelta::seconds(10), "re-proved now");
    age_session(&pg, &proved, "membership_id = 'another-membership', checked_at = now() - interval '2 minutes'").await;
    assert_eq!(proven.refused(Method::GET, "/me", None).await, (401, "session_expired".into()));
    assert!(session_row(&pg, &proved).await.is_none());
    let slt = iam.slt(&cat, Some(&org)).await;
    let revoked = api.ok(Method::POST, "/auth/session", Some(json!({"slt": slt, "org": org}))).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let oat = crypto::open(&key, &session_row(&pg, &revoked).await.unwrap().oat_enc).unwrap();
    let (status, _) = iam.app_form("/api/v1/oauth/revoke", &format!("token={oat}")).await;
    assert_eq!(status, 200);
    assert_eq!(api.with_bearer(&revoked).get("/me").await.0, 200, "the proof is still fresh");
    age_session(&pg, &revoked, "checked_at = now() - interval '2 minutes'").await;
    assert_eq!(api.with_bearer(&revoked).get("/me").await.0, 200, "an inactive access token refreshes first");
    let refreshed = session_row(&pg, &revoked).await.unwrap();
    assert_ne!(crypto::open(&key, &refreshed.oat_enc).unwrap(), oat, "a new access token was proved");
    let ort = crypto::open(&key, &refreshed.ort_enc).unwrap();
    let (status, _) = iam.app_form("/api/v1/oauth/revoke", &format!("token={ort}")).await;
    assert_eq!(status, 200, "revoke the entire family");
    age_session(&pg, &revoked, "checked_at = now() - interval '2 minutes'").await;
    assert_eq!(api.with_bearer(&revoked).refused(Method::GET, "/me", None).await, (401, "session_expired".into()));

    // The IAM webhook: alice leaves the org. Her access token dies with her sessions, her name
    // leaves the recipients lists, and the bot is untouched.
    let fresh = api.with_bearer(rotated["token"].as_str().unwrap());
    assert_eq!(fresh.get(&format!("{orgs}/me")).await.0, 200);
    let gone = member_row(&org, "Test Org", "alice", &Uuid::now_v7().to_string(), "removed", &[], 2);
    let removed = event("organization.membership.removed.v1", vec![gone]).to_string();
    assert_eq!(deliver(&api, "/webhooks/api/", WEBHOOK_SECRET, now, &removed).await, 204);
    assert_eq!(fresh.refused(Method::GET, &format!("{orgs}/me"), None).await, (401, "invalid_token".into()));
    assert_eq!(api.get("/me").await.0, 401, "her browser session is gone too");
    assert_eq!(api.get("/orgs").await.0, 401);
    assert_eq!(bot_api.get("/me").await.0, 200, "the bot's session is its own");
    let (status, _) = api.call(Method::POST, "/auth/logout", None, None).await;
    assert_eq!(status, 204, "logging out an ended session is quiet");

    assert!(errors.lock().unwrap().is_empty(), "the client saw no errors: {:?}", errors.lock().unwrap());
    drop(client);
    app.stop().await;
}

/// Not a test: `cargo test -p space-station-backend --test core -- --ignored --nocapture` prints
/// how many small records per second the ingest path and the flusher sustain on this machine.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn throughput() {
    let org = format!("p{}", &hex32()[..12]);
    let port = free_port();
    let origin = format!("http://127.0.0.1:{port}");
    let (iam, _stub) = iam_stub::serve(seed(&org, &format!("{org}b")), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let app = App::start(config(port, &origin, iam)).await.unwrap();
    let api = Api::new(&origin);
    login(&api, "alice", &org).await;
    let key = api.ok(Method::POST, &format!("/orgs/{org}/tables"), Some(json!({"id": "load"}))).await["key"]
        .as_str()
        .unwrap()
        .to_owned();
    let home = tempfile::tempdir().unwrap();
    let client = SpaceClient::builder(&key).url(&origin).home(home.path()).build().unwrap();
    const N: u64 = 50_000;
    let started = std::time::Instant::now();
    for i in 0..N {
        client.record(json!({"i": i, "name": "load", "value": 1.5}));
        if i % 5000 == 0 {
            client.flush();
        }
    }
    client.flush();
    let sql = "SELECT count() AS n FROM load";
    loop {
        if count(&api.query(&org, sql, json!({})).await) == N {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let secs = started.elapsed().as_secs_f64();
    println!("{N} records recorded, shipped, flushed and queryable in {secs:.2} s = {:.0} records/s", N as f64 / secs);
    app.stop().await;
}
