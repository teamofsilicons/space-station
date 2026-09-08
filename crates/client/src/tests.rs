//! Real behaviour, not mocks: every test here talks HTTP to a small responder on 127.0.0.1 and
//! runs the login flow through an actual loopback redirect. Nothing here touches a home or the
//! environment — the package is stateless, so the tests are too, save a temp dir for the runtime
//! and the spool. Grouped by what they cover: the credential value, the two ways in (a browser
//! login, a short-lived token exchanged), the wire (a final 401, errors as given), each family of
//! API calls, and the local half.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use std::{fs, thread};

use serde_json::{Value, json};

use crate::{Auth, Def, Error, Kind, Rows, Space, Trigger, test_home, windows};

/// One HTTP/1.1 responder per test: `handler(method, path, lowercased headers, body)` answers
/// `(status, body)`. Status 302 sends the body as `Location` (that is how the backend hands a
/// terminal its session); status 0 drops the connection, which the client sees as a transport
/// error.
fn serve(handler: impl Fn(&str, &str, &str, &str) -> (u16, String) + Send + Sync + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let handler = handler.clone();
            thread::spawn(move || {
                let mut buf = Vec::new();
                let (line, headers, body) = loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                        let (line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
                        let headers = headers.to_lowercase();
                        let len = headers.lines().find_map(|l| l.strip_prefix("content-length: "));
                        let len: usize = len.and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            let body = String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                            break (line.to_string(), headers, body);
                        }
                    }
                };
                let mut parts = line.split(' ');
                let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                let (status, out) = handler(method, path, &headers, &body);
                // A 302 carries `out` as its Location, which is how the backend hands a terminal
                // its session; every other answer carries it as a JSON body.
                let (extra, answer) = match status {
                    0 => return,
                    302 => (format!("Location: {out}\r\n"), String::new()),
                    _ => ("Content-Type: application/json\r\n".to_string(), out),
                };
                let head = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n{extra}");
                let head = format!("{head}Content-Length: {}\r\n\r\n", answer.len());
                let _ = stream.write_all((head + &answer).as_bytes());
            });
        }
    });
    url
}

/// Every request one station received: method, path (query included) and body.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

/// A `Space` on a station that records what it is asked and answers from `routes`, as a session
/// in org `tos`.
fn station(routes: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static) -> (Space, Seen) {
    let seen: Seen = Seen::default();
    let log = seen.clone();
    let url = serve(move |method, path, _, body| {
        log.lock().unwrap().push((method.into(), path.into(), body.into()));
        routes(method, path, body)
    });
    (Space::new(url, Auth::session("sscli-session")).unwrap().org("tos"), seen)
}

fn error(status: u16, code: &str) -> (u16, String) {
    (status, json!({"error": {"code": code, "message": format!("{code} happened")}}).to_string())
}

fn ok(body: Value) -> (u16, String) {
    (200, body.to_string())
}

fn param<'a>(query: &'a str, name: &str) -> &'a str {
    query.split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v).unwrap_or_default()
}

/// The body of the nth request a station recorded, as JSON.
fn body(seen: &Seen, n: usize) -> Value {
    serde_json::from_str(&seen.lock().unwrap()[n].2).unwrap_or(Value::Null)
}

/// Every (method, path) a station was asked, in order.
fn calls(seen: &Seen) -> Vec<(String, String)> {
    seen.lock().unwrap().iter().map(|(m, p, _)| (m.clone(), p.clone())).collect()
}

// ── the credential value ────────────────────────────────────────────────────────────────────

#[test]
fn an_auth_is_named_by_its_prefix_survives_serde_verbatim_and_never_prints_what_it_holds() {
    let session = Auth::session(" sscli-x ");
    assert_eq!(session.describe(), "a session");
    assert_eq!(Auth::access_token("spacewindow-x").describe(), "an access token");
    assert_eq!(Auth::api_key("apikey-x").describe(), "an API key");
    assert_eq!(Auth::api_key("x").describe(), "a bearer token");
    assert_eq!(format!("{session:?}"), "Auth(a session)");

    let json = serde_json::to_value(&session).unwrap();
    assert_eq!(json, json!({"bearer": "sscli-x"}));
    let back: Auth = serde_json::from_value(json).unwrap();
    assert_eq!((back.token(), back.describe()), ("sscli-x", "a session"));
    let dated = json!({"bearer": "sscli-y", "refresh": {"token": "rft_gone", "iam": "http://iam"}});
    let dated: Auth = serde_json::from_value(dated).unwrap();
    assert_eq!(dated.token(), "sscli-y", "a file from when a credential could still rotate loads as its bearer");
}

