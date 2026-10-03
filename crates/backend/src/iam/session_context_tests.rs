//! Database-backed IAM 5 regressions. Run with isolated SS_CONTEXT_TEST_DATABASE and
//! SS_CONTEXT_TEST_REDIS; ClickHouse is not needed for this identity-only fixture.
use super::*;
use crate::{config::Config, frontend, http::Inner, iam_stub, store};
use std::sync::Arc;

async fn fixture() -> (AppState, tokio::task::JoinHandle<()>, reqwest::Client) {
    let mut seed = iam_stub::default_seed();
    seed.app.testing_key = None;
    let app_secret = seed.app.secret.clone();
    let webhook_secret = seed.app.webhook_secret.clone();
    let (addr, task) = iam_stub::serve(seed, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let vars = HashMap::from([
        ("SS_ORIGIN", "http://localhost:3000".to_owned()),
        ("SS_KEY", "ab".repeat(32)),
        ("CLICKHOUSE_URL", "http://unused:unused@127.0.0.1:1/unused".into()),
        ("CLICKHOUSE_QUERY_PASSWORD", "unused".into()),
        ("DATABASE_URL", std::env::var("SS_CONTEXT_TEST_DATABASE").expect("isolated Postgres URL")),
        ("REDIS_URL", std::env::var("SS_CONTEXT_TEST_REDIS").expect("isolated Redis URL")),
        ("SILICON_IAM_URL", format!("http://{addr}")),
        ("SILICON_IAM_APP_ID", "spacestation".into()),
        ("SILICON_IAM_APP_SECRET", app_secret),
        ("SILICON_IAM_WEBHOOK_SECRET", webhook_secret),
    ]);
    let cfg = Config::from_vars(|name| vars.get(name).cloned()).unwrap();
    let pg = sqlx::PgPool::connect(&cfg.database_url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pg).await.unwrap();
    let redis = redis::aio::ConnectionManager::new(redis::Client::open(cfg.redis_url.as_str()).unwrap()).await.unwrap();
    let ch = store::Clickhouse::new(&cfg.clickhouse_url, &cfg.clickhouse_query_password).unwrap();
    let watermarks = store::Watermarks::new(ch.clone());
    let store = store::Store {
        pg,
        redis,
        ch,
        watermarks,
        flushed: tokio::sync::broadcast::channel(16).0,
        staged: Arc::default(),
    };
    let iam = iam::Client::connect(&cfg).await.unwrap();
    let lease = store::Lease::start(store.redis.clone());
    let frontend = frontend::Collector::from_config(&cfg);
    let state = AppState(Arc::new(Inner {
        cfg,
        store,
        iam,
        lease,
        stop: tokio::sync::watch::channel(false).1,
        triggers: Arc::default(),
        hub: Default::default(),
        keys: Default::default(),
        visible: Default::default(),
        telemetry: None,
        frontend,
    }));
    (state, task, reqwest::Client::new())
}

async fn post(http: &reqwest::Client, state: &AppState, path: &str, token: Option<&str>, body: Value) -> Value {
    let mut request = http
        .post(format!("{}{path}", state.cfg.iam_url))
        .header("idempotency-key", Uuid::new_v4().to_string())
        .json(&body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let reply = request.send().await.unwrap();
    assert!(reply.status().is_success(), "IAM fixture refused {}", reply.status());
    reply.json().await.unwrap()
}

async fn slt(http: &reqwest::Client, state: &AppState, org: &str) -> String {
    let challenge = post(http, state, "/api/v1/login/challenges", None, json!({"carbon_id": "c:alice"})).await;
    let path = format!("/api/v1/login/challenges/{}/verify", challenge["session_id"].as_str().unwrap());
    let actor = post(http, state, &path, None, json!({"code": "000000"})).await;
    post(
        http,
        state,
        "/api/v1/app-auth/short-lived-tokens",
        actor["access_token"].as_str(),
        json!({"app_id": "spacestation", "org_id": org}),
    )
    .await["slt"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn browser_headers(state: &AppState, group: &str, secret: &str) -> HeaderMap {
    HeaderMap::from_iter([
        (header::COOKIE, format!("{GROUP_COOKIE}={group}; {COOKIE}={secret}").parse().unwrap()),
        (header::ORIGIN, state.cfg.origin.parse().unwrap()),
        (axum::http::HeaderName::from_static(CONTEXT_HEADER), sha256_hex(secret).parse().unwrap()),
    ])
}

#[tokio::test]
#[ignore = "requires isolated Postgres and Redis URLs in SS_CONTEXT_TEST_DATABASE and SS_CONTEXT_TEST_REDIS"]
async fn saved_contexts_receipts_refresh_and_logout_remain_independent() {
    let (state, stub, http) = fixture().await;
    let group = URL_SAFE_NO_PAD.encode(crypto::random::<32>());
    let code_a = slt(&http, &state, "tos").await;
    let login_reply = login(
        State(state.clone()),
        Query(HashMap::from([("org".into(), "tos".into())])),
        browser_headers(&state, &group, "unused"),
    )
    .await
    .unwrap();
    let cookie = login_reply.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned();
    let auth_url = url::Url::parse(login_reply.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let callback_url = auth_url.query_pairs().find(|(k, _)| k == "redirect_uri").unwrap().1.into_owned();
    let callback_url = url::Url::parse(&callback_url).unwrap();
    let nonce = callback_url.query_pairs().find(|(k, _)| k == "state").unwrap().1.into_owned();
    let mut landing_headers = HeaderMap::new();
    landing_headers.insert(header::COOKIE, cookie.parse().unwrap());
    let mut query = HashMap::from([("slt".into(), code_a.clone()), ("state".into(), "wrong-state".into())]);
    assert_eq!(
        callback(State(state.clone()), Query(query.clone()), landing_headers.clone()).await.unwrap_err().code,
        "login_expired"
    );
    query.insert("state".into(), nonce);
    let callback_reply = callback(State(state.clone()), Query(query), landing_headers).await.unwrap();
    let a = callback_reply
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.strip_prefix("ss_session=").and_then(|v| v.split(';').next()))
        .unwrap()
        .to_owned();
    assert_eq!(a, open(&state, &code_a, Some("tos"), false, Some(&group)).await.unwrap());
    // A spent code cannot create another session row or adopt another browser's group.
    assert!(open(&state, &code_a, Some("tos"), true, None).await.is_err());
    assert!(open(&state, &code_a, Some("tos"), false, Some("another-browser")).await.is_err());
    let b = open(&state, &slt(&http, &state, "acme").await, Some("acme"), false, Some(&group)).await.unwrap();
    assert_eq!(load(&state, &a).await.unwrap().unwrap().org, "tos");
    assert_eq!(load(&state, &b).await.unwrap().unwrap().org, "acme");
    let rows = contexts(State(state.clone()), browser_headers(&state, &group, &b)).await.unwrap().0;
    assert_eq!(rows.as_array().unwrap().len(), 2);
    let mut stale = browser_headers(&state, &group, &b);
    stale.insert(CONTEXT_HEADER, sha256_hex(&a).parse().unwrap());
    assert_eq!(
        select_context(State(state.clone()), stale, Json(Selection { context_id: sha256_hex(&a) }))
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    let selected = select_context(
        State(state.clone()),
        browser_headers(&state, &group, &b),
        Json(Selection { context_id: sha256_hex(&a) }),
    )
    .await
    .unwrap();
    assert!(selected.headers()[header::SET_COOKIE].to_str().unwrap().starts_with(&format!("{COOKIE}={a};")));
    let outsider = URL_SAFE_NO_PAD.encode(crypto::random::<32>());
    assert!(
        select_context(
            State(state.clone()),
            browser_headers(&state, &outsider, &b),
            Json(Selection { context_id: sha256_hex(&a) })
        )
        .await
        .is_err()
    );

    let seen = load(&state, &a).await.unwrap().unwrap();
    // IAM completed a refresh, but our process died before storing it. The next request must
    // reconstruct the same receipt and recover rather than reuse the refresh token under a new key.
    let key = mutation_key(&format!("iam5-refresh:{}:{}", seen.id_hash, seen.ort));
    let rotated = state.iam.refresh(&seen.ort, &key.to_string()).await.unwrap();
    let (one, two) = tokio::join!(refresh(&state, &seen), refresh(&state, &seen));
    let one = one.unwrap();
    let two = two.unwrap();
    assert_eq!(one.oat, rotated.oat);
    assert_eq!(one.ort, two.ort);
    assert_ne!(one.ort, seen.ort);
    assert_eq!(load(&state, &b).await.unwrap().unwrap().org, "acme");
    // Old contract and another testing world are inaccessible without deleting account data.
    sqlx::query("UPDATE sessions SET iam_contract = 0 WHERE id_hash = $1")
        .bind(&seen.id_hash)
        .execute(&state.store.pg)
        .await
        .unwrap();
    assert!(load(&state, &a).await.unwrap().is_none());
    sqlx::query("UPDATE sessions SET iam_contract = 5, world = 'other-world' WHERE id_hash = $1")
        .bind(&seen.id_hash)
        .execute(&state.store.pg)
        .await
        .unwrap();
    assert!(load(&state, &a).await.unwrap().is_none());
    sqlx::query("UPDATE sessions SET world = $2 WHERE id_hash = $1")
        .bind(&seen.id_hash)
        .bind(&state.iam.world)
        .execute(&state.store.pg)
        .await
        .unwrap();
    let out = logout(State(state.clone()), browser_headers(&state, &group, &a)).await.unwrap();
    assert!(out.headers()[header::SET_COOKIE].to_str().unwrap().starts_with(&format!("{COOKIE}={b};")));
    assert!(load(&state, &a).await.unwrap().is_none());
    assert!(open(&state, &code_a, Some("tos"), false, Some(&group)).await.is_err(), "logout must survive replay");
    assert!(load(&state, &b).await.unwrap().is_some());
    state.lease.release().await;
    stub.abort();
}
