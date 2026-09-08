//! The binary as a person runs it: a real process, a real HTTP responder on 127.0.0.1, a real
//! `SPACE_STATION_HOME`. What is asserted is wiring, presentation and state — which route a
//! command asks for, which stream each half of an answer lands on, what `auth.json` holds after
//! each credential command, and that every capability the package has can be reached from the
//! tree.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{env, fs, process, thread};

use serde_json::{Value, json};

/// The binary this test just built.
const BIN: &str = env!("CARGO_BIN_EXE_spacestation");
/// The package's own source, read to find the methods a command must exist for.
const API: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../client/src/api.rs");

/// `Space`'s constructor and accessors: the way to reach a capability, not one themselves.
const PLUMBING: &[&str] = &["new", "org", "url", "scope"];

/// Every capability of the package, and the command that reaches it. `Space`'s half is scanned
/// out of its source, so a method added there and forgotten here fails the test below; `login`,
/// `exchange`, `Auth` and `daemon` are named by hand, because those modules also carry plumbing
/// the tree has no business exposing (`describe`, `Config`).
const COVERAGE: &[(&str, &[&str])] = &[
    ("orgs", &["orgs"]),
    ("me", &["whoami"]),
    ("app_url", &["login"]),
    ("tables", &["tables", "ls"]),
    ("table", &["tables", "get"]),
    ("create_table", &["tables", "create"]),
    ("set_table_access", &["tables", "access"]),
    ("rotate_table_key", &["tables", "rotate"]),
    ("delete_table", &["tables", "rm"]),
    ("overview", &["tables", "overview"]),
    ("windows", &["windows", "ls"]),
    ("window", &["windows", "get"]),
    ("create_window", &["windows", "create"]),
    ("update_window", &["windows", "edit"]),
    ("delete_window", &["windows", "rm"]),
    ("versions", &["windows", "versions"]),
    ("publish", &["windows", "publish"]),
    ("window_state", &["windows", "json"]),
    ("run_window", &["windows", "run"]),
    ("window_tool", &["windows", "tool"]),
    ("window_url", &["windows", "open"]),
    ("notifications", &["notifications", "ls"]),
    ("notification", &["notifications", "get"]),
    ("create_notification", &["notifications", "create"]),
    ("update_notification", &["notifications", "edit"]),
    ("delete_notification", &["notifications", "rm"]),
    ("events", &["notifications", "events"]),
    ("subscribe", &["notifications", "subscribe"]),
    ("unsubscribe", &["notifications", "unsubscribe"]),
    ("test_notification", &["notifications", "test"]),
    ("webhooks", &["webhooks", "ls"]),
    ("create_webhook", &["webhooks", "create"]),
    ("delete_webhook", &["webhooks", "rm"]),
    ("set_silicon_webhook", &["webhook", "set"]),
    ("delete_silicon_webhook", &["webhook", "rm"]),
    ("api_keys", &["keys", "ls"]),
    ("create_api_key", &["keys", "create"]),
    ("delete_api_key", &["keys", "rm"]),
    ("access_token", &["token", "show"]),
    ("rotate_access_token", &["token", "rotate"]),
    ("query", &["query"]),
    ("dev_errors", &["errors"]),
    ("logout", &["logout"]),
    ("space_station::login", &["login"]),
    ("space_station::exchange", &["auth"]),
    ("Auth::session", &["login"]),
    ("Auth::api_key", &["--api-key", "apikey-x", "whoami"]),
    ("Auth::access_token", &["--access-token", "spacewindow-x", "whoami"]),
    ("SpaceClient::record", &["record"]),
    ("daemon::run", &["daemon", "run"]),
    ("daemon::status", &["daemon", "status"]),
];

/// One HTTP/1.1 responder per test: `handler(method, path, lowercased headers, body)` answers
/// `(status, body)`; a 302 sends the body as `Location`, the way the backend hands a terminal
/// its session.
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
                let (extra, out) = match status {
                    302 => (format!("Location: {out}\r\n"), String::new()),
                    _ => ("Content-Type: application/json\r\n".to_string(), out),
                };
                let head = format!("HTTP/1.1 {status} X\r\n{extra}Connection: close\r\n");
                let _ = write!(stream, "{head}Content-Length: {}\r\n\r\n{out}", out.len());
                // Closing with bytes still unread is a reset, and a reset reaches the client as a
                // failure instead of the answer it already has. A body-less POST is sent chunked,
                // so its `0\r\n\r\n` is exactly such a leftover: read to the end before letting go.
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let _ = stream.read_to_end(&mut Vec::new());
            });
        }
    });
    url
}

