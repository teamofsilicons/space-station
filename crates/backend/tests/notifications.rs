//! The notification engine against the local docker services and the IAM stub: a definition is
//! saved (and refused, one code per mistake), a record lands, the flush triggers it, the run
//! stores one event and delivers it to a mission-control socket and to a signed webhook, the
//! cooldown silences the same `dedup_key` and then lets it through, a row of the wrong shape
//! becomes a dev error and nothing else, and `test` shows rows without moving anything. One
//! flow, in order, because each step needs what the one before produced.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::Json as AxumJson;
use axum::http::HeaderMap;
use axum::routing::post;
use futures_util::{SinkExt, StreamExt};
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::HeaderName;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use space_station_backend::config::Config;
use space_station_backend::iam_stub::{self, Seed};
use space_station_backend::{App, webhooks};
use space_station_shared::secrets::hex32;
use space_station_shared::wire::{Batch, Entry, Metadata};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;
use uuid::Uuid;

const APP_ID: &str = "tos>spacestation";
const APP_SECRET: &str = "ask_stubstubstubstubstubstubstubstubstubstubabc";
const WEBHOOK_SECRET: &str = "whs_stubstubstubstubstubstubstubstubstubstubabc";

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

/// A free port, so the origin (and the login's redirect URI) is known before the server binds.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// One org, one carbon who owns it.
fn seed(org: &str) -> Seed {
    serde_json::from_value(json!({
        "app": {"app_id": APP_ID, "secret": APP_SECRET, "webhook_secret": WEBHOOK_SECRET},
        "orgs": [{"org_id": org, "name": "Test Org", "tags": ["tech"]}],
        "carbons": [{"carbon_id": "alice", "name": "Alice", "memberships": {org: {"org_role": "owner", "tags": ["tech"]}}}],
        "silicons": [],
    }))
    .expect("the stub's seed shape")
}