#[test]
fn station_origin_rejects_embedded_credentials_and_control_characters() {
    assert!(crate::api::origin("https://user:password@example.com").is_err());
    assert!(crate::api::origin("https://example.com\r\nX-Leak: yes").is_err());
    assert_eq!(crate::api::origin("https://example.com///").unwrap(), "https://example.com");
}

// ── a browser login from a terminal ─────────────────────────────────────────────────────────

/// The browser's half: fetch the link the listener printed and follow the backend's redirect.
/// It runs on a thread of its own because `login` is already waiting for the request it makes.
fn browser(link: &str) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let link = link.to_string();
    thread::spawn(move || {
        let page = ureq::get(&link).call().and_then(|mut r| r.body_mut().read_to_string());
        let _ = tx.send(page.unwrap_or_else(|e| e.to_string()));
    });
    rx
}

/// The backend half of a browser login as `iam::session` performs it: `/api/auth/login?org=…&
/// cli={port}&state={nonce}` 303s the loopback listener to `?{reply}&state=…` (`reply` is what
/// IAM brought back, `slt=oac_fresh` when all is well), and `POST /api/auth/session` spends that
/// short-lived token. Every step is logged: `login <org>`, `session <slt> <org>`.
fn login_station(reply: &'static str, state: Option<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
    let steps = Arc::new(Mutex::new(Vec::new()));
    let log = steps.clone();
    let url = serve(move |method, path, _, body| {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        match (method, route) {
            ("GET", "/api/auth/login") => {
                log.lock().unwrap().push(format!("login {}", param(query, "org")));
                let port: u16 = param(query, "cli").parse().expect("cli is a bare loopback port");
                let echo = state.unwrap_or(param(query, "state"));
                (302, format!("http://127.0.0.1:{port}/?{reply}&state={echo}"))
            }
            ("POST", "/api/auth/session") => {
                let body: Value = serde_json::from_str(body).unwrap();
                log.lock().unwrap().push(format!("session {} {}", body["slt"].as_str().unwrap(), body["org"]));
                match body["slt"] == "oac_fresh" {
                    true => (201, json!({"token": "sscli-terminal"}).to_string()),
                    false => error(400, "invalid_grant"),
                }
            }
            _ => error(404, "not_found"),
        }
    });
    (url, steps)
}

#[test]
fn login_names_the_org_spends_the_short_lived_token_the_backend_redirects_and_returns_the_session() {
    let (url, steps) = login_station("slt=oac_fresh", None);
    let mut page = None;
    let auth = crate::login(&format!("{url}/"), "tos", |link| page = Some(browser(link))).unwrap();

    assert_eq!((auth.token(), auth.describe()), ("sscli-terminal", "a session"));
    assert_eq!(
        *steps.lock().unwrap(),
        ["login tos", "session oac_fresh \"tos\""],
        "the link and the exchange name the org"
    );
    let page = page.unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(page.contains("back to the terminal"), "{page}");
}

#[test]
fn login_refuses_a_redirect_carrying_another_state_and_spends_nothing() {
    let (url, steps) = login_station("slt=oac_fresh", Some("some-other-login"));
    let mut page = None;
    let err = crate::login(&url, "tos", |link| page = Some(browser(link))).unwrap_err();

    assert!(matches!(err, Error::Local(ref m) if m.contains("state")), "{err}");
    let page = page.unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(page.contains("Not signed in"), "{page}");
    assert_eq!(*steps.lock().unwrap(), ["login tos"], "a token from another login is never exchanged");
}

#[test]
fn login_refuses_the_session_token_an_older_backend_redirects_instead_of_a_short_lived_one() {
    let (url, steps) = login_station("token=sscli-minted-by-the-backend", None);
    let mut page = None;
    let err = crate::login(&url, "tos", |link| page = Some(browser(link))).unwrap_err();

    assert!(matches!(err, Error::Local(ref m) if m.contains("session token") && m.contains("older")), "{err}");
    assert!(!err.to_string().contains("sscli-minted"), "never echoed: {err}");
    let page = page.unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(page.contains("Not signed in"), "{page}");
    assert_eq!(*steps.lock().unwrap(), ["login tos"]);

    let (url, _) = login_station("slt=oac_spent", None);
    let spent = crate::login(&url, "tos", |link| drop(browser(link))).unwrap_err();
    assert!(matches!(spent, Error::Api { status: 400, ref code, .. } if code == "invalid_grant"), "{spent}");
}

