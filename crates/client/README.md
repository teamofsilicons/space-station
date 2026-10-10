# space-station

The [Space Station](https://github.com/teamofsilicons/space-station) interface, in Rust. Two
halves, one dependency: **recording** sends events to a table, **managing** is everything else —
account tables, space windows, notifications, webhooks, keys and queries.

Everything Space Station can do is a method here. The `spacestation` command is a shell over this
crate and has no capability of its own; the web app is a subset.

```toml
[dependencies]
space-station = "0.4"
```

## Recording

```rust
use space_station::SpaceClient;

let ss = SpaceClient::new("table-orders-…")?;                 // the key shown once when the table was created
ss.record(serde_json::json!({"order": 42, "price": 9.5}));    // non-blocking, never panics
ss.flush();                                                   // optional: wait ≤ 5 s for delivery; Drop does the same
```

Anything that implements `serde::Serialize` can be recorded; send a JSON object. Errors are events,
not panics: `SpaceClient::builder(key).on_error(|e| …).build()` receives every one of them (the
default prints to stderr). `.url(..)` and `.home(..)` override `$SPACE_STATION_URL`
(`https://backend.spacestation.teamofsilicons.com`) and the default home
(`$SILICON_HOME/.space-station`, else `$SPACE_STATION_HOME`, else `~/.space-station`).

### Space Station telemetry

Provision the ordinary `si:tos/spacestation` table once and keep its one-time key in
`SPACE_STATION_TELEMETRY_KEY` or `<SILICON_HOME>/.space-station/telemetry.key`. Then use the same
daemon and spool for context-rich events:

```rust
use space_station::Telemetry;

if let Some(telemetry) = Telemetry::from_env()? {
    telemetry.record("daemon", "ship", Some(0.5), "batch_sent", serde_json::json!({"count": 12}));
}
```

Set `SPACE_STATION_TELEMETRY=0` to opt out. Missing telemetry configuration is treated as
disabled so an application never fails to start because observability is unavailable.

## Managing

```rust
use space_station::{Auth, Space};

let url = space_station::default_url();
let auth = space_station::exchange(&url, slt)?;          // slt: what `silicon-accounts login --app spacestation -q` printed
let space = Space::new(&url, auth)?;

let key = space.create_table("orders")?;   // the table key, shown exactly once
let rows = space.query("SELECT count() FROM orders", &Default::default())?;
let window = &space.windows()?[0];
let link = space.window_url(&window.id)?;                       // graphs live in the app; this is the way there
```

This half is stateless: no file, no environment, no browser. `Auth` is a value you build, `Space`
is a function of `(url, Auth)`, and nothing here ever rotates — the backend holds the session and
refreshes it. A confirmed expired or revoked session means *sign in again*. `Auth` is `Serialize`/`Deserialize` so
it can be persisted verbatim, and `describe()` names what it is without revealing what it holds.

| | for | credential |
|---|---|---|
| `Auth::session(sscli)` | a carbon or a silicon signed in from a terminal | the `sscli-` session the backend minted, holds and refreshes |
| `Auth::access_token(t)` | window and processor code | a `spacewindow-` token |
| `Auth::api_key(k)` | a program acting for the account | an `apikey-` key |

The client never receives an STK, Accounts access token, or refresh token. Two functions produce
a session without storing it:

- `space_station::exchange(url, slt)` sends an app-bound `slt_` token from
  `silicon-accounts login --app spacestation -q` to `POST /api/auth/session {slt}`. The backend
  exchanges it with Accounts and returns a stable `Auth` carrying its absolute session expiry.
- `space_station::login(url, visit)` opens a loopback listener and calls `visit(link)` so the host
  can open or print the sign-in URL. The backend uses authorization code and PKCE, then redirects
  a one-time handoff to loopback. The client checks state and exchanges `{code, state}` for a
  session. It never opens a browser itself.

Every request acts as the credential's carbon or silicon account. Keep separate `Auth` values
for separate accounts. The CLI isolates profiles and backend URLs automatically. Serialize
`Auth` to persist the session until expiry or logout; `expires_at()` exposes its server-issued
expiration and `is_expired()` checks it without a network call. Temporary transport failures
must not discard an unexpired session. The backend refreshes Accounts
access tokens while the absolute session lifetime remains valid.

`Space` is one method per operation, synchronous, each returning a typed value or `Error`:

```
me() app_url()                                   whoami and where the UI lives
tables() table(id) create_table(id) -> Key
rotate_table_key(id) -> Key  delete_table(id)  overview(window)
windows() window(id) create_window(name) update_window(id, name) delete_window(id)
versions(id) publish(id, name, processor, renderer) window_state(id)
run_window(id, runtime_dir, on_line) window_tool(id, runtime_dir, name, args) window_url(id)
logout()
notifications() notification(id) create_notification(def, recipients) update_notification(id, …)
delete_notification(id) events(id) subscribe(id) unsubscribe(id) test_notification(id)
webhooks() create_webhook(url) -> Key delete_webhook(id) set_silicon_webhook(url) -> Key
api_keys() create_api_key(scopes) -> Key delete_api_key(id)
access_token() rotate_access_token()
query(sql, restrict) dev_errors()
```

`me()` returns `Identity { kind, id, uuid, context_id, expires_at }`. UUID identifies an account
permanently; `c:<handle>` and `si:<handle>` are display IDs that can change. A refused request is
`Error::Api { status, code, message }`; branch on the stable code. Confirmed expiry or revocation
requires a fresh sign-in, while temporary failures preserve saved credentials. `window_url(id)`
returns the account-bound browser page.

`publish` refuses code carrying a secret — Space Station's or Silicon Accounts' credentials — before it leaves the machine, and `run_window` unpacks
the mission-control runtime into the directory you name and runs the window's processor in `node`
with only `PATH`, the access token and Windows `SystemRoot` in its environment, one output line per callback,
until the window goes idle. Its callback receives `WindowOutput::Json(line)` for SiliconJSON
and `WindowOutput::Diagnostic(line)` for status and errors, keeping data separate from diagnostics.

## What happens to a record

1. `record()` stamps `record_id`, `table_id` and `event_ts_ms`, sanitises the value (below), and
   pushes one JSON line into a queue of 10 000. A full queue drops the line and reports `QueueFull`.
2. A sender thread writes the lines to `~/.space-station/daemon.sock`. When nobody is listening it
   takes the OS file lock on `daemon.lock`; the winner starts the daemon on a thread inside your process, a
   loser simply connects to the winner's socket.
3. The daemon appends every line to `spool.jsonl` and ships the oldest unacked lines, from every key
   on the machine, as one in-flight batch over a WebSocket (`/api/ws/ingest`). The server's ack
   moves `spool.cursor`; anything unacked is resent, under a new `batch_id`, after a reconnect.

`flush()` waits up to five seconds for the server to acknowledge everything recorded so far when
this process runs the daemon. If another process owns the daemon, it returns once that daemon
has spooled the records. `false` means delivery was not confirmed before the deadline; records
already on disk remain available for retry. Set `.flush_timeout(Duration)` on the builder to
change the wait.

## The daemon

One per machine, any number of table keys and processes. It lives in the home it is given,
`~/.space-station/` by default (0700): `daemon.lock`, `daemon.sock`, `spool.jsonl`, `spool.cursor`
(all 0600; owner-only Windows DACLs). Windows requires Windows 10 1803 or newer for native
AF_UNIX sockets. Sends as soon
as it can, never batches on purpose; a batch is whatever is waiting, up to 8 MB. Pings every 20 s,
gives up on a batch after 30 s without an ack, reconnects with backoff from 1 s to 30 s, and
truncates a fully acked spool once it passes 64 MB. It logs one line per event to stderr.

Rejections come back per record with a code. `duplicate` is treated as accepted;
`unauthorized`, `size_exceeded` and `invalid` are dropped and reported through `on_error` as
`Error::Rejected { record_id, code, reason }`.

`spacestation daemon` (the `space-station-cli` crate) runs the very same daemon in the foreground,
for a service manager, so records ship even when no application is running. Programmatically:
`space_station::daemon::run(space_station::daemon::Config::default())`, and
`space_station::daemon::status(home)` says whether one is listening and how much the spool still
owes.

## Metadata

```json
{"record_id": "…uuid…", "table_id": "orders", "event_ts_ms": 1725000000000,
 "system": {"hostname": "…", "os": "…", "arch": "…", "cpu": "…", "cores": 8, "ram_mb": 16384},
 "cpu_pct": 12.5, "gpu_pct": null, "ram_pct": 41.0, "disk_free_mb": 120000}
```

The first three are stamped by `record()`. The rest is sampled by the daemon when the batch leaves:
`system` once per daemon, the live values at most once a second (`gpu_pct` via `nvidia-smi` when it
is installed, otherwise `null`). `disk_free_mb` is the volume that holds `~/.space-station`.

## Limits

| | |
|---|---|
| string value | over 32 KB is cut in the middle: `"abc...[SIZE]...xyz"`, `SIZE` the original byte length |
| file-looking string | data URI, magic bytes, base64 shape or control-character heavy becomes `"[FILETYPE:SIZE]"` |
| record | over 256 KB after sanitising is refused locally: `SizeExceeded` |
| queue | 10 000 lines waiting for the daemon: `QueueFull` |
| batch | 8 MB per frame |
| SiliconJSON | 64 KB per processor result or tool answer |

Sizes are UTF-8 byte lengths of the JSON text as sent.
