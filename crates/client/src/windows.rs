//! Space windows from a terminal: the publish pre-flight, and the Node host that runs a window's
//! processor locally. The runtime ships inside the binary and is unpacked into the directory the
//! caller names only when its bytes differ, so `node` always runs the copy this build was made
//! with and this crate never picks a place on disk by itself.
//!
//! The child is spawned with nothing but `PATH` and the access token: no `.env`, no home, no
//! inherited credentials. Its output is an event per line, never a print from here.

use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::{env, fs};

use serde_json::Value;

use crate::shared::secrets;
use crate::{Error, Space};

/// The runtime, vendored by `scripts/sync-runtime.sh` because `cargo package` cannot reach
/// outside the crate.
pub(crate) const RUNTIME: &str = include_str!("../runtime/mission-control.js");

const NO_NODE: &str = "`node` was not found; running a window needs Node >= 22.13 (https://nodejs.org)";

/// One line from a running processor. JSON data and diagnostics stay separate so a caller can
/// pipe the data into another program without status messages appearing among its records.
pub enum WindowOutput<'a> {
    Json(&'a str),
    Diagnostic(&'a str),
}

/// Refuses code carrying any secret-shaped token, naming only its first characters. The backend
/// runs the same scan on every publish; this saves the round trip and keeps the secret local.
pub(crate) fn preflight(what: &str, code: &str) -> Result<(), Error> {
    match secrets::find_secret(code) {
        Some(token) => Err(Error::Local(format!(
            "the {what} contains a secret ({}…); remove it before publishing",
            &token[..12.min(token.len())]
        ))),
        None => Ok(()),
    }
}

/// `<dir>/mission-control.js`, written only when it differs from the embedded copy.
pub(crate) fn runtime(dir: &Path) -> Result<PathBuf, Error> {
    let path = dir.join("mission-control.js");
    if fs::read(&path).ok().as_deref() != Some(RUNTIME.as_bytes()) {
        fs::create_dir_all(dir)
            .and_then(|()| fs::write(&path, RUNTIME))
            .map_err(|e| Error::Local(format!("{}: {e}", path.display())))?;
    }
    Ok(path)
}

/// Runs the published processor until the window goes idle, one line of the child's output at a
/// time. Both streams are events: the runtime writes SiliconJSON on stdout and status and dev
/// errors on stderr.
pub(crate) fn run_window(
    space: &Space,
    id: &str,
    dir: &Path,
    on_line: impl Fn(WindowOutput<'_>) + Sync,
) -> Result<(), Error> {
    let mut child = spawn(space, dir, "run", &["--window", id])?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    std::thread::scope(|scope| {
        scope.spawn(|| pump(err, &|line| on_line(WindowOutput::Diagnostic(line))));
        pump(out, &|line| on_line(WindowOutput::Json(line)));
    });
    let status = child.wait().map_err(Error::Io)?;
    match status.success() {
        true => Ok(()),
        false => Err(Error::Local(format!("the window runner stopped ({status})"))),
    }
}

/// One tool call against the cached SiliconJSON: the runtime prints the result and exits. Both
/// pipes are read at once — a chatty processor must never wedge on a full stderr.
pub(crate) fn window_tool(space: &Space, id: &str, dir: &Path, name: &str, args: &Value) -> Result<Value, Error> {
    let args = args.to_string();
    let child = spawn(space, dir, "tool", &["--window", id, "--name", name, "--args", &args])?;
    let done = child.wait_with_output().map_err(Error::Io)?;
    let text = |bytes| String::from_utf8_lossy(bytes).trim().to_string();
    match done.status.success() {
        true => serde_json::from_str(&text(&done.stdout)).map_err(|e| Error::Local(format!("tool {name}: {e}"))),
        false => Err(refusal(name, &text(&done.stderr))),
    }
}

/// What a failed runtime said on stderr: its `{"error": {"code", "message"}}` line — the
/// backend's own shape, often the backend's own refusal relayed — becomes [`Error::Api`] with the
/// code to branch on and a status of 0, since no HTTP status travelled with it. Anything else is
/// the child's output, as it was.
pub(crate) fn refusal(name: &str, stderr: &str) -> Error {
    let refused = stderr.lines().rev().find_map(|line| {
        let line: Value = serde_json::from_str(line).ok()?;
        let code = line["error"]["code"].as_str()?.to_string();
        let message = line["error"]["message"].as_str().unwrap_or_default().to_string();
        Some(Error::Api { status: 0, code, message })
    });
    refused.unwrap_or_else(|| Error::Local(format!("tool {name}: {stderr}")))
}

/// `node mission-control.js <role> --url U --org O <args…>` with only `PATH` and the access token
/// in its environment, and both its streams piped back here.
fn spawn(space: &Space, dir: &Path, role: &str, args: &[&str]) -> Result<Child, Error> {
    let runtime = runtime(dir)?;
    let token = space.access_token()?.token;
    let child = Command::new("node")
        .arg(runtime)
        .args([role, "--url", space.url(), "--org", space.scope()?])
        .args(args)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("SPACE_STATION_ACCESS_TOKEN", token)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    child.map_err(|e| match e.kind() {
        ErrorKind::NotFound => Error::Local(NO_NODE.into()),
        _ => Error::Local(format!("node: {e}")),
    })
}

/// Every complete line of a child stream, handed on as it arrives.
fn pump(stream: Option<impl Read>, on_line: &(impl Fn(&str) + Sync)) {
    let Some(stream) = stream else { return };
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        on_line(&line);
    }
}