// ── a short-lived token, exchanged ──────────────────────────────────────────────────────────

#[test]
fn exchange_posts_the_short_lived_token_with_its_org_and_no_bearer_and_returns_the_session() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let url = serve(move |method, path, headers, body| {
        log.lock().unwrap().push((path.to_string(), headers.to_string(), body.to_string()));
        match (method, path, body.contains("slt_fresh")) {
            ("POST", "/api/auth/session", true) => (201, json!({"token": "sscli-minted"}).to_string()),
            ("POST", "/api/auth/session", false) => error(400, "invalid_grant"),
            _ => error(404, "not_found"),
        }
    });

    let auth = crate::exchange(&format!("{url}/"), " slt_fresh\n", "tos").unwrap();
    assert_eq!((auth.token(), auth.describe()), ("sscli-minted", "a session"));
    let (path, headers, body) = seen.lock().unwrap()[0].clone();
    assert_eq!(path, "/api/auth/session");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({"slt": "slt_fresh", "org": "tos"}));
    assert!(!headers.contains("authorization"), "nobody to be yet, so no bearer: {headers}");

    let spent = crate::exchange(&url, "slt_spent", "tos").unwrap_err();
    assert!(matches!(spent, Error::Api { status: 400, ref code, .. } if code == "invalid_grant"), "{spent}");
    assert!(matches!(crate::exchange("http://127.0.0.1:1", "slt_x", "tos"), Err(Error::Transport(_))));
    assert!(matches!(crate::exchange("127.0.0.1:1", "slt_x", "tos"), Err(Error::Local(_))));
}

#[test]
fn exchange_refuses_anything_shaped_like_a_credential_before_it_leaves_the_machine() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let url = serve(move |_, _, _, _| {
        counter.fetch_add(1, SeqCst);
        (500, "nothing should arrive".into())
    });
    let bearer = format!("sat_{}", "a".repeat(43));
    let refresh = format!("rft_{}", "b".repeat(43));
    let app_secret = format!("ask_{}", "k".repeat(43));
    let silicon_token = "stk-0123456789abcdef0123456789abcdef";
    let ours = "spacewindow-0123456789abcdef0123456789abcdef";
    for token in [bearer.as_str(), refresh.as_str(), app_secret.as_str(), silicon_token, ours, "", "  "] {
        let err = crate::exchange(&url, token, "tos").unwrap_err();
        assert!(matches!(err, Error::Local(ref m) if m.contains("not a short-lived token")), "{token:?}: {err}");
        assert!(token.trim().is_empty() || !err.to_string().contains(token), "never echoed: {err}");
    }
    assert_eq!(hits.load(SeqCst), 0, "nothing left the machine");
}

// ── the wire: a final 401, errors as given ──────────────────────────────────────────────────

#[test]
fn a_401_is_final_and_arrives_with_its_status_and_code() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let url = serve(move |_, _, _, _| {
        counter.fetch_add(1, SeqCst);
        error(401, "session_expired")
    });
    let space = Space::new(url, Auth::session("sscli-old")).unwrap().org("tos");
    let err = space.tables().unwrap_err();
    assert!(matches!(err, Error::Api { status: 401, ref code, .. } if code == "session_expired"), "{err}");
    assert_eq!(err.to_string(), "session_expired: session_expired happened");
    assert_eq!(hits.load(SeqCst), 1, "the backend refreshes a session itself; the package never retries");

    let orgless = Space::new("http://127.0.0.1:1", Auth::session("sscli-x")).unwrap();
    assert!(matches!(orgless.tables(), Err(Error::Local(ref m)) if m.contains("no org")));
    assert!(matches!(orgless.orgs(), Err(Error::Transport(_))), "an unreachable station is a transport error");
    assert!(matches!(Space::new("127.0.0.1:8080", Auth::session("x")), Err(Error::Local(_))));
}

