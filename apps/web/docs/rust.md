# The Rust package

`space-station` is the interface. Everything Space Station can do is a method on this crate: the
[CLI](/docs/cli) is a `clap` tree over it with no capability of its own, and this app is a subset
of the same surface. If you are writing code rather than typing commands, this is the whole
product.

```toml
[dependencies]
space-station = "0.4"
```

Version 0.4 is published on [crates.io](https://crates.io/crates/space-station). MSRV 1.89.

It is synchronous throughout — every call is one request and one answer — and it never opens a
browser. Supported platforms are macOS, Linux, and Windows 10 1803 or newer, using native local
sockets and platform file locks.

## Two halves

**Recording** is [`SpaceClient`](#recording): a table key in, JSON records out, non-blocking,
backed by the machine's daemon. **Managing** is `Auth` + `Space`: who you are, and one typed
method per operation.

```rust
use space_station::{Auth, Space, SpaceClient};

// Recording: the table key that was shown once when the table was created.
let ss = SpaceClient::new("table-orders-…")?;
ss.record(serde_json::json!({"id": "o-42", "amount": 12.5}));

// Managing: the current Carbon or Silicon account.
// SLT from `silicon-accounts login --app spacestation -q`.
let url = space_station::default_url();
let auth = space_station::exchange(&url, &slt)?;
let space = Space::new(&url, auth)?;
let tables = space.tables()?;
let key = space.create_table("orders")?;
let rows = space.query("SELECT count() FROM orders", &Default::default())?;
```

`Space::new` takes the Space Station URL and an `Auth`. All calls use that credential's account;
keep separate credentials and caches for different accounts. Account UUIDs are immutable,
while `c:` and `si:` handles are display identifiers.

## Stateless

The managing half keeps nothing. `Auth` is a value you build; `Space` is a function of `(url,
Auth)`; neither reads a file, the environment or the home directory, and neither writes. The same
code with the same inputs does the same thing on any machine.

The one thing that changes over time — an Application session's refresh token rotates on every
use, and presenting a stale one revokes the whole family — is not this crate's problem by
construction: the session and its refresh live in the backend, which alone holds the Application
secret, and an `sscli-` credential is a stable handle to it. The package never rotates anything,
so there is nothing to emit or persist mid-run. `Auth` is `Serialize`/`Deserialize`, so persisting
it verbatim is one `serde_json::to_string`.

The recording half is stateful on purpose — the spool and the daemon lock are what survive a
crash — but never ambient: `Builder::home` and `daemon::Config` name the directory, and
`run_window` takes the directory it may unpack the runtime into. The [CLI](/docs/cli) is the one
that stores each account context in `<home>/accounts/<server-hash>/<profile>/auth.json`; select one
with `--profile` or `$SPACE_STATION_PROFILE`. The recording spool stays in the shared base home.

## Auth

`Auth` is how you prove who you are — a value, never a file:

| | for | credential |
|---|---|---|
| `Auth::session(sscli)` | a carbon or a silicon signed in from a terminal | the `sscli-` session the backend minted and refreshes |
| `Auth::access_token(t)` | window and processor code | a `spacewindow-` token |
| `Auth::api_key(k)` | a program acting for the account | an `apikey-` key |

`Auth::describe()` says what it is and never what it holds; `Debug` prints the same. There is no
constructor that takes a Silicon Accounts bearer, refresh token or Silicon STK.

**Carbon browser flow.** `space_station::login(url, visit)` opens a loopback listener and passes
a sign-in URL to `visit`. The backend performs the OAuth/PKCE exchange and redirects a one-time
handoff. The client verifies its state and redeems the handoff over POST for a session.

```rust
let auth = space_station::login(&space_station::default_url(), |link| {
    eprintln!("open this to sign in:\n{link}");
})?;
```

**Silicon token flow.** `space_station::exchange(url, slt)` posts the `slt_` token issued by
`silicon-accounts login --app spacestation -q` to `/api/auth/session`. It returns a session
and its absolute expiry. The token can be exchanged once within two minutes.

The backend alone holds the app secret and refreshes Silicon Accounts access tokens. Browser
and terminal sessions persist independently until expiration, revocation or logout. Temporary
network and service errors do not clear the saved credential. `Auth::expires_at()` exposes the
server session expiry when known.

See [Credentials](/docs/credentials) for what each one may do.

## Space

One method per operation. Each returns a typed value or `Error`.

```
me()  app_url()                                          who you are, where the UI lives

tables()  table(id)  create_table(id) -> Key
rotate_table_key(id) -> Key   delete_table(id)   overview(window)

windows()  window(id)  create_window(name)  update_window(id, name)
delete_window(id)  versions(id)  publish(id, name, processor, renderer)  window_state(id)
run_window(id, runtime_dir, on_line)  window_tool(id, runtime_dir, name, args)  window_url(id)

notifications()  notification(id)  create_notification(def, recipients)
update_notification(id, def, recipients)  delete_notification(id)  events(id)
subscribe(id)  unsubscribe(id)  test_notification(id)

webhooks()  create_webhook(url) -> Key  delete_webhook(id)  set_silicon_webhook(url) -> Key  delete_silicon_webhook()
api_keys()  create_api_key(scopes) -> Key  delete_api_key(id)
access_token()  rotate_access_token()

query(sql, restrict)  dev_errors()  logout()
```

`me()` returns the account's `kind`, display `id`, immutable `uuid`, `app`, and session expiry.
Each account owns its data. `Key` is a secret the server
shows once; `AccessToken` can be read back. `update_window` and `update_notification` accept
`Option`s: `None` leaves that field alone. Every returned type implements `Serialize`.

`delete_table`, `delete_window` and `delete_notification` need the same access as reading the
thing. A window takes its versions and its state with it, a notification its versions and its
events, and a table its records — those by a ClickHouse mutation that runs after the call
returns, so the id is free to reuse at once and a query in the next second may still see a few.

### Reading records

```rust
let rows = space.query("SELECT count() AS n FROM orders", &Default::default())?;
println!("{} rows, watermarks {:?}", rows.rows.len(), rows.watermarks);
```

The second argument is a `Restrict` — `BTreeMap<String, Bounds>`, the cursor range to read each
table over. Leave it `Default::default()` to see everything up to the current watermark; the
watermarks actually used come back with the rows, so the next call can carry on from them. The
SQL is the same read-only dialect the app and notifications use: see [SQL](/docs/sql). Rows are
`serde_json::Value` objects exactly as ClickHouse emitted them, which means every 64-bit integer
— `cursor`, `event_ts_ms`, `registered_ts_ms`, and any `count()`, `sum()` of an integer or
`::Int64` — is a `Value::String`; `as_str().parse::<u64>()` it, or cast in SQL (`toInt32(count())`).
The package coerces nothing.

### Windows, and the pixels problem

`run_window` and `window_tool` need Node. They write the embedded runtime into the `runtime_dir`
you name — `<dir>/mission-control.js`, only when its bytes differ — and spawn `node` on it with
only `PATH` and `SPACE_STATION_ACCESS_TOKEN` in the environment; the processor itself is a
further child with no credential at all. `run_window(id, runtime_dir, on_line)` streams every
JSON and diagnostic output through separate callback variants and returns when the window goes idle:

```rust
let dir = std::env::temp_dir().join("space-station-runtime");
space.run_window("w_01", &dir, |event| match event {
    space_station::WindowOutput::Json(line) => println!("{line}"),
    space_station::WindowOutput::Diagnostic(line) => eprintln!("{line}"),
})?;
let detail = space.window_tool("w_01", &dir, "order_detail", &serde_json::json!({"order_id": "o_9"}))?;
```

What the crate deliberately does not do is draw. A renderer is an iframe, a chart is pixels, and
neither belongs in a process with no screen — so `window_url(id)` returns
`{app}/a/{uuid}/windows/{id}`, where `{app}` is where the UI lives according to `GET /me`, and you
hand that to a browser. That is the rule for anything visual: the package returns a link, the app
renders it.

### <a id="recording"></a>Recording

```rust
use space_station::SpaceClient;

let ss = SpaceClient::builder(&key)
    .on_error(|e| eprintln!("space station: {e}"))
    .build()?;
ss.record(serde_json::json!({"id": "o-42", "amount": 12.5}));
```

`record()` stamps metadata, sanitises the value and pushes it into a bounded queue; a sender
thread hands it to the machine's daemon, which spools it to disk and ships it over one WebSocket.
It never blocks and never panics. `flush()` and `Drop` block — at most 5 s, or
`Builder::flush_timeout` — until the in-process daemon has had everything this client recorded
acked by the server, so a program that records and returns from `main` still delivers; when a
separate `spacestation daemon` carries the records it waits only until they are in that daemon's
spool, which outlives the program. `flush()` answers `false` when the bound ran out first, and
the spool keeps the lines for the next daemon. Server-side rejections (`Error::Rejected {
record_id, code, reason }`) reach the `on_error` of the client whose in-process daemon sent the
batch; when a separate daemon is carrying the records, they are that process's to report
(`daemon status`), and this hook never sees them. `Builder` also takes `.url()`, `.home()` and
`.flush_timeout()`.
`daemon::status(home)` says whether a daemon is listening for that home, where its socket is and
how many records are still unacked; `daemon::run(config)` is the daemon itself, in the
foreground. `daemon::socket_path(home)` is where it listens: `<home>/daemon.sock` when that fits
a socket address (104/108 bytes on macOS/Linux), else `<temp dir>/space-station-<hash of
home>.sock`, so a long `home` moves only the socket. The whole path — limits, metadata,
rejection codes — is in [Getting started](/docs/getting-started).

## Errors

One `Error` for both halves. Branch on the variant, and inside `Api` on `code` — never on a
message:

| variant | |
|---|---|
| `InvalidKey` | the string is not `table-{id}-{32 hex}` |
| `SizeExceeded { bytes }` · `QueueFull` | a record refused before it left the process |
| `Rejected { record_id, code, reason }` | the server's verdict on one record |
| `Transport(..)` | Space Station could not be reached |
| `Api { code, message }` | it answered, and said no; `code` is snake_case and stable — `context_changed` means the selected account changed |
| `Local(..)` | nothing left the machine: no credential, no `node`, a secret in the code |
| `Io(..)` · `Ws(..)` | |

## Environment

The crate reads no `.env` file and no variable on its own. Two helpers are there for a program
that wants the conventions, and they are the only place the environment is consulted:

| | |
|---|---|
| `default_url()` | `$SPACE_STATION_URL`, else the public Space Station |
| `default_home()` | `$SILICON_HOME/.space-station`, else `$SPACE_STATION_HOME`, else `~/.space-station` — the shared base for the spool, lock and daemon socket. The CLI stores credentials below `accounts/<server-hash>/<profile>/` |

The URL defaults to the *public* host, so point it at a local stack explicitly rather than by
omission. Or skip the helpers and pass your own values to `Space::new`, `exchange`, `login` and
`SpaceClient::builder`.

## Next

[CLI](/docs/cli) · [Credentials](/docs/credentials) · [SQL](/docs/sql) ·
[Space Windows](/docs/space-windows) · [HTTP API](/docs/api)
