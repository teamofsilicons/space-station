# space-station

The [Space Station](https://github.com/teamofsilicons/space-station) interface, in Rust. Two
halves, one dependency: **recording** sends events to a table, **managing** is everything else —
orgs, tables, space windows, notifications, webhooks, keys and queries.

Everything Space Station can do is a method here. The `spacestation` command is a shell over this
crate and has no capability of its own; the web app is a subset.

```toml
[dependencies]
space-station = "0.1"
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
(`https://backend.spacestation.teamofsilicons.com`) and `$SPACE_STATION_HOME` (`~/.space-station`).

## Managing

```rust
use space_station::{Auth, Space};

let url = space_station::default_url();
let auth = space_station::exchange(&url, slt, "tos")?;          // slt: what `iam silicon-login --app-id tos>spacestation` printed
let space = Space::new(&url, auth)?.org("tos");

let key = space.create_table("orders", &["@alice", "tech"])?;   // the table key, shown exactly once
let rows = space.query("SELECT count() FROM orders", &Default::default())?;
let window = &space.windows()?[0];
let link = space.window_url(&window.id)?;                       // graphs live in the app; this is the way there
```

This half is stateless: no file, no environment, no browser. `Auth` is a value you build, `Space`
is a function of `(url, Auth)`, and nothing here ever rotates — the backend holds the session and
refreshes it, so a 401 is final and means *sign in again*. `Auth` is `Serialize`/`Deserialize` so
it can be persisted verbatim, and `describe()` names what it is without revealing what it holds.

| | for | credential |
|---|---|---|
| `Auth::session(sscli)` | a carbon or a silicon signed in from a terminal | the `sscli-` session the backend minted, holds and refreshes |
| `Auth::access_token(t)` | window and processor code | a `spacewindow-` token |
| `Auth::api_key(k)` | a program acting for the org | an `apikey-` key |

Nothing here talks to IAM, and nothing here ever prompts for, receives or stores an IAM
credential — not a silicon's token, not a bearer, not a refresh token. Two functions produce a
session without keeping one:

- `space_station::exchange(url, slt, org)` spends a **short-lived token** — what `iam login
  --app-id tos>spacestation --org <org>` or `iam silicon-login --app-id tos>spacestation` prints,
  two minutes old at most and good for one exchange — at `POST /api/auth/session {slt, org}` and
  returns the `Auth::session` the backend minted for it, bound to `org`. Anything that is not a
  short-lived token is refused before it leaves the machine as `Error::Local("not a short-lived
  token…")`: a silicon's `stk-` token, the Application's `ask_` secret, an IAM bearer (`sat_`,
  `cat_`, `oat_`) or refresh token (`rft_`, `ort_`), and Space Station's own secrets. Only the
  `oac_` token IAM minted to be handed over goes through.
- `space_station::login(url, org, visit)` is the browser flow: it binds a loopback port, calls
  `visit(link)` so you can open or print the link, catches a short-lived token and exchanges it
  for `Auth::session` — it never opens a browser itself.

A session is bound to exactly one org, and every org call needs `.org("tos")`.

A session lives as long as IAM lets the backend refresh it, and IAM ties that to the actor's own
IAM login: a **new** IAM login by the same actor — a second device, `iam login` with a fresh code,
`iam silicon-login --sid … --stk …` run again — makes IAM refuse the refresh of the older Space
Station sessions, so within about 30 minutes their calls answer `Error::Api { status: 401, .. }`
and the program `exchange`s or `login`s again. Minting another short-lived token from the IAM
session already held (`iam login --app-id …`, `iam silicon-login --app-id …` without `--stk`) is
not a new login and ends nothing.

`Space` is one method per operation, synchronous, each returning a typed value or `Error`:

```
orgs() me() app_url()                                   whoami and where the UI lives
tables() table(id) create_table(id, access) -> Key      set_table_access(id, access)
rotate_table_key(id) -> Key  delete_table(id)  overview(window)
windows() window(id) create_window(name, access) update_window(id, name, access) delete_window(id)
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

`me()` is `Identity { kind, id, org, tags }` — the public id is the handle people know; there is no
display name. A refused request is `Error::Api { status, code, message }`; branch on `code`, which
is snake_case and stable, never on the message. A `status` of 401 is final — there is nothing here
to retry with — so a program tells its person to sign in again. Nothing that needs pixels is
invented here: `window_url(id)` hands back the page instead.

`publish` refuses code carrying a secret — Space Station's or IAM's, the same shapes `exchange`
refuses — before it leaves the machine, and `run_window` unpacks
the mission-control runtime into the directory you name and runs the window's processor in `node`
with nothing in its environment but `PATH` and the access token, one output line per callback,
until the window goes idle. Its callback receives `WindowOutput::Json(line)` for SiliconJSON
and `WindowOutput::Diagnostic(line)` for status and errors, keeping data separate from diagnostics.

## What happens to a record

1. `record()` stamps `record_id`, `table_id` and `event_ts_ms`, sanitises the value (below), and
   pushes one JSON line into a queue of 10 000. A full queue drops the line and reports `QueueFull`.
2. A sender thread writes the lines to `~/.space-station/daemon.sock`. When nobody is listening it
   takes `flock` on `daemon.lock`; the winner starts the daemon on a thread inside your process, a
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
(all 0600). Sends as soon
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
