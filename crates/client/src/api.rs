//! Space Station's HTTP API as one type: every operation is a method, sync, one request and one
//! answer. Paths are `/api` plus the route, scoped to the authenticated account, and
//! `{error: {code, message}}` becomes [`Error::Api`] so callers branch on `code` and never on
//! prose. The backend refreshes the Accounts session behind an `sscli-` bearer.
//!
//! A `Space` is a function of `(url, Auth)`: it reads nothing ambient and writes nothing.
//! Anything that needs pixels stays in the web app: [`Space::window_url`] hands back the link.

use std::io;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use ureq::Agent;
use ureq::http::Request;

use crate::types::*;
use crate::{Auth, Error, WindowOutput, windows};

/// A Space Station and the account it acts as.
pub struct Space {
    url: String,
    auth: Auth,
    agent: Agent,
}

/// Errors arrive as responses, never as `Err`, so their bodies can be read.
pub(crate) fn agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .new_agent()
}

/// `url` without its trailing slash, or the one reason it cannot name a Space Station.
pub(crate) fn origin(url: &str) -> Result<String, Error> {
    let url = url.trim_end_matches('/');
    let authority = url.split_once("://").and_then(|(_, rest)| rest.split('/').next()).unwrap_or_default();
    match (
        url.starts_with("http://") || url.starts_with("https://"),
        authority.is_empty(),
        authority.contains('@'),
        url.bytes().any(|b| b.is_ascii_control()),
    ) {
        (true, false, false, false) => Ok(url.to_string()),
        _ => Err(Error::Local(format!("{url} is not a valid http(s) origin"))),
    }
}

/// One request to `{origin}/api{path}` — with `bearer` when there is one — and one answer. A 2xx
/// body is parsed into `T` (an empty body is `null`, which is `()`); anything else is
/// [`Error::Api`] carrying the server's status and code.
pub(crate) fn call<T: DeserializeOwned>(
    agent: &Agent,
    origin: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<&Value>,
) -> Result<T, Error> {
    let req = Request::builder().method(method).uri(format!("{origin}/api{path}")).header("Accept", "application/json");
    let req = match bearer {
        Some(bearer) => req.header("Authorization", format!("Bearer {bearer}")),
        None => req,
    };
    let bad = |e: ureq::http::Error| Error::Local(e.to_string());
    let sent = match body {
        Some(v) => {
            let bytes = serde_json::to_vec(v).map_err(io::Error::from)?;
            agent.run(req.header("Content-Type", "application/json").body(bytes).map_err(bad)?)
        }
        None => agent.run(req.body(()).map_err(bad)?),
    };
    let mut res = sent.map_err(|e| Error::Transport(format!("{origin}: {e}")))?;
    let status = res.status().as_u16();
    let read = res.body_mut().with_config().limit(64 << 20).read_to_string();
    let text = read.map_err(|e| Error::Transport(format!("{method} {path}: {e}")))?;
    let value: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    if status >= 400 {
        let e = &value["error"];
        return Err(match e["code"].as_str() {
            Some(code) => Error::Api { status, code: code.into(), message: e["message"].as_str().unwrap_or("").into() },
            None => Error::Api { status, code: format!("http_{status}"), message: text },
        });
    }
    serde_json::from_value(value).map_err(|e| Error::Transport(format!("{method} {path}: unexpected reply: {e}")))
}