#[test]
fn a_refusal_without_a_code_is_named_by_its_status_and_a_reply_that_is_not_the_api_is_transport() {
    let url = serve(|_, path, _, _| match path {
        "/api/orgs/tos/tables" => (502, "<html>bad gateway</html>".into()),
        _ => ok(json!({"unexpected": true})),
    });
    let space = Space::new(url, Auth::session("sscli-x")).unwrap().org("tos");
    let err = space.tables().unwrap_err();
    let gateway = matches!(err, Error::Api { status: 502, ref code, ref message } if code == "http_502" && message.contains("bad gateway"));
    assert!(gateway, "{err}");
    assert!(matches!(space.windows().unwrap_err(), Error::Transport(ref m) if m.contains("unexpected reply")));
}

#[test]
fn logout_ends_a_terminal_session_with_its_bearer_and_sends_nothing_for_any_other_credential() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let url = serve(move |method, path, headers, _| {
        let bearer = headers.lines().find(|l| l.starts_with("authorization: ")).unwrap_or_default().to_string();
        log.lock().unwrap().push(format!("{method} {path} {bearer}"));
        (204, String::new())
    });
    Space::new(url.clone(), Auth::session("sscli-terminal")).unwrap().logout().unwrap();
    Space::new(url.clone(), Auth::api_key("apikey-x")).unwrap().logout().unwrap();
    Space::new(url, Auth::access_token("spacewindow-x")).unwrap().logout().unwrap();
    assert_eq!(*seen.lock().unwrap(), ["POST /api/auth/logout authorization: bearer sscli-terminal"]);
}

// ── tables ──────────────────────────────────────────────────────────────────────────────────

#[test]
fn tables_are_listed_created_rotated_reaccessed_deleted_and_summarised() {
    let key = "table-orders-0123456789abcdef0123456789abcdef";
    let row = json!({"id": "orders", "records": 12, "watermark": 130, "access": ["@alice", "tech"],
                     "created_by": "alice", "created_at": "2026-09-03T10:00:00Z"});
    let listing = json!([row]);
    let (space, seen) = station(move |method, path, _| match (method, path) {
        ("GET", "/api/orgs/tos/tables") => ok(listing.clone()),
        ("POST", "/api/orgs/tos/tables") => (201, json!({"key": key}).to_string()),
        ("PUT", "/api/orgs/tos/tables/orders") => ok(row.clone()),
        ("POST", "/api/orgs/tos/tables/orders/rotate-key") => ok(json!({"key": key})),
        ("DELETE", "/api/orgs/tos/tables/orders") => (204, String::new()),
        ("GET", "/api/orgs/tos/tables/overview?window=1h") => {
            ok(json!({"tables": 1, "records": 12, "top": [{"id": "orders", "records": 12}], "avg_lag_ms": 41.5}))
        }
        _ => error(404, "not_found"),
    });

    let tables = space.tables().unwrap();
    assert_eq!((tables[0].id.as_str(), tables[0].records, tables[0].watermark), ("orders", 12, 130));
    assert_eq!(space.table("orders").unwrap().access, ["@alice", "tech"]);
    assert!(
        matches!(space.table("gone").unwrap_err(), Error::Api { status: 404, ref code, .. } if code == "not_found")
    );
    assert_eq!(space.create_table("orders", &["@alice", "tech"]).unwrap().value, key);
    assert_eq!(body(&seen, 3), json!({"id": "orders", "access": ["@alice", "tech"]}));
    assert_eq!(space.set_table_access("orders", &["@alice"]).unwrap().id, "orders");
    assert_eq!(body(&seen, 4), json!({"access": ["@alice"]}));
    assert_eq!(space.rotate_table_key("orders").unwrap().value, key);
    space.delete_table("orders").unwrap();
    let overview = space.overview("1h").unwrap();
    assert_eq!((overview.records, overview.top[0].id.as_str(), overview.avg_lag_ms), (12, "orders", Some(41.5)));
    assert_eq!(calls(&seen).last().unwrap().1, "/api/orgs/tos/tables/overview?window=1h");
}

// ── space windows ───────────────────────────────────────────────────────────────────────────