/// A fresh `SPACE_STATION_HOME` for one test.
fn home(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("space-station-cli-{name}-{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stored session, as `login` or `auth` leaves one, bound to `org` when there is one.
fn signed_in(home: &Path, org: Option<&str>) {
    let stored = match org {
        Some(org) => json!({"bearer": "sscli-test", "org": org}),
        None => json!({"bearer": "sscli-test"}),
    };
    fs::write(home.join("auth.json"), stored.to_string()).unwrap();
}

struct Ran {
    out: String,
    err: String,
    ok: bool,
}

/// The binary, with this home and this Space Station and nothing else of the ambient
/// environment that could scope it.
fn run(home: &Path, url: &str, env: &[(&str, &str)], args: &[&str]) -> Ran {
    let done = Command::new(BIN)
        .args(args)
        .env_remove("SPACE_STATION_ORG")
        .env_remove("SPACE_STATION_API_KEY")
        .env_remove("SPACE_STATION_ACCESS_TOKEN")
        .env_remove("SPACE_STATION_TOKEN")
        .env("SPACE_STATION_HOME", home)
        .env("SPACE_STATION_URL", url)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    Ran {
        out: String::from_utf8_lossy(&done.stdout).into_owned(),
        err: String::from_utf8_lossy(&done.stderr).into_owned(),
        ok: done.status.success(),
    }
}

/// A responder that remembers `METHOD /path body` and answers from `routes`.
fn station(routes: Vec<(&'static str, Value)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let url = serve(move |method, path, _, body| {
        log.lock().unwrap().push(format!("{method} {path} {body}").trim_end().to_string());
        match routes.iter().find(|(route, _)| *route == format!("{method} {path}")) {
            Some((_, answer)) => (200, answer.to_string()),
            None => {
                (404, json!({"error": {"code": "not_found", "message": format!("no {method} {path}")}}).to_string())
            }
        }
    });
    (url, seen)
}

fn calls(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock().unwrap().clone()
}

fn auth_file(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(home.join("auth.json")).unwrap()).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ── the org, and the credential ─────────────────────────────────────────────────────────────

#[test]
fn the_org_is_the_flag_then_the_environment_then_the_one_stored() {
    let (url, seen) = station(vec![]);
    let home = home("org");
    signed_in(&home, Some("cli-stored"));

    run(&home, &url, &[], &["tables", "ls"]);
    run(&home, &url, &[("SPACE_STATION_ORG", "cli-env")], &["tables", "ls"]);
    run(&home, &url, &[("SPACE_STATION_ORG", "cli-env")], &["--org", "cli-flag", "tables", "ls"]);

    let asked: Vec<String> =
        calls(&seen).iter().map(|c| c.replace("GET /api/orgs/", "").replace("/tables", "")).collect();
    assert_eq!(asked, ["cli-stored", "cli-env", "cli-flag"]);
}

#[test]
fn without_an_org_the_error_names_every_way_to_set_one_and_nothing_is_asked_for() {
    let (url, seen) = station(vec![]);
    let home = home("no-org");
    signed_in(&home, None);

    let ran = run(&home, &url, &[], &["tables", "ls"]);
    assert!(!ran.ok && ran.out.is_empty(), "{ran:?}", ran = (ran.ok, ran.out));
    assert!(ran.err.starts_with("error: local: no org: "), "{}", ran.err);
    for way in ["--org", "$SPACE_STATION_ORG", "spacestation use <org>"] {
        assert!(ran.err.contains(way), "{way} is a way to set an org: {}", ran.err);
    }
    assert!(calls(&seen).is_empty(), "nothing needed to leave the machine to know this");
}

#[test]
fn an_api_key_or_an_access_token_acts_instead_of_the_stored_credential_and_names_its_own_org() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let url = serve(move |_, _, headers, _| {
        let bearer = headers.lines().find(|l| l.starts_with("authorization: ")).unwrap_or_default();
        log.lock().unwrap().push(bearer.to_string());
        (200, "[]".into())
    });
    let home = home("api-key");
    signed_in(&home, Some("cli-org"));

    run(&home, &url, &[], &["tables", "ls"]);
    run(&home, &url, &[], &["--api-key", "apikey-cli", "--org", "cli-org", "tables", "ls"]);
    run(&home, &url, &[("SPACE_STATION_API_KEY", "apikey-env"), ("SPACE_STATION_ORG", "cli-org")], &["tables", "ls"]);
    run(&home, &url, &[], &["--access-token", "spacewindow-cli", "--org", "cli-org", "tables", "ls"]);
    assert_eq!(
        calls(&seen),
        [
            "authorization: bearer sscli-test",
            "authorization: bearer apikey-cli",
            "authorization: bearer apikey-env",
            "authorization: bearer spacewindow-cli",
        ]
    );

    let alone = run(&home, &url, &[], &["--api-key", "apikey-cli", "tables", "ls"]);
    assert!(
        !alone.ok && alone.err.contains("no org"),
        "a credential that is not stored has no stored org: {alone:?}",
        alone = (alone.ok, alone.err)
    );
}