fn config(port: u16, origin: &str, iam: SocketAddr) -> Config {
    let vars = HashMap::from([
        ("PORT", port.to_string()),
        ("SS_ORIGIN", origin.to_owned()),
        ("SS_KEY", format!("{}{}", hex32(), hex32())),
        ("SS_ALLOW_PRIVATE_WEBHOOKS", "1".into()),
        ("CLICKHOUSE_URL", env("CLICKHOUSE_URL", "http://dev:dev@localhost:8123/space_station")),
        ("CLICKHOUSE_QUERY_PASSWORD", env("CLICKHOUSE_QUERY_PASSWORD", "ss_query")),
        ("DATABASE_URL", env("DATABASE_URL", "postgres://dev:dev@localhost:5433/space_station")),
        ("REDIS_URL", env("REDIS_URL", "redis://localhost:6379")),
        ("SILICON_IAM_URL", format!("http://{iam}")),
        ("SILICON_IAM_APP_ID", APP_ID.into()),
        ("SILICON_IAM_APP_SECRET", APP_SECRET.into()),
        ("SILICON_IAM_WEBHOOK_SECRET", WEBHOOK_SECRET.into()),
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
        Api { bearer: Some(token.into()), http: self.http.clone(), jar: self.jar.clone(), ..Api::new(&self.origin) }
    }

    fn cookie(&self) -> String {
        self.jar.cookies(&Url::parse(&self.origin).unwrap()).unwrap().to_str().unwrap().to_owned()
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = self.http.request(method, format!("{}{path}", self.base)).header("Origin", &self.origin);
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

    /// A 2xx body, or a panic naming the failure.
    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let (status, value) = self.call(method, path, body).await;
        assert!(status.is_success(), "{path}: {status} {value}");
        value
    }

    async fn get(&self, path: &str) -> Value {
        self.ok(Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        self.ok(Method::POST, path, Some(body)).await
    }

    /// The `{code}` of a refusal, with its status.
    async fn refused(&self, method: Method, path: &str, body: Option<Value>) -> (u16, String) {
        let (status, value) = self.call(method, path, body).await;
        (status.as_u16(), value["error"]["code"].as_str().unwrap_or("none").to_owned())
    }
}

/// Plays the browser through the stub as `carbon`, bound to `org`: login → IAM (`?as=`) → callback.
async fn login(api: &Api, carbon: &str, org: &str) {
    let res = api.http.get(format!("{}/auth/login?org={org}", api.base)).send().await.unwrap();
    let authorize = res.headers()["location"].to_str().unwrap().to_owned();
    let res = api.http.get(format!("{authorize}&as={carbon}")).send().await.unwrap();
    let callback = res.headers()["location"].to_str().unwrap().to_owned();
    assert!(api.http.get(&callback).send().await.unwrap().status().is_redirection(), "the callback lands on next");
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws(url: &str, headers: &[(&str, &str)]) -> Ws {
    let mut request = url.into_client_request().unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(HeaderName::try_from(*name).unwrap(), value.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

/// The next text frame within fifteen seconds.
async fn next_text(ws: &mut Ws) -> Value {
    loop {
        let message = timeout(Duration::from_secs(15), ws.next()).await.expect("a frame within 15 s").unwrap().unwrap();
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

/// A webhook endpoint on 127.0.0.1 that hands every delivery (headers, raw body) to the test.
async fn receiver() -> (String, UnboundedReceiver<(HeaderMap, String)>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let app = Router::new().route(
        "/hook",
        post(move |headers: HeaderMap, body: String| {
            let _ = tx.send((headers, body));
            async { AxumJson(json!({"ok": true})) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, rx)
}

/// One record into `orders` over the ingest socket, the way the daemon sends it.
async fn record(ws: &mut Ws, key: &str, record: Value) {
    let batch = Batch {
        batch_id: Uuid::new_v4(),
        records: vec![Entry {
            key: key.into(),
            metadata: Metadata {
                record_id: Uuid::new_v4(),
                table_id: "orders".into(),
                event_ts_ms: chrono::Utc::now().timestamp_millis(),
                system: None,
                cpu_pct: None,
                gpu_pct: None,
                ram_pct: None,
                disk_free_mb: None,
            },
            record,
        }],
    };
    ws.send(Message::text(serde_json::to_string(&batch).unwrap())).await.unwrap();
    assert_eq!(next_text(ws).await["status"], "ok", "the batch was accepted");
}

const SQL: &str = "SELECT record.id::String AS dedup_key, concat('order ', record.id::String) AS text, \
                   map('amount', record.amount::String) AS metadata FROM orders WHERE record.amount::Float64 > 100";

#[tokio::test(flavor = "multi_thread")]
async fn a_notification_fires_delivers_and_cools_down() {
    let org = format!("n{}", &hex32()[..12]);
    let port = free_port();
    let origin = format!("http://127.0.0.1:{port}");
    let (iam, _stub) = iam_stub::serve(seed(&org), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let app = App::start(config(port, &origin, iam)).await.expect("docker services and the stub must be reachable");
    let api = Api::new(&origin);
    let orgs = format!("/orgs/{org}");
    login(&api, "alice", &org).await;
    let key = api.post(&format!("{orgs}/tables"), json!({"id": "orders"})).await["key"].as_str().unwrap().to_owned();
    let (url, mut hooks) = receiver().await;
    let hook = api.post(&format!("{orgs}/webhooks"), json!({"url": url})).await;
    let (hook_id, secret) = (hook["id"].as_str().unwrap().to_owned(), hook["secret"].as_str().unwrap().to_owned());

    // Every way a definition can be wrong, one code each.
    let def = |patch: Value| {
        let mut def = json!({"name": "Big order", "triggers": [{"table": "orders"}], "sql": SQL, "access": []});
        for (k, v) in patch.as_object().unwrap() {
            def[k] = v.clone();
        }
        def
    };
    let path = format!("{orgs}/notifications");
    for (patch, recipients, code) in [
        (json!({"name": " "}), json!([]), "invalid_name"),
        (json!({"sql": "SELECT * FROM nope"}), json!([]), "invalid_sql"),
        (json!({"sql": "DROP TABLE orders"}), json!([]), "invalid_sql"),
        (json!({"triggers": []}), json!([]), "invalid_trigger"),
        (json!({"triggers": [{"table": "nope"}]}), json!([]), "invalid_trigger"),
        (json!({"triggers": [{"table": "orders", "where": "record.x >"}]}), json!([]), "invalid_trigger"),
        (json!({"triggers": [{"schedule": "*/5 * * *"}]}), json!([]), "invalid_cron"),
        (json!({"delay": "2x"}), json!([]), "invalid_duration"),
        (json!({"cooldown": "60d"}), json!([]), "invalid_duration"),
        (json!({}), json!(["@bob"]), "recipients_not_in_access"),
        (json!({}), json!(["ops"]), "recipients_not_in_access"),
        (json!({}), json!([format!("webhook:{}", Uuid::new_v4())]), "unknown_webhook"),
    ] {
        let body = json!({"def": def(patch.clone()), "recipients": recipients});
        assert_eq!(api.refused(Method::POST, &path, Some(body)).await, (422, code.into()), "{patch} {recipients}");
    }

    // Three notifications: the one under test, one whose rows are the wrong shape, and one whose
    // delay outlasts the test, so `test` has something to show that has never fired.
    let create = async |patch: Value, recipients: Value| {
        api.post(&path, json!({"def": def(patch), "recipients": recipients})).await
    };
    let big = create(
        json!({"triggers": [{"table": "orders", "where": "record.amount::Float64 > 100"}],
               "delay": "1s", "cooldown": "8s", "description": "over 100"}),
        json!(["@alice", format!("webhook:{hook_id}")]),
    )
    .await;
    assert_eq!(big["def"]["access"], json!(["@alice"]), "the creator is appended");
    assert_eq!((big["enabled"].as_bool(), big["created_by"].as_str()), (Some(true), Some("alice")));
    assert_eq!(big["def"]["delay"], "1s", "the definition is stored with its defaults filled in");
    let bad = create(
        json!({"name": "Bad rows", "delay": "1s",
               "sql": "SELECT record.amount::Float64 AS dedup_key, 'x' AS text, map('a', 'b') AS metadata FROM orders"}),
        json!([]),
    )
    .await;
    let probe = create(
        json!({"name": "Probe", "delay": "1h",
               "sql": "SELECT record.id::String AS dedup_key, 'probe' AS text, map('a', 'b') AS metadata FROM orders"}),
        json!([]),
    )
    .await;
    let of = |n: &Value, suffix: &str| format!("{orgs}/notifications/{}{suffix}", n["id"].as_str().unwrap());
    assert_eq!(api.get(&path).await.as_array().unwrap().len(), 3);

    // One matching record: flush → trigger → delay → run → one event, on the socket and the hook.
    let mut mc = ws(
        &format!("ws://127.0.0.1:{port}/api/ws/mission-control?org={org}"),
        &[("Cookie", &api.cookie()), ("Origin", &origin)],
    )
    .await;
    let mut ingest = ws(&format!("ws://127.0.0.1:{port}/api/ws/ingest"), &[]).await;
    record(&mut ingest, &key, json!({"id": "o1", "amount": 150})).await;
    let frame = next_text(&mut mc).await;
    let fired = Instant::now();
    assert_eq!(frame["type"], "notification");
    assert_eq!(frame["notification"], big["id"]);
    assert_eq!(frame["name"], "Big order");
    assert_eq!(frame["dedup_key"], "o1");
    assert_eq!(frame["text"], "order o1");
    assert_eq!(frame["metadata"], json!({"amount": "150"}));
    assert!(frame["event_id"].is_number() && frame["fired_at"].is_string());

    let (headers, body) = timeout(Duration::from_secs(15), hooks.recv()).await.expect("a delivery").unwrap();
    let header = |name: &str| headers[name].to_str().unwrap().to_owned();
    assert_eq!(body, r#"{"dedup_key":"o1","metadata":{"amount":"150"},"text":"order o1"}"#);
    assert_eq!(header("x-space-station-event-id"), frame["event_id"].to_string());
    assert_eq!(header("x-space-station-notification"), big["id"].as_str().unwrap());
    let seconds: i64 = header("x-space-station-timestamp").parse().unwrap();
    let signature = header("x-space-station-signature");
    assert_eq!(signature, webhooks::sign(&secret, seconds, body.as_bytes()), "the signature is over seconds.body");
    assert_ne!(signature, webhooks::sign("whsec-wrong", seconds, body.as_bytes()), "under this webhook's secret");

    let events = api.get(&of(&big, "/events")).await;
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!((events[0]["dedup_key"].as_str(), events[0]["text"].as_str()), (Some("o1"), Some("order o1")));

    // The same run, seen by the notification whose rows are not {dedup_key, text, metadata}.
    eventually("the bad row to become a dev error", || async {
        let errors = api.get(&format!("{orgs}/dev-errors")).await;
        errors.as_array().unwrap().iter().any(|e| e["source"] == "notification" && e["ref"] == bad["id"])
    })
    .await;
    assert_eq!(api.get(&of(&bad, "/events")).await, json!([]), "a bad row sends nothing");

    // The cooldown: the same dedup_key is silent inside the window and fires again after it.
    record(&mut ingest, &key, json!({"id": "o1", "amount": 160})).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(api.get(&of(&big, "/events")).await.as_array().unwrap().len(), 1, "still inside the cooldown");
    assert_eq!(api.post(&of(&big, "/test"), json!({})).await["rows"], json!([]), "the run happened and moved past it");
    tokio::time::sleep(Duration::from_secs(9).saturating_sub(fired.elapsed())).await;
    record(&mut ingest, &key, json!({"id": "o1", "amount": 170})).await;
    eventually("the same dedup_key to fire again after the cooldown", || async {
        api.get(&of(&big, "/events")).await.as_array().unwrap().len() == 2
    })
    .await;

    // `test` runs the sql now and moves nothing, so it answers the same rows twice.
    let first = api.post(&of(&probe, "/test"), json!({})).await;
    let again = api.post(&of(&probe, "/test"), json!({})).await;
    assert_eq!(first["rows"].as_array().unwrap().len(), 3, "every record since it was created");
    assert_eq!(first["rows"], again["rows"], "testing does not advance the cursors");
    assert_eq!(first["last_trigger_at"], Value::Null, "this one has never fired: \"no trigger seen yet\"");
    assert!(api.post(&of(&big, "/test"), json!({})).await["last_trigger_at"].is_string(), "that one has");

    // Subscribing edits only the recipients; editing writes a new version and can pause it.
    assert_eq!(api.post(&of(&probe, "/subscribe"), json!({})).await["recipients"], json!(["@alice"]));
    let edit = json!({"def": def(json!({"name": "Probe v2", "enabled": false}))});
    let edited = api.ok(Method::PUT, &of(&probe, ""), Some(edit)).await;
    assert_eq!((edited["def"]["name"].as_str(), edited["enabled"].as_bool()), (Some("Probe v2"), Some(false)));
    assert_eq!(edited["recipients"], json!(["@alice"]), "an absent recipients keeps the subscribers");
    assert_eq!(api.ok(Method::DELETE, &of(&probe, "/subscribe"), None).await["recipients"], json!([]));

    // An API key with the notifications scope reads, and only reads.
    let created = api.post(&format!("{orgs}/api-keys"), json!({"scopes": ["notifications"]})).await;
    let key_api = api.with_bearer(created["key"].as_str().unwrap());
    assert_eq!(key_api.get(&path).await.as_array().unwrap().len(), 3);
    assert_eq!(key_api.get(&of(&big, "/events")).await.as_array().unwrap().len(), 2);
    assert_eq!(key_api.refused(Method::POST, &of(&big, "/test"), Some(json!({}))).await, (401, "unauthorized".into()));

    // This server lives in no testing environment, so a `test` envelope — however well signed —
    // is somebody else's and is refused before its rows are read.
    let ts = chrono::Utc::now().timestamp();
    let event_id = Uuid::now_v7();
    let envelope = json!({"test": {"testing_key": "TkTkTkTkTkTkTkTkTkTkTkTkTkTkTkTk",
        "metadata": {"spec_version": "1.0", "event_id": event_id, "event_type": "organization.membership.updated.v1",
                     "occurred_at": chrono::Utc::now().to_rfc3339(), "organization_id": Uuid::now_v7(),
                     "aggregate": {"type": "organization_membership", "id": Uuid::now_v7(), "version": 1}},
        "data": {"changed_fields": ["membership.tags"], "current": {"members": []}}}})
    .to_string();
    let res = api
        .http
        .post(format!("{origin}/api/iam/webhook"))
        .header("x-silicon-iam-event-id", event_id.to_string())
        .header("x-silicon-iam-key-version", "1")
        .header("x-silicon-iam-timestamp", ts.to_string())
        .header("x-silicon-iam-signature", webhooks::sign(WEBHOOK_SECRET, ts, envelope.as_bytes()))
        .body(envelope)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "invalid_testing_key");

    ingest.close(None).await.unwrap();
    mc.close(None).await.unwrap();
    app.stop().await;
}