#[test]
fn windows_are_created_edited_published_read_and_linked_to_their_page() {
    let window = json!({"id": "w1", "name": "Orders", "access": ["@alice"], "created_by": "alice",
                        "created_at": "2026-09-03T10:00:00Z",
                        "version": {"name": "v3", "processor": "export default {}", "renderer": "<div/>"}});
    let one = window.clone();
    let (space, seen) = station(move |method, path, _| match (method, path) {
        ("GET", "/api/orgs/tos/windows") => ok(json!([one.clone()])),
        ("GET", "/api/orgs/tos/windows/w1") => ok(one.clone()),
        ("POST", "/api/orgs/tos/windows") => (201, one.to_string()),
        ("PUT", "/api/orgs/tos/windows/w1") => ok(one.clone()),
        ("DELETE", "/api/orgs/tos/windows/w1") => (204, String::new()),
        ("GET", "/api/orgs/tos/windows/w1/versions") => {
            ok(json!([{"id": "v-1", "name": "v3", "processor": "p", "renderer": "r",
                       "created_by": "alice", "created_at": "2026-09-03T10:00:00Z"}]))
        }
        ("POST", "/api/orgs/tos/windows/w1/versions") => {
            (201, json!({"name": "v4", "processor": "p", "renderer": "r"}).to_string())
        }
        ("GET", "/api/orgs/tos/windows/w1/state") => {
            ok(json!({"json": {"orders": 3}, "metadata": {"processor_version": "v3", "renderer_version": "v3",
                      "produced_at": "2026-09-03T10:00:01Z", "is_live": true}}))
        }
        ("GET", "/api/me") => {
            ok(json!({"id": "alice", "kind": "carbon", "org": "tos", "app": "https://space.example"}))
        }
        _ => error(404, "not_found"),
    });

    assert_eq!(space.windows().unwrap()[0].version.as_ref().unwrap().name, "v3");
    assert_eq!(space.window("w1").unwrap().name, "Orders");
    assert_eq!(space.create_window("Orders", &["@alice"]).unwrap().id, "w1");
    assert_eq!(body(&seen, 2), json!({"name": "Orders", "access": ["@alice"]}));
    space.update_window("w1", Some("Orders 2"), None).unwrap();
    assert_eq!(body(&seen, 3), json!({"name": "Orders 2", "access": null}));
    space.delete_window("w1").unwrap();
    assert_eq!(space.versions("w1").unwrap()[0].created_by.as_deref(), Some("alice"));
    assert_eq!(space.publish("w1", "v4", "export default {}", "<div/>").unwrap().name, "v4");
    let state = space.window_state("w1").unwrap();
    assert_eq!((state.json.unwrap()["orders"].as_u64(), state.metadata.is_live), (Some(3), true));
    assert_eq!(space.window_url("w1").unwrap(), "https://space.example/o/tos/windows/w1");
}

#[test]
fn publishing_code_that_carries_a_secret_never_leaves_the_machine() {
    let token = format!("spacewindow-{}", "0123456789abcdef".repeat(2));
    let (space, seen) = station(|_, _, _| (500, "the pre-flight should have stopped this".into()));

    let code = format!("// {token}\nexport default defineProcessor({{}})");
    let err = space.publish("w1", "v1", &code, "<div/>").unwrap_err().to_string();
    assert!(err.starts_with("the processor contains a secret (spacewindow-"), "{err}");
    assert!(!err.contains(&token), "the token itself is never printed");
    let err = space.publish("w1", "v1", "export default {}", &format!("<!-- {token} -->")).unwrap_err().to_string();
    assert!(err.starts_with("the renderer contains a secret"), "{err}");
    assert!(calls(&seen).is_empty(), "nothing was sent");
    assert!(windows::preflight("renderer", "<div>{{ mission_control.json.x }}</div>").is_ok());
}

#[test]
fn running_a_window_that_is_not_there_is_not_found_before_any_node_is_started() {
    let (space, seen) = station(|method, path, _| match (method, path) {
        ("GET", "/api/orgs/tos/windows/w_x") => error(404, "not_found"),
        _ => ok(json!({"token": "spacewindow-0123456789abcdef0123456789abcdef", "last_used_at": null})),
    });
    let err = space.run_window("w_x", &test_home(), |_| {}).unwrap_err();
    assert!(matches!(err, Error::Api { status: 404, ref code, .. } if code == "not_found"), "{err}");
    assert_eq!(
        calls(&seen),
        [("GET".to_string(), "/api/orgs/tos/windows/w_x".to_string())],
        "no token fetched, no node"
    );
}