impl Space {
    /// The Space Station at `url`, as the account authenticated by `auth`.
    pub fn new(url: impl Into<String>, auth: Auth) -> Result<Space, Error> {
        Ok(Space { url: origin(&url.into())?, auth, agent: agent() })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The authenticated carbon or silicon, including its immutable account UUID.
    pub fn me(&self) -> Result<Identity, Error> {
        self.get("/me")
    }

    /// Where the UI lives, for the links a terminal hands off.
    pub fn app_url(&self) -> Result<String, Error> {
        let me: Value = self.get("/me")?;
        Ok(me["app"].as_str().unwrap_or(&self.url).to_string())
    }

    /// End a terminal session at the server. No other credential has a session row to end, so
    /// nothing is sent for one.
    pub fn logout(&self) -> Result<(), Error> {
        match self.auth.is_session() {
            true => self.post("/auth/logout", None),
            false => Ok(()),
        }
    }

    // ── tables ──────────────────────────────────────────────────────────────────────────────

    pub fn tables(&self) -> Result<Vec<Table>, Error> {
        self.tables_query(None)
    }

    /// Retired tables remain readable for analytics, but are excluded from normal listings.
    pub fn retired_tables(&self) -> Result<Vec<Table>, Error> {
        self.tables_query(Some("true"))
    }

    /// All active and retired tables, for lookup and administrative tooling.
    pub fn all_tables(&self) -> Result<Vec<Table>, Error> {
        self.tables_query(Some("all"))
    }

    fn tables_query(&self, retired: Option<&str>) -> Result<Vec<Table>, Error> {
        let path = match retired {
            Some(value) => format!("/tables?retired={value}"),
            None => "/tables".to_string(),
        };
        self.get(&path)
    }

    /// One table. The server has no route for a single one: the list is the source, filtered.
    pub fn table(&self, id: &str) -> Result<Table, Error> {
        self.all_tables()?.into_iter().find(|t| t.id == id).ok_or_else(|| missing("table", id))
    }

    pub fn retire_table(&self, id: &str) -> Result<(), Error> {
        self.post(&format!("/tables/{id}/retire"), None)
    }

    pub fn unretire_table(&self, id: &str) -> Result<(), Error> {
        self.post(&format!("/tables/{id}/unretire"), None)
    }

    /// Create a table. The key it answers with is shown exactly once, here.
    pub fn create_table(&self, id: &str) -> Result<Key, Error> {
        self.post("/tables", Some(json!({"id": id, "access": []})))
    }

    /// A new key for the table; the old one stops resolving at once.
    pub fn rotate_table_key(&self, id: &str) -> Result<Key, Error> {
        self.post(&format!("/tables/{id}/rotate-key"), None)
    }

    /// Delete the table. Its records go too, on ClickHouse's own clock, and the id is free to
    /// reuse immediately.
    pub fn delete_table(&self, id: &str) -> Result<(), Error> {
        self.delete(&format!("/tables/{id}"))
    }

    /// The Tables tab in one value. `window` is one of `1m 5m 15m 1h 5h 1d 7d 30d`.
    pub fn overview(&self, window: &str) -> Result<Overview, Error> {
        self.get(&format!("/tables/overview?window={window}"))
    }

    // ── space windows ───────────────────────────────────────────────────────────────────────

    pub fn windows(&self) -> Result<Vec<Window>, Error> {
        self.get("/windows")
    }

    pub fn window(&self, id: &str) -> Result<Window, Error> {
        self.get(&format!("/windows/{id}"))
    }

    /// Create a window; the name is what the UI shows, under 20 characters.
    pub fn create_window(&self, name: &str) -> Result<Window, Error> {
        self.post("/windows", Some(json!({"name": name, "access": []})))
    }

    /// Rename a window owned by the authenticated account.
    pub fn update_window(&self, id: &str, name: &str) -> Result<Window, Error> {
        self.put(&format!("/windows/{id}"), json!({"name": name}))
    }

    /// Delete the window, its versions and its stored state.
    pub fn delete_window(&self, id: &str) -> Result<(), Error> {
        self.delete(&format!("/windows/{id}"))
    }

    pub fn versions(&self, id: &str) -> Result<Vec<Version>, Error> {
        self.get(&format!("/windows/{id}/versions"))
    }

    /// Publish a named processor/renderer pair, which becomes the current version at once. Code
    /// carrying a secret is refused here before it travels; the backend is the actual gate.
    pub fn publish(&self, id: &str, name: &str, processor: &str, renderer: &str) -> Result<Version, Error> {
        windows::preflight("processor", processor)?;
        windows::preflight("renderer", renderer)?;
        let body = json!({"name": name, "processor": processor, "renderer": renderer});
        self.post(&format!("/windows/{id}/versions"), Some(body))
    }

    /// The last SiliconJSON a runner published for this window, and how fresh it is.
    pub fn window_state(&self, id: &str) -> Result<WindowState, Error> {
        self.get(&format!("/windows/{id}/state"))
    }

    /// Run this window's published processor locally in Node, feeding every line of the child's
    /// output to `on_line`, until the window is idle (10 minutes) or the child stops. The window
    /// is fetched first, so an id that is not there answers `not_found` like every other call;
    /// the embedded runtime is unpacked into `runtime_dir` when its bytes differ.
    pub fn run_window(
        &self,
        id: &str,
        runtime_dir: &Path,
        on_line: impl Fn(WindowOutput<'_>) + Sync,
    ) -> Result<(), Error> {
        self.window(id)?;
        windows::run_window(self, id, runtime_dir, on_line)
    }

    /// Run one of the processor's tools against the cached SiliconJSON, once. A refusal — an
    /// unknown tool, `invalid_args`, a `timeout`, a failed run, or anything the backend refused
    /// the runtime — is [`Error::Api`] with the runtime's `code` and a `status` of 0.
    pub fn window_tool(&self, id: &str, runtime_dir: &Path, name: &str, args: &Value) -> Result<Value, Error> {
        windows::window_tool(self, id, runtime_dir, name, args)
    }

    /// The page that renders this window. Graphs, live views and video belong to the web app;
    /// this is how a terminal hands off to it.
    pub fn window_url(&self, id: &str) -> Result<String, Error> {
        let me: Value = self.get("/me")?;
        let account = me["uuid"].as_str().ok_or_else(|| Error::Transport("account UUID missing from /me".into()))?;
        let app = me["app"].as_str().unwrap_or(&self.url);
        Ok(format!("{app}/a/{account}/windows/{id}"))
    }

    // ── notifications ───────────────────────────────────────────────────────────────────────

    pub fn notifications(&self) -> Result<Vec<Notification>, Error> {
        self.get("/notifications")
    }

    pub fn notification(&self, id: &str) -> Result<Notification, Error> {
        self.get(&format!("/notifications/{id}"))
    }

    /// Create a notification delivered to this account or its webhooks.
    pub fn create_notification(&self, def: &Def, recipients: &[&str]) -> Result<Notification, Error> {
        self.post("/notifications", Some(json!({"def": def, "recipients": recipients})))
    }

    /// Replace the definition with a new version; `None` recipients keeps the current subscribers.
    pub fn update_notification(&self, id: &str, def: &Def, recipients: Option<&[&str]>) -> Result<Notification, Error> {
        self.put(&format!("/notifications/{id}"), json!({"def": def, "recipients": recipients}))
    }

    /// Delete the notification, its versions and its event history.
    pub fn delete_notification(&self, id: &str) -> Result<(), Error> {
        self.delete(&format!("/notifications/{id}"))
    }

    /// What it has fired, newest first. Append only: storing an event is what "read" means.
    pub fn events(&self, id: &str) -> Result<Vec<Event>, Error> {
        self.get(&format!("/notifications/{id}/events"))
    }

    /// Subscribe this actor; the new recipient list comes back. A silicon needs its delivery
    /// webhook set first, since that is where its events land.
    pub fn subscribe(&self, id: &str) -> Result<Vec<String>, Error> {
        let path = format!("/notifications/{id}/subscribe");
        Ok(self.post::<Recipients>(&path, None)?.recipients)
    }

    pub fn unsubscribe(&self, id: &str) -> Result<Vec<String>, Error> {
        let path = format!("/notifications/{id}/subscribe");
        Ok(self.send::<Recipients>("DELETE", &path, None)?.recipients)
    }

    /// Run its SQL now, over everything since its cursors, without advancing them.
    pub fn test_notification(&self, id: &str) -> Result<TestRun, Error> {
        self.post(&format!("/notifications/{id}/test"), None)
    }

    // ── settings ────────────────────────────────────────────────────────────────────────────

    pub fn webhooks(&self) -> Result<Vec<Webhook>, Error> {
        self.get("/webhooks")
    }

    /// An account webhook a notification can deliver to; the signing secret is shown once, here.
    pub fn create_webhook(&self, url: &str) -> Result<Key, Error> {
        self.post("/webhooks", Some(json!({"url": webhook_url(url)?})))
    }

    pub fn delete_webhook(&self, id: &str) -> Result<(), Error> {
        self.delete(&format!("/webhooks/{id}"))
    }

    /// This silicon's own delivery webhook, where `@si:<handle>` recipients land. Every write
    /// replaces the URL and mints a new signing secret.
    pub fn set_silicon_webhook(&self, url: &str) -> Result<Key, Error> {
        self.put("/silicon-webhook", json!({"url": webhook_url(url)?}))
    }

    /// Drop this silicon's delivery webhook; its notifications stop arriving anywhere.
    pub fn delete_silicon_webhook(&self) -> Result<(), Error> {
        self.delete("/silicon-webhook")
    }

    pub fn api_keys(&self) -> Result<Vec<ApiKey>, Error> {
        self.get("/api-keys")
    }

    /// A key acting for the account inside `scopes` (`tables`, `notifications`), shown once.
    pub fn create_api_key(&self, scopes: &[&str]) -> Result<Key, Error> {
        self.post("/api-keys", Some(json!({"scopes": scopes})))
    }

    pub fn delete_api_key(&self, id: &str) -> Result<(), Error> {
        self.delete(&format!("/api-keys/{id}"))
    }

    /// The access token processors and dev servers use, minted on first sight. One per account, and the server can read it back, so this may be asked for as often as you like.
    pub fn access_token(&self) -> Result<AccessToken, Error> {
        self.get("/access-token")
    }

    pub fn rotate_access_token(&self) -> Result<AccessToken, Error> {
        self.post("/access-token/rotate", None)
    }

    // ── data ────────────────────────────────────────────────────────────────────────────────

    /// One read-only query over the account's tables, plus the cursor each table was read up to.
    pub fn query(&self, sql: &str, restrict: &Restrict) -> Result<Rows, Error> {
        self.post("/query", Some(json!({"sql": sql, "restrict": restrict})))
    }

    /// What went wrong server-side for this account: notification runs, deliveries, refused rows.
    pub fn dev_errors(&self) -> Result<Vec<DevError>, Error> {
        self.get("/dev-errors")
    }

    // ── the wire ────────────────────────────────────────────────────────────────────────────

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        self.send("GET", path, None)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: Option<Value>) -> Result<T, Error> {
        self.send("POST", path, body)
    }

    fn put<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<T, Error> {
        self.send("PUT", path, Some(body))
    }

    fn delete(&self, path: &str) -> Result<(), Error> {
        self.send("DELETE", path, None)
    }

    /// One request as this credential; the answer, or the server's refusal, as it was given.
    fn send<T: DeserializeOwned>(&self, method: &str, path: &str, body: Option<Value>) -> Result<T, Error> {
        call(&self.agent, &self.url, method, path, Some(self.auth.token()), body.as_ref())
    }
}

/// `{"recipients": [...]}`, the answer to subscribing and unsubscribing.
#[derive(Deserialize)]
struct Recipients {
    recipients: Vec<String>,
}

/// What the server would have said, said here: the list already proved this one is not there.
fn missing(what: &str, id: &str) -> Error {
    Error::Api { status: 404, code: "not_found".into(), message: format!("no {what} {id}") }
}

/// A webhook URL as given, or the one thing worth saying before the server says anything.
fn webhook_url(url: &str) -> Result<&str, Error> {
    match url.trim() {
        "" => Err(Error::Local("a webhook URL is required, like https://example.com/hooks/space-station".into())),
        url => Ok(url),
    }
}