#[test]
fn an_empty_credential_or_org_from_a_flag_or_the_environment_is_none_and_the_stored_session_acts() {
    // `set -a; . ./.env; set +a` on the repo's own `.env` exports `SPACE_STATION_ACCESS_TOKEN=`
    // blank; that must not sign the terminal out with an empty bearer.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let url = serve(move |_, path, headers, _| {
        let bearer = headers.lines().find(|l| l.starts_with("authorization: ")).unwrap_or_default();
        log.lock().unwrap().push(format!("{bearer} {path}"));
        (200, "[]".into())
    });
    let home = home("empty-credential");
    signed_in(&home, Some("cli-org"));

    for (env, args) in [
        (&[("SPACE_STATION_ACCESS_TOKEN", "")][..], &["tables", "ls"][..]),
        (&[("SPACE_STATION_API_KEY", "")], &["tables", "ls"]),
        (&[("SPACE_STATION_API_KEY", " "), ("SPACE_STATION_ACCESS_TOKEN", "")], &["tables", "ls"]),
        (&[], &["--access-token", "", "tables", "ls"]),
        (&[], &["--api-key", "", "tables", "ls"]),
        (&[("SPACE_STATION_ORG", "")], &["tables", "ls"]),
    ] {
        let ran = run(&home, &url, env, args);
        assert!(ran.ok, "{env:?} {args:?}: {}", ran.err);
    }
    let expected = vec!["authorization: bearer sscli-test /api/orgs/cli-org/tables"; 6];
    assert_eq!(calls(&seen), expected, "the stored session and its org act every time");
}

#[test]
fn a_401_from_the_backend_prints_the_way_in_and_exits_1_whatever_its_code() {
    let url = serve(|_, path, _, _| match path {
        "/api/orgs/tos/tables" => (401, json!({"error": {"code": "session_expired", "message": "gone"}}).to_string()),
        "/api/orgs/tos/windows" => (401, json!({"error": {"code": "brand_new_code", "message": "no"}}).to_string()),
        _ => (403, json!({"error": {"code": "forbidden", "message": "no access"}}).to_string()),
    });
    let home = home("401");
    signed_in(&home, Some("tos"));

    let refused = run(&home, &url, &[], &["tables", "ls"]);
    assert!(!refused.ok && refused.out.is_empty(), "exit 1, nothing on stdout: {}", refused.out);
    assert!(refused.err.starts_with("error: session_expired: gone\n"), "{}", refused.err);
    assert!(refused.err.contains("spacestation login"), "{}", refused.err);
    assert!(refused.err.contains("spacestation auth <slt>"), "{}", refused.err);
    assert_eq!(auth_file(&home), json!({"org": "tos"}), "an expired session is forgotten; its org is kept");
    signed_in(&home, Some("tos"));
    let novel = run(&home, &url, &[], &["windows", "ls"]);
    assert!(novel.err.contains("spacestation login"), "the status, not the code, is what says to sign in");
    assert_eq!(auth_file(&home), json!({"org": "tos"}));
    signed_in(&home, Some("tos"));
    let denied = run(&home, &url, &[], &["keys", "ls"]);
    assert!(!denied.ok && denied.err.trim() == "error: forbidden: no access", "a 403 is about access: {}", denied.err);
    assert_eq!(auth_file(&home)["bearer"], "sscli-test", "a 403 keeps the credential");
}

// ── how an answer is printed ────────────────────────────────────────────────────────────────

#[test]
fn a_secret_is_alone_on_stdout_and_its_note_is_only_on_stderr() {
    let key = "table-orders-0123456789abcdef0123456789abcdef";
    let (url, _) =
        station(vec![("POST /api/orgs/tos/tables", json!({"key": "table-orders-0123456789abcdef0123456789abcdef"}))]);
    let home = home("secret");
    signed_in(&home, Some("tos"));

    let ran = run(&home, &url, &[], &["tables", "create", "orders", "--access", "@alice,tech"]);
    assert!(ran.ok, "{}", ran.err);
    assert_eq!(ran.out, format!("{key}\n"), "stdout is the secret and nothing else");
    assert!(ran.err.contains("the table key, shown once; treat it like a password"), "{}", ran.err);
    assert!(!ran.err.contains(key), "the secret never reaches stderr: {}", ran.err);
}

#[test]
fn a_list_is_columns_and_json_only_when_asked_and_that_json_is_one_line_in_a_pipe() {
    let rows = json!([{"id": "orders", "records": 12, "watermark": 130, "access": ["@a", "tech"],
                       "created_by": "alice", "created_at": "2026-01-01T00:00:00Z"}]);
    let (url, _) = station(vec![("GET /api/orgs/tos/tables", rows)]);
    let home = home("columns");
    signed_in(&home, Some("tos"));

    let columns = run(&home, &url, &[], &["tables", "ls"]);
    assert_eq!(columns.out.lines().next().unwrap(), "ID      RECORDS  WATERMARK  CREATED_BY  ACCESS");
    assert_eq!(columns.out.lines().nth(1).unwrap(), "orders  12       130        alice       @a,tech");

    let scripted = run(&home, &url, &[], &["tables", "ls", "--json"]);
    assert_eq!(scripted.out.lines().count(), 1, "a pipe gets one compact line: {}", scripted.out);
    assert_eq!(serde_json::from_str::<Value>(&scripted.out).unwrap()[0]["id"], "orders");
}

// ── one end-to-end per command group ────────────────────────────────────────────────────────