#[test]
fn a_runtime_refusal_is_the_api_error_it_relays_and_anything_else_is_the_child_output() {
    let stderr = "{\"status\":{\"is_live\":false}}\n{\"error\":{\"code\":\"invalid_args\",\"message\":\"missing argument \\\"seq\\\"\"}}";
    let err = windows::refusal("order_detail", stderr);
    assert!(
        matches!(err, Error::Api { status: 0, ref code, ref message } if code == "invalid_args" && message == "missing argument \"seq\""),
        "{err:?}"
    );
    assert_eq!(err.to_string(), "invalid_args: missing argument \"seq\"");
    let relayed = windows::refusal("x", "{\"error\":{\"code\":\"forbidden\",\"message\":\"no access\"}}\n");
    assert!(matches!(relayed, Error::Api { code, .. } if code == "forbidden"));
    let trace = windows::refusal("x", "TypeError: boom\n    at run (processor:3)");
    assert!(
        matches!(trace, Error::Local(ref m) if m == "tool x: TypeError: boom\n    at run (processor:3)"),
        "{trace}"
    );
}

#[test]
fn the_embedded_runtime_is_unpacked_once_left_alone_when_unchanged_and_restored_when_tampered() {
    let dir = test_home().join("runtime");
    let path = windows::runtime(&dir).unwrap();
    assert_eq!(path, dir.join("mission-control.js"));
    assert_eq!(fs::read_to_string(&path).unwrap(), windows::RUNTIME);

    let epoch = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options().write(true).open(&path).unwrap().set_modified(epoch).unwrap();
    windows::runtime(&dir).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), epoch, "an unchanged runtime is not rewritten");

    fs::write(&path, "tampered").unwrap();
    windows::runtime(&dir).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), windows::RUNTIME);
}

// ── notifications ───────────────────────────────────────────────────────────────────────────

fn def() -> Def {
    Def {
        name: "new order".into(),
        description: None,
        enabled: true,
        triggers: vec![Trigger::Table { table: "orders".into(), where_: Some("record.price::Float64 > 5".into()) }],
        sql: "SELECT dedup_key, text, metadata FROM orders".into(),
        delay: "2s".into(),
        cooldown: "10m".into(),
        access: vec!["@alice".into()],
    }
}

#[test]
fn notifications_are_created_edited_read_subscribed_tested_and_deleted() {
    let row = json!({"id": "n1", "def": def(), "recipients": ["@alice"], "enabled": true,
                     "created_by": "alice", "created_at": "2026-09-03T10:00:00Z"});
    let one = row.clone();
    let (space, seen) = station(move |method, path, _| match (method, path) {
        ("GET", "/api/orgs/tos/notifications") => ok(json!([one.clone()])),
        ("GET", "/api/orgs/tos/notifications/n1") => ok(one.clone()),
        ("POST", "/api/orgs/tos/notifications") => (201, one.to_string()),
        ("PUT", "/api/orgs/tos/notifications/n1") => ok(one.clone()),
        ("DELETE", "/api/orgs/tos/notifications/n1") => (204, String::new()),
        ("GET", "/api/orgs/tos/notifications/n1/events") => {
            ok(json!([{"id": 7, "dedup_key": "o-42", "text": "new order", "metadata": {"amount": 12.5},
                       "created_at": "2026-09-03T10:00:00Z"}]))
        }
        ("POST", "/api/orgs/tos/notifications/n1/subscribe") => ok(json!({"recipients": ["@alice", "@bot:tos"]})),
        ("DELETE", "/api/orgs/tos/notifications/n1/subscribe") => ok(json!({"recipients": ["@alice"]})),
        ("POST", "/api/orgs/tos/notifications/n1/test") => {
            ok(json!({"rows": [{"dedup_key": "o-42"}], "last_trigger_at": "2026-09-03T09:59:00Z"}))
        }
        _ => error(404, "not_found"),
    });

    assert_eq!(space.notifications().unwrap()[0].def.name, "new order");
    assert_eq!(space.notification("n1").unwrap().recipients, ["@alice"]);
    assert_eq!(space.create_notification(&def(), &["@alice"]).unwrap().id, "n1");
    assert_eq!(body(&seen, 2), json!({"def": def(), "recipients": ["@alice"]}));
    space.update_notification("n1", &def(), None).unwrap();
    assert_eq!(body(&seen, 3), json!({"def": def(), "recipients": null}), "no recipients keeps the subscribers");
    space.delete_notification("n1").unwrap();
    assert_eq!(space.events("n1").unwrap()[0].dedup_key, "o-42");
    assert_eq!(space.subscribe("n1").unwrap(), ["@alice", "@bot:tos"]);
    assert_eq!(space.unsubscribe("n1").unwrap(), ["@alice"]);
    let run = space.test_notification("n1").unwrap();
    assert_eq!((run.rows.len(), run.error, run.last_trigger_at.is_some()), (1, None, true));
}

