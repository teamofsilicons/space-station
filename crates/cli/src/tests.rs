//! What this crate owns: presentation — how a list is laid out, how JSON differs between a
//! terminal and a pipe, how a failure reads — and the credential file. What Space Station itself
//! does is tested in the package; what needs the real binary is tested in `tests/cli.rs`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;
use std::{fs, thread};

use serde_json::{Value, json};
use space_station::{Auth, Error};

use crate::store::{self, Stored};
use crate::{WINDOWS, out};

fn api(status: u16, code: &str, message: &str) -> Error {
    Error::Api { status, code: code.into(), message: message.into() }
}

#[test]
fn columns_are_headed_by_their_label_not_their_path_and_a_field_that_is_not_there_reads_dash() {
    let rows = json!([
        {"id": "orders", "records": 12, "access": ["@a", "tech"], "def": {"name": "n"}},
        {"id": "x", "records": null},
    ]);
    let cols: &[out::Col] = &[("ID", "id"), ("RECORDS", "records"), ("ACCESS", "access"), ("NAME", "def.name")];
    let expected = "ID      RECORDS  ACCESS   NAME\norders  12       @a,tech  n\nx       -        -        -\n";
    assert_eq!(out::table(&rows, cols), expected, "a reader sees NAME, never the dotted path it came from");
    assert_eq!(out::table(&json!([]), &[("ID", "id")]), "ID\n", "an empty list is still its header");
}

#[test]
fn a_window_with_no_published_version_shows_a_dash_where_its_version_name_would_be() {
    let rows = json!([{"id": "w_01", "name": "Orders", "version": null, "created_by": "alice", "access": []}]);
    let printed = out::table(&rows, WINDOWS);
    assert_eq!(printed.lines().next().unwrap(), "ID    NAME    VERSION  CREATED_BY  ACCESS");
    assert_eq!(printed.lines().nth(1).unwrap(), "w_01  Orders  -        alice");
}

#[test]
fn json_is_indented_on_a_terminal_and_one_line_in_a_pipe_and_empty_for_nothing() {
    let v = json!({"id": "w_01", "access": ["@a"]});
    assert_eq!(out::render(&v, false), r#"{"access":["@a"],"id":"w_01"}"#);
    assert_eq!(out::render(&v, true), "{\n  \"access\": [\n    \"@a\"\n  ],\n  \"id\": \"w_01\"\n}");
    assert_eq!(out::render(&Value::Null, true), "", "an empty answer prints nothing at all");
}

#[test]
fn a_failure_reads_code_then_message_and_a_401_says_what_it_means_for_the_credential_in_use() {
    assert_eq!(out::fail(&api(404, "not_found", "no table x"), out::SIGN_IN), "error: not_found: no table x");
    assert_eq!(out::fail(&Error::Local("no org".into()), out::SIGN_IN), "error: local: no org");
    assert_eq!(out::code(&Error::Transport("gone".into())), "transport");

    let refused = out::fail(&api(401, "session_expired", "log in again"), out::SIGN_IN);
    assert!(refused.starts_with("error: session_expired: log in again\n"), "{refused}");
    assert!(refused.contains("spacestation login --org <org>"), "{refused}");
    assert!(refused.contains("spacestation auth <slt> --org <org>"), "{refused}");
    let any = out::fail(&api(401, "some_new_code", ""), out::SIGN_IN);
    assert!(any.contains("spacestation login"), "the status says to sign in, whatever the code: {any}");

    let key = out::fail(&api(401, "unauthorized", "no"), out::KEY_REFUSED);
    assert!(!key.contains("spacestation login"), "an api key has no session to sign in to: {key}");
    assert!(key.contains("scopes"), "it says what a key does reach: {key}");
    let token = out::fail(&api(401, "unauthorized", "no"), out::TOKEN_REFUSED);
    assert!(token.contains("rotated"), "an access token stops resolving when rotated: {token}");

    let access = out::fail(&api(403, "forbidden", "login"), out::SIGN_IN);
    assert!(!access.contains("spacestation login"), "a 403 is about access, and prose is never branched on");
}

// ── the credential file ─────────────────────────────────────────────────────────────────────

fn home() -> PathBuf {
    let home = std::env::temp_dir().join(format!("ss-cli-{}", uuid_ish()));
    fs::create_dir_all(&home).unwrap();
    home
}

/// Unique enough for a temp dir, without a dependency.
fn uuid_ish() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("{}-{nanos}", std::process::id())
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn file(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(home.join("auth.json")).unwrap()).unwrap()
}

#[test]
fn update_load_and_forget_are_the_whole_life_of_auth_json_and_it_is_private() {
    let home = home();
    let Err(nobody) = store::load(&home) else { panic!("nothing is stored yet") };
    let nobody = nobody.to_string();
    assert!(nobody.starts_with("not signed in: run `spacestation login --org <org>`"), "{nobody}");

    let session = Auth::session("sscli-x");
    store::update(&home, |none| {
        assert!(none.is_none());
        Ok(Stored { auth: session.clone(), org: Some("tos".into()) })
    })
    .unwrap();
    assert_eq!((mode(&home), mode(&home.join("auth.json"))), (0o700, 0o600));
    assert_eq!(file(&home), json!({"bearer": "sscli-x", "org": "tos"}));
    assert!(!home.join("auth.json.tmp").exists());

    store::update(&home, |s| Ok(Stored { org: Some("acme".into()), ..s.unwrap() })).unwrap();
    let loaded = store::load(&home).unwrap();
    assert_eq!((loaded.auth.describe(), loaded.org.as_deref()), ("a session", Some("acme")));
    assert_eq!(file(&home)["bearer"], "sscli-x", "`use` keeps the credential as it was");

    store::forget(&home).unwrap();
    assert!(!home.join("auth.json").exists());
    store::forget(&home).unwrap();
}

#[test]
fn the_lock_serialises_concurrent_updates_so_none_is_lost() {
    let home = home();
    let inside = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                store::update(&home, |s| {
                    assert_eq!(inside.fetch_add(1, SeqCst), 0, "two updates ran at once");
                    thread::sleep(Duration::from_millis(50));
                    inside.fetch_sub(1, SeqCst);
                    let n: u32 = s.and_then(|s| s.org).map(|o| o.parse().unwrap()).unwrap_or(0);
                    Ok(Stored { auth: Auth::session("sscli-x"), org: Some((n + 1).to_string()) })
                })
                .unwrap()
            });
        }
    });
    assert_eq!(file(&home)["org"], "4", "every update saw the one before it");
    assert_eq!(mode(&home.join("auth.lock")), 0o600);
}