#[test]
fn the_tables_group_reaches_every_table_route() {
    let (url, seen) = station(vec![
        (
            "GET /api/orgs/tos/tables",
            json!([{"id": "orders", "records": 1, "watermark": 1, "access": [],
                                             "created_by": "alice", "created_at": "t"}]),
        ),
        (
            "PUT /api/orgs/tos/tables/orders",
            json!({"id": "orders", "records": 1, "watermark": 1,
                                                   "access": ["@bob"], "created_by": "alice", "created_at": "t"}),
        ),
        ("POST /api/orgs/tos/tables/orders/rotate-key", json!({"key": "table-orders-1"})),
        ("DELETE /api/orgs/tos/tables/orders", Value::Null),
        (
            "GET /api/orgs/tos/tables/overview?window=1h",
            json!({"tables": 1, "records": 9, "top": [], "avg_lag_ms": 4.5}),
        ),
    ]);
    let home = home("tables");
    signed_in(&home, Some("tos"));
    let ran = |args: &[&str]| {
        let ran = run(&home, &url, &[], args);
        assert!(ran.ok, "spacestation {}: {}", args.join(" "), ran.err);
        ran
    };

    assert!(ran(&["tables", "get", "orders"]).out.contains("\"orders\""));
    assert!(ran(&["tables", "access", "orders", "--access", "@bob"]).out.contains("@bob"));
    assert_eq!(ran(&["tables", "rotate", "orders"]).out, "table-orders-1\n");
    assert!(ran(&["tables", "rm", "orders"]).err.contains("table orders deleted"));
    assert!(ran(&["tables", "overview", "--window", "1h"]).out.contains("\"avg_lag_ms\":4.5"));

    assert_eq!(
        calls(&seen),
        [
            "GET /api/orgs/tos/tables",
            "PUT /api/orgs/tos/tables/orders {\"access\":[\"@bob\"]}",
            "POST /api/orgs/tos/tables/orders/rotate-key",
            "DELETE /api/orgs/tos/tables/orders",
            "GET /api/orgs/tos/tables/overview?window=1h",
        ]
    );
}

#[test]
fn the_windows_group_reaches_every_window_route_and_hands_over_the_page_that_draws_it() {
    let window = json!({"id": "w_01", "name": "Orders", "access": [], "created_by": "alice",
                        "created_at": "t", "version": null});
    let (url, seen) = station(vec![
        ("GET /api/me", json!({"id": "alice", "kind": "carbon", "org": "tos", "app": "https://app.example"})),
        ("GET /api/orgs/tos/windows", json!([window.clone()])),
        ("GET /api/orgs/tos/windows/w_01", window.clone()),
        ("POST /api/orgs/tos/windows", window.clone()),
        ("PUT /api/orgs/tos/windows/w_01", window.clone()),
        ("DELETE /api/orgs/tos/windows/w_01", Value::Null),
        ("GET /api/orgs/tos/windows/w_01/versions", json!([])),
        ("POST /api/orgs/tos/windows/w_01/versions", json!({"name": "v1", "processor": "p", "renderer": "r"})),
        (
            "GET /api/orgs/tos/windows/w_01/state",
            json!({"json": {"a": 1}, "metadata": {"processor_version": "v1",
            "renderer_version": "v1", "produced_at": "t", "is_live": true}}),
        ),
    ]);
    let home = home("windows");
    signed_in(&home, Some("tos"));
    let (processor, renderer) = (home.join("processor.js"), home.join("renderer.html"));
    fs::write(&processor, "export default defineProcessor({})").unwrap();
    fs::write(&renderer, "<div id=app></div>").unwrap();
    let ran = |args: &[&str]| {
        let ran = run(&home, &url, &[], args);
        assert!(ran.ok, "spacestation {}: {}", args.join(" "), ran.err);
        ran
    };

    let list = ran(&["windows", "ls"]);
    assert_eq!(list.out.lines().nth(1).unwrap(), "w_01  Orders  -        alice");

    let one = ran(&["windows", "get", "w_01"]);
    assert!(
        one.out.contains("\"w_01\"") && one.err == "view: https://app.example/o/tos/windows/w_01\n",
        "{one:?}",
        one = (one.out, one.err)
    );
    assert!(ran(&["windows", "create", "Orders"]).err.contains("view: https://app.example/o/tos/windows/w_01"));
    assert!(ran(&["windows", "edit", "w_01", "--name", "Sales"]).out.contains("w_01"));
    assert!(ran(&["windows", "rm", "w_01"]).err.contains("window w_01 deleted"));
    assert_eq!(ran(&["windows", "versions", "w_01"]).out, "[]\n");
    assert!(
        ran(&[
            "windows",
            "publish",
            "w_01",
            "--name",
            "v1",
            "--processor",
            processor.to_str().unwrap(),
            "--renderer",
            renderer.to_str().unwrap()
        ])
        .out
        .contains("\"v1\"")
    );
    assert!(ran(&["windows", "json", "w_01"]).out.contains("\"is_live\":true"));

    let opened = Command::new(BIN)
        .args(["windows", "open", "w_01"])
        .env("SPACE_STATION_HOME", &home)
        .env("SPACE_STATION_URL", &url)
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&opened.stdout), "https://app.example/o/tos/windows/w_01\n");
    assert!(String::from_utf8_lossy(&opened.stderr).contains("no browser"), "there is no opener on an empty PATH");

    let routes: Vec<String> = calls(&seen).into_iter().filter(|c| !c.starts_with("GET /api/me")).collect();
    assert_eq!(routes.first().unwrap(), "GET /api/orgs/tos/windows");
    assert_eq!(routes.last().unwrap(), "GET /api/orgs/tos/windows/w_01/state");
    assert_eq!(routes.len(), 8, "one call per window command: {routes:?}");
}