// ── settings ────────────────────────────────────────────────────────────────────────────────

#[test]
fn webhooks_api_keys_and_the_access_token_hand_back_their_secrets_once() {
    let secret = "whsec-0123456789abcdef0123456789abcdef";
    let apikey = "apikey-0123456789abcdef0123456789abcdef";
    let token = "spacewindow-0123456789abcdef0123456789abcdef";
    let (space, seen) = station(move |method, path, _| match (method, path) {
        ("GET", "/api/orgs/tos/webhooks") => {
            ok(json!([{"id": "h1", "url": "https://example.com/hook", "created_by": "alice",
                       "created_at": "2026-09-03T10:00:00Z"}]))
        }
        ("POST", "/api/orgs/tos/webhooks") => {
            (201, json!({"id": "h1", "url": "https://example.com/hook", "secret": secret}).to_string())
        }
        ("DELETE", "/api/orgs/tos/webhooks/h1") => (204, String::new()),
        ("PUT", "/api/orgs/tos/silicon-webhook") => ok(json!({"url": "https://bot.example", "secret": secret})),
        ("DELETE", "/api/orgs/tos/silicon-webhook") => (204, String::new()),
        ("GET", "/api/orgs/tos/api-keys") => ok(json!([{"id": "k1", "scopes": ["tables"], "created_by": "alice",
                       "created_at": "2026-09-03T10:00:00Z", "last_used_at": null}])),
        ("POST", "/api/orgs/tos/api-keys") => (201, json!({"id": "k1", "key": apikey}).to_string()),
        ("DELETE", "/api/orgs/tos/api-keys/k1") => (204, String::new()),
        ("GET", "/api/orgs/tos/access-token") => ok(json!({"token": token, "last_used_at": "2026-09-03T09:00:00Z"})),
        ("POST", "/api/orgs/tos/access-token/rotate") => ok(json!({"token": token, "last_used_at": null})),
        _ => error(404, "not_found"),
    });

    assert_eq!(space.webhooks().unwrap()[0].url, "https://example.com/hook");
    let made = space.create_webhook("https://example.com/hook").unwrap();
    assert_eq!((made.id.as_deref(), made.value.as_str()), (Some("h1"), secret));
    assert_eq!(body(&seen, 1), json!({"url": "https://example.com/hook"}));
    space.delete_webhook("h1").unwrap();
    assert_eq!(space.set_silicon_webhook("https://bot.example").unwrap().value, secret);
    space.delete_silicon_webhook().unwrap();
    assert_eq!(calls(&seen)[4], ("DELETE".to_string(), "/api/orgs/tos/silicon-webhook".to_string()));
    for blank in ["", "  "] {
        let err = space.set_silicon_webhook(blank).unwrap_err();
        assert!(matches!(err, Error::Local(ref m) if m.contains("URL is required")), "{err}");
        let err = space.create_webhook(blank).unwrap_err();
        assert!(matches!(err, Error::Local(ref m) if m.contains("URL is required")), "{err}");
    }
    assert_eq!(calls(&seen).len(), 5, "a blank URL never leaves the machine");
    assert_eq!(space.api_keys().unwrap()[0].scopes, ["tables"]);
    assert_eq!(space.create_api_key(&["tables", "notifications"]).unwrap().value, apikey);
    assert_eq!(body(&seen, 6), json!({"scopes": ["tables", "notifications"]}));
    space.delete_api_key("k1").unwrap();
    assert_eq!(space.access_token().unwrap().last_used_at.as_deref(), Some("2026-09-03T09:00:00Z"));
    assert_eq!(space.rotate_access_token().unwrap().token, token);
}