#[test]
fn the_notifications_group_sends_the_definition_file_as_its_two_arguments() {
    let notification = json!({"id": "n_01", "def": {"name": "New orders", "enabled": true, "triggers": [],
        "sql": "select 1", "delay": "2s", "cooldown": "10m", "access": []},
        "recipients": ["@alice"], "enabled": true, "created_by": "alice", "created_at": "t"});
    let (url, seen) = station(vec![
        ("GET /api/orgs/tos/notifications", json!([notification.clone()])),
        ("GET /api/orgs/tos/notifications/n_01", notification.clone()),
        ("POST /api/orgs/tos/notifications", notification.clone()),
        ("PUT /api/orgs/tos/notifications/n_01", notification.clone()),
        ("DELETE /api/orgs/tos/notifications/n_01", Value::Null),
        ("GET /api/orgs/tos/notifications/n_01/events", json!([])),
        ("POST /api/orgs/tos/notifications/n_01/subscribe", json!({"recipients": ["@alice"]})),
        ("DELETE /api/orgs/tos/notifications/n_01/subscribe", json!({"recipients": []})),
        ("POST /api/orgs/tos/notifications/n_01/test", json!({"rows": [], "last_trigger_at": null})),
    ]);
    let home = home("notifications");
    signed_in(&home, Some("tos"));
    let file = home.join("new-orders.json");
    let def = json!({"name": "New orders", "triggers": [{"table": "orders"}], "sql": "select 1"});
    fs::write(&file, json!({"def": def, "recipients": ["@alice"]}).to_string()).unwrap();
    let path = file.to_str().unwrap();
    let ran = |args: &[&str]| {
        let ran = run(&home, &url, &[], args);
        assert!(ran.ok, "spacestation {}: {}", args.join(" "), ran.err);
        ran
    };

    assert_eq!(
        ran(&["notifications", "ls"]).out.lines().next().unwrap(),
        "ID    NAME        ENABLED  RECIPIENTS  CREATED_BY"
    );
    assert!(ran(&["notifications", "get", "n_01"]).out.contains("New orders"));
    assert!(ran(&["notifications", "create", path]).out.contains("n_01"));
    assert!(ran(&["notifications", "edit", "n_01", path]).out.contains("n_01"));
    assert!(ran(&["notifications", "rm", "n_01"]).err.contains("notification n_01 deleted"));
    assert_eq!(ran(&["notifications", "events", "n_01"]).out, "[]\n");
    assert_eq!(ran(&["notifications", "subscribe", "n_01"]).out, "[\"@alice\"]\n");
    assert_eq!(ran(&["notifications", "unsubscribe", "n_01"]).out, "[]\n");
    assert!(ran(&["notifications", "test", "n_01"]).out.contains("\"rows\":[]"));

    let created = calls(&seen).into_iter().find(|c| c.starts_with("POST /api/orgs/tos/notifications ")).unwrap();
    let body: Value = serde_json::from_str(created.splitn(3, ' ').nth(2).unwrap()).unwrap();
    assert_eq!(body["recipients"], json!(["@alice"]));
    assert_eq!(body["def"]["delay"], "2s", "the package fills in what the file left out");
    assert_eq!(body["def"]["triggers"], json!([{"table": "orders"}]));
}

#[test]
fn the_webhook_groups_print_each_secret_once_naming_what_it_belongs_to() {
    let (url, seen) = station(vec![
        (
            "GET /api/orgs/tos/webhooks",
            json!([{"id": "wh_1", "url": "https://x.example/h", "created_by": "alice",
                                               "created_at": "t"}]),
        ),
        ("POST /api/orgs/tos/webhooks", json!({"id": "wh_1", "url": "https://x.example/h", "secret": "whsec-1"})),
        ("DELETE /api/orgs/tos/webhooks/wh_1", Value::Null),
        ("PUT /api/orgs/tos/silicon-webhook", json!({"url": "https://bot.example/h", "secret": "whsec-2"})),
    ]);
    let home = home("webhooks");
    signed_in(&home, Some("tos"));
    let ran = |args: &[&str]| {
        let ran = run(&home, &url, &[], args);
        assert!(ran.ok, "spacestation {}: {}", args.join(" "), ran.err);
        ran
    };

    assert_eq!(
        ran(&["webhooks", "ls"]).out.lines().next().unwrap(),
        "ID    URL                  CREATED_BY  CREATED_AT"
    );
    let created = ran(&["webhooks", "create", "https://x.example/h"]);
    assert_eq!(created.out, "whsec-1\n");
    assert!(created.err.contains("the webhook signing secret for wh_1, shown once"), "{}", created.err);
    assert!(ran(&["webhooks", "rm", "wh_1"]).err.contains("webhook wh_1 deleted"));
    assert_eq!(ran(&["webhook", "set", "https://bot.example/h"]).out, "whsec-2\n");
    assert_eq!(calls(&seen).len(), 4);
}