// ── data ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_query_carries_its_restriction_and_comes_back_with_rows_and_watermarks() {
    let (space, seen) = station(|method, path, _| match (method, path) {
        ("POST", "/api/orgs/tos/query") => {
            ok(json!({"rows": [{"n": "3", "cursor": "131"}], "watermarks": {"orders": 131}}))
        }
        ("GET", "/api/orgs/tos/dev-errors") => {
            ok(json!([{"id": 1, "source": "notifications", "ref": "n1", "message": "boom",
                       "detail": {}, "created_at": "2026-09-03T10:00:00Z"}]))
        }
        _ => error(404, "not_found"),
    });

    let mut restrict = crate::Restrict::new();
    restrict.insert("orders".into(), crate::Bounds { from: Some(130), to: None });
    let rows: Rows = space.query("SELECT count() FROM orders", &restrict).unwrap();
    assert_eq!((rows.rows[0]["n"].as_str(), rows.watermarks["orders"]), (Some("3"), 131));
    assert_eq!(body(&seen, 0), json!({"sql": "SELECT count() FROM orders", "restrict": {"orders": {"from": 130}}}));
    assert!(space.query("SELECT 1", &Default::default()).is_ok());
    assert_eq!(body(&seen, 1)["restrict"], json!({}), "no restriction means the server picks the bounds");
    let errors = space.dev_errors().unwrap();
    assert_eq!((errors[0].source.as_str(), errors[0].reference.as_str()), ("notifications", "n1"));
}

#[test]
fn whoami_reads_the_org_route_and_the_app_and_the_orgs_come_from_the_session_alone() {
    let (space, seen) = station(|method, path, _| match (method, path) {
        ("GET", "/api/orgs/tos/me") => ok(json!({"kind": "silicon", "id": "bot:tos", "org": "tos", "tags": ["ops"]})),
        ("GET", "/api/me") => {
            ok(json!({"id": "bot:tos", "kind": "silicon", "org": "tos", "app": "https://space.example"}))
        }
        ("GET", "/api/orgs") => ok(json!([{"id": "tos", "name": "Team of Silicons"}, {"id": "acme"}])),
        _ => error(404, "not_found"),
    });
    let me = space.me().unwrap();
    assert_eq!((me.kind, me.id.as_str(), me.org.as_str()), (Kind::Silicon, "bot:tos", "tos"));
    assert_eq!(me.tags, ["ops"]);
    assert_eq!(space.app_url().unwrap(), "https://space.example");
    assert_eq!(space.window_url("w1").unwrap(), "https://space.example/o/tos/windows/w1");
    let orgs = space.orgs().unwrap();
    assert_eq!((orgs[0].id.as_str(), orgs[0].name.as_deref()), ("tos", Some("Team of Silicons")));
    assert_eq!((orgs[1].id.as_str(), orgs[1].name.as_deref()), ("acme", None), "the mirror may not know a name yet");
    let asked: Vec<String> = calls(&seen).into_iter().map(|(_, p)| p).collect();
    assert_eq!(asked, ["/api/orgs/tos/me", "/api/me", "/api/me", "/api/orgs"], "no ?org= anywhere: a session is bound");
}

// ── the local half ──────────────────────────────────────────────────────────────────────────

#[test]
fn daemon_status_reports_the_socket_and_what_the_spool_still_owes_in_the_home_it_is_given() {
    let home = test_home();
    let empty = crate::daemon::status(&home).unwrap();
    assert_eq!((empty.running, empty.unacked, empty.home.as_path()), (false, 0, home.as_path()));

    let lines: String = (1..=3).map(|seq| format!("{{\"seq\":{seq},\"key\":\"k\",\"record\":{{}}}}\n")).collect();
    fs::write(home.join("spool.jsonl"), lines).unwrap();
    fs::write(home.join("spool.cursor"), "1").unwrap();
    let listener = UnixListener::bind(home.join("daemon.sock")).unwrap();
    let status = crate::daemon::status(&home).unwrap();
    assert_eq!((status.running, status.unacked), (true, 2), "seq 2 and 3 are still waiting");
    assert_eq!(status.socket, home.join("daemon.sock"), "a short home keeps its socket at home");
    drop(listener);
    assert!(!crate::daemon::status(&home).unwrap().running, "nobody is listening any more");
}