#[test]
fn the_keys_and_token_groups_print_the_key_the_server_shows_once_and_the_token_it_can_show_again() {
    let (url, seen) = station(vec![
        (
            "GET /api/orgs/tos/api-keys",
            json!([{"id": "k_1", "scopes": ["tables"], "created_by": "alice",
                                               "created_at": "t", "last_used_at": null}]),
        ),
        ("POST /api/orgs/tos/api-keys", json!({"id": "k_1", "key": "apikey-1"})),
        ("DELETE /api/orgs/tos/api-keys/k_1", Value::Null),
        ("GET /api/orgs/tos/access-token", json!({"token": "spacewindow-1", "last_used_at": "2026-01-01T00:00:00Z"})),
        ("POST /api/orgs/tos/access-token/rotate", json!({"token": "spacewindow-2", "last_used_at": null})),
    ]);
    let home = home("keys");
    signed_in(&home, Some("tos"));
    let ran = |args: &[&str]| {
        let ran = run(&home, &url, &[], args);
        assert!(ran.ok, "spacestation {}: {}", args.join(" "), ran.err);
        ran
    };

    assert_eq!(ran(&["keys", "ls"]).out.lines().nth(1).unwrap(), "k_1  tables  alice       -");
    let created = ran(&["keys", "create", "--scopes", "tables,notifications"]);
    assert_eq!(created.out, "apikey-1\n");
    assert!(created.err.contains("the api key for k_1, shown once"), "{}", created.err);
    assert!(ran(&["keys", "rm", "k_1"]).err.contains("api key k_1 deleted"));

    let shown = ran(&["token", "show"]);
    assert_eq!(shown.out, "spacewindow-1\n");
    assert!(shown.err.contains("last used 2026-01-01T00:00:00Z"), "{}", shown.err);
    assert_eq!(ran(&["token", "rotate"]).out, "spacewindow-2\n");

    let body = calls(&seen).into_iter().find(|c| c.starts_with("POST /api/orgs/tos/api-keys ")).unwrap();
    assert!(body.ends_with(r#"{"scopes":["tables","notifications"]}"#), "{body}");
}

#[test]
fn the_query_and_errors_groups_print_what_the_server_answered() {
    let (url, seen) = station(vec![
        ("POST /api/orgs/tos/query", json!({"rows": [{"n": 1}], "watermarks": {"orders": 130}})),
        (
            "GET /api/orgs/tos/dev-errors",
            json!([{"id": 1, "source": "notification", "ref": "n_01",
                                                 "message": "bad row", "detail": {}, "created_at": "t"}]),
        ),
    ]);
    let home = home("query");
    signed_in(&home, Some("tos"));

    let rows = run(&home, &url, &[], &["query", "select count() from orders"]);
    assert_eq!(rows.out, "{\"rows\":[{\"n\":1}],\"watermarks\":{\"orders\":130}}\n");
    assert!(run(&home, &url, &[], &["errors"]).out.contains("bad row"));
    assert_eq!(calls(&seen)[0], "POST /api/orgs/tos/query {\"restrict\":{},\"sql\":\"select count() from orders\"}");
}

#[test]
fn whoami_orgs_use_and_logout_are_the_credential_group_and_logout_ends_the_session() {
    let (url, seen) = station(vec![
        ("GET /api/orgs/tos/me", json!({"kind": "carbon", "id": "alice", "org": "tos", "tags": ["tech"]})),
        ("GET /api/orgs", json!([{"id": "tos", "name": "Team of Silicons"}, {"id": "acme"}])),
        ("POST /api/auth/logout", Value::Null),
    ]);
    let home = home("who");
    signed_in(&home, Some("tos"));

    let who = run(&home, &url, &[], &["whoami"]);
    assert_eq!(who.out.trim(), r#"{"id":"alice","kind":"carbon","org":"tos","tags":["tech"]}"#);
    assert!(who.err.contains("stored: a session"), "what is stored is named, never shown: {}", who.err);
    assert_eq!(run(&home, &url, &[], &["orgs"]).out, "ID    NAME\ntos   Team of Silicons\nacme  -\n");

    let used = run(&home, &url, &[], &["use", "other"]);
    assert!(used.err.contains("working in other"), "{}", used.err);
    assert_eq!(auth_file(&home)["org"], "other", "`use` is what the next command reads");
    assert_eq!(auth_file(&home)["bearer"], "sscli-test", "and it keeps the credential");

    let out = run(&home, &url, &[], &["logout"]);
    assert!(out.ok && out.err.contains("the stored credential is gone"), "{}", out.err);
    assert!(!home.join("auth.json").exists(), "logout forgets the credential");
    assert!(!run(&home, &url, &[], &["whoami"]).ok, "and then there is nobody to be");
    assert_eq!(calls(&seen), ["GET /api/orgs/tos/me", "GET /api/orgs", "POST /api/auth/logout"]);

    fs::write(home.join("auth.json"), json!({"bearer": "apikey-x"}).to_string()).unwrap();
    assert!(run(&home, &url, &[], &["logout"]).ok);
    assert_eq!(calls(&seen).len(), 3, "a key has no session to end, so nothing is sent");
    assert!(!home.join("auth.json").exists());
}

#[test]
fn whoami_offline_still_names_what_is_stored() {
    let home = home("offline");
    signed_in(&home, Some("tos"));
    let who = run(&home, "http://127.0.0.1:1", &[], &["whoami"]);
    assert!(!who.ok && who.out.is_empty());
    assert!(who.err.contains("stored: a session"), "{}", who.err);
    assert!(who.err.contains("error: transport:"), "{}", who.err);
}

// ── signing in ──────────────────────────────────────────────────────────────────────────────

/// A `PATH` whose `open` (or `xdg-open`) is the browser: it fetches the link and follows the
/// backend's redirect into the terminal's loopback listener, in the background, like a browser.
fn browser_on_path(home: &Path) -> String {
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["open", "xdg-open"] {
        let script = bin.join(name);
        fs::write(&script, "#!/bin/sh\ncurl -sL \"$1\" >/dev/null 2>&1 &\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    format!("{}:{}", bin.display(), env::var("PATH").unwrap_or_default())
}

#[test]
fn login_opens_the_browser_binds_the_org_and_exchanges_the_short_lived_redirect() {
    let orgs = Arc::new(Mutex::new(Vec::new()));
    let bound = orgs.clone();
    let backend = serve(move |method, path, headers, body| {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        if route == "/api/me" {
            assert!(headers.lines().any(|l| l == "authorization: bearer sscli-fresh"), "{headers}");
            return (200, json!({"id": "alice", "kind": "carbon", "app": "https://app.example"}).to_string());
        }
        if route == "/api/auth/session" {
            assert_eq!(method, "POST");
            let body: Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["slt"], "oac_fresh");
            assert!(!headers.contains("authorization:"), "the exchange carries no existing credential");
            return (200, json!({"token": "sscli-fresh"}).to_string());
        }
        assert_eq!((method, route), ("GET", "/api/auth/login"));
        let param = |name: &str| query.split('&').find_map(|p| p.strip_prefix(&format!("{name}=")).map(String::from));
        bound.lock().unwrap().push(param("org").expect("the link names the org the session is bound to"));
        let port: u16 = param("cli").unwrap().parse().expect("cli is a bare loopback port");
        let state = param("state").unwrap();
        (302, format!("http://127.0.0.1:{port}/?slt=oac_fresh&state={state}"))
    });
    let home = home("login");
    let path = browser_on_path(&home);

    let orgless = run(&home, &backend, &[("PATH", &path)], &["login"]);
    assert!(!orgless.ok && orgless.err.contains("no org"), "nothing stored, no --org: {}", orgless.err);
    assert!(orgs.lock().unwrap().is_empty(), "no link was handed out");

    let ran = run(&home, &backend, &[("PATH", &path)], &["--org", "tos", "login"]);
    assert!(ran.ok, "{}", ran.err);
    assert!(ran.err.contains("signed in to tos: a session"), "{}", ran.err);
    assert!(ran.err.contains("the app: https://app.example"), "{}", ran.err);
    assert!(!ran.err.contains("sscli-fresh"), "the session is never printed: {}", ran.err);
    assert_eq!(auth_file(&home), json!({"bearer": "sscli-fresh", "org": "tos"}), "the org the session is bound to");
    assert_eq!(mode(&home.join("auth.json")), 0o600);

    let again = run(&home, &backend, &[("PATH", &path)], &["login"]);
    assert!(again.ok, "{}", again.err);
    assert_eq!(auth_file(&home)["org"], "tos", "a login without --org binds to the stored org");
    let switched = run(&home, &backend, &[("PATH", &path), ("SPACE_STATION_ORG", "acme")], &["login"]);
    assert!(switched.ok, "{}", switched.err);
    assert_eq!(auth_file(&home)["org"], "acme", "switching orgs is another login");
    assert_eq!(*orgs.lock().unwrap(), ["tos", "tos", "acme"]);
}

#[test]
fn auth_takes_a_short_lived_token_from_the_argument_stdin_or_the_environment_and_never_prompts() {
    let (url, seen) = station(vec![
        ("POST /api/auth/session", json!({"token": "sscli-minted"})),
        ("GET /api/me", json!({"id": "bot:tos", "kind": "silicon", "org": "tos", "app": "https://app.example"})),
    ]);
    let home = home("auth");

    let ran = run(&home, &url, &[], &["auth", "slt_arg", "--org", "tos"]);
    assert!(ran.ok, "{}", ran.err);
    assert!(ran.err.contains("signed in to tos: a session"), "{}", ran.err);
    assert!(ran.err.contains("the app: https://app.example"), "{}", ran.err);
    assert!(
        !ran.err.contains("slt_arg") && !ran.err.contains("sscli-minted"),
        "nothing secret is printed: {}",
        ran.err
    );
    assert_eq!(auth_file(&home), json!({"bearer": "sscli-minted", "org": "tos"}), "the session, never the slt");
    assert_eq!(mode(&home.join("auth.json")), 0o600);
    assert_eq!(calls(&seen)[0], r#"POST /api/auth/session {"org":"tos","slt":"slt_arg"}"#);

    let piped = Command::new(BIN)
        .args(["auth", "-"])
        .env("SPACE_STATION_HOME", &home)
        .env("SPACE_STATION_URL", &url)
        .env_remove("SPACE_STATION_ORG")
        .stdin(process::Stdio::piped())
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().unwrap().write_all(b"slt_stdin\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(piped.status.success(), "{}", String::from_utf8_lossy(&piped.stderr));
    assert_eq!(calls(&seen)[2], r#"POST /api/auth/session {"org":"tos","slt":"slt_stdin"}"#, "the stored org is kept");

    let env = run(&home, &url, &[("SPACE_STATION_TOKEN", "slt_env")], &["auth"]);
    assert!(env.ok, "{}", env.err);
    assert_eq!(calls(&seen)[4], r#"POST /api/auth/session {"org":"tos","slt":"slt_env"}"#);

    let nothing = run(&home, &url, &[], &["auth"]);
    assert!(!nothing.ok && nothing.out.is_empty(), "without a token there is nothing to do");
    assert!(nothing.err.contains("SPACE_STATION_TOKEN"), "the fallback is named: {}", nothing.err);
    assert!(!nothing.err.to_lowercase().contains("prompt"), "{}", nothing.err);

    // A silicon's own `stk-` token, the Application's `ask_` secret and an IAM bearer are what a
    // silicon has at hand and must never hand over; each is refused here, named by kind, unsent.
    let silicon_token = format!("stk-{}", "0123456789abcdef".repeat(2));
    let app_secret = format!("ask_{}", "k".repeat(43));
    let bearer = format!("sat_{}", "a".repeat(43));
    for credential in [&silicon_token, &app_secret, &bearer] {
        let wrong = run(&home, &url, &[], &["auth", credential]);
        assert!(!wrong.ok && wrong.out.is_empty(), "exit 1 and nothing on stdout: {}", wrong.out);
        assert!(wrong.err.starts_with("error: local: not a short-lived token"), "{}", wrong.err);
        assert!(wrong.err.contains("never handed over"), "the refusal says why: {}", wrong.err);
        assert!(!wrong.err.contains(&credential[4..]), "a credential is never echoed: {}", wrong.err);
    }
    let from_env = run(&home, &url, &[("SPACE_STATION_TOKEN", &silicon_token)], &["auth"]);
    assert!(!from_env.ok && from_env.err.contains("not a short-lived token"), "from the environment too");
    assert_eq!(calls(&seen).len(), 6, "and none of them was sent");
    assert_eq!(auth_file(&home)["bearer"], "sscli-minted", "nothing changed");

    let help = Command::new(BIN).args(["auth", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("iam login --app-id 'tos>spacestation' --org <org>"), "{help}");
    assert!(help.contains("iam silicon-login --app-id 'tos>spacestation'"), "{help}");
    assert!(help.contains("never prompts"), "{help}");
    for word in ["stk", "sat_", "refresh"] {
        assert!(!help.to_lowercase().contains(word), "{word} has no place in the help: {help}");
    }
}

#[test]
fn the_daemon_group_answers_about_this_machine_without_a_credential() {
    let home = home("daemon");
    let status = run(&home, "http://127.0.0.1:1", &[], &["daemon", "status"]);
    assert!(status.ok, "{}", status.err);
    let status: Value = serde_json::from_str(&status.out).unwrap();
    assert_eq!((&status["running"], &status["unacked"]), (&json!(false), &json!(0)));
    assert_eq!(status["home"], home.to_str().unwrap(), "the home it speaks for is named in the answer");
}

#[test]
fn record_uses_a_table_key_without_a_session_and_refuses_invalid_input_locally() {
    let home = home("record");
    let key = "table-orders-0123456789abcdef0123456789abcdef";
    let missing = run(&home, "http://127.0.0.1:1", &[], &["record", "{}"]);
    assert!(missing.err.contains("SPACE_STATION_TABLE_KEY"), "{}", missing.err);
    for data in ["not json", "[]", "null", "7"] {
        let bad = run(&home, "http://127.0.0.1:1", &[("SPACE_STATION_TABLE_KEY", key)], &["record", data]);
        assert!(!bad.ok && bad.out.is_empty(), "{data}: {}", bad.err);
        assert!(bad.err.contains("record must be"), "{}", bad.err);
        assert!(!bad.err.contains(key), "the key is never printed");
    }
    assert!(!home.join("spool.jsonl").exists(), "invalid input never reaches the daemon");
}

// ── the rule this crate exists under ────────────────────────────────────────────────────────

#[test]
fn every_capability_the_package_has_is_reachable_from_the_tree() {
    let source = fs::read_to_string(API).expect("the package's api.rs is the list of capabilities");
    let methods: Vec<&str> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub fn "))
        .filter_map(|rest| rest.split('(').next())
        .filter(|name| !PLUMBING.contains(name))
        .collect();
    assert!(methods.len() > 30, "found only {} methods; the scan stopped matching the package", methods.len());

    for method in &methods {
        let covered = COVERAGE.iter().any(|(name, _)| name == method);
        assert!(covered, "`Space::{method}` has no command; the CLI may not lack what the package has");
    }
    for (name, args) in COVERAGE {
        let known = name.contains("::") || methods.contains(name);
        assert!(known, "`Space::{name}` is not in the package any more; this row is stale");
        let help = Command::new(BIN).args(*args).arg("--help").output().unwrap();
        assert!(help.status.success(), "`spacestation {}` is not in the tree", args.join(" "));
    }
}
