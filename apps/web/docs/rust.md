# The Rust package

`space-station` is the interface. Everything Space Station can do is a method on this crate: the
[CLI](/docs/cli) is a `clap` tree over it with no capability of its own, and this app is a subset
of the same surface. If you are writing code rather than typing commands, this is the whole
product.

```toml
[dependencies]
space-station = "0.1"
```

Not on crates.io yet — until it is published, depend on the repo instead:
`space-station = { git = "https://github.com/teamofsilicons/space-station", package = "space-station" }`
(or `path = "../space-station/crates/client"` from a checkout). MSRV 1.88.

It is synchronous throughout — every call is one request and one answer — and it never opens a
browser. Unix only, for now: the daemon uses a unix socket and `flock`.

## Two halves

**Recording** is [`SpaceClient`](#recording): a table key in, JSON records out, non-blocking,
backed by the machine's daemon. **Managing** is `Auth` + `Space`: who you are, and one typed
method per operation.

```rust
use space_station::{Auth, Space, SpaceClient};

// Recording: the table key that was shown once when the table was created.
let ss = SpaceClient::new("table-orders-…")?;
ss.record(serde_json::json!({"id": "o-42", "amount": 12.5}));

// Managing: an identity, an org, and typed calls. The short-lived token came from
// `iam login --app-id 'tos>spacestation' --org tos` or `iam silicon-login --app-id 'tos>spacestation'`.
let url = space_station::default_url();
let auth = space_station::exchange(&url, &slt, "tos")?;   // a value; the CLI is what stores one
let space = Space::new(&url, auth)?.org("tos");
let tables = space.tables()?;
let key = space.create_table("orders", &["@alice", "tech"])?;
let rows = space.query("SELECT count() FROM orders", &Default::default())?;
```

`Space::new` takes the Space Station URL and an `Auth`; `.org(id)` scopes it. Every org call
needs one, and a session is bound to the org it signed in to — a silicon's is the suffix of its
id, so pass that.

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
that owns `~/.space-station/`, a signed-in user and a browser.

## Auth

`Auth` is how you prove who you are — a value, never a file:

| | for | credential |
|---|---|---|
| `Auth::session(sscli)` | a carbon or a silicon signed in from a terminal | the `sscli-` session the backend minted and refreshes |
| `Auth::access_token(t)` | window and processor code | a `spacewindow-` token |
| `Auth::api_key(k)` | a program acting for the org | an `apikey-` key |

`Auth::describe()` says what it is and never what it holds; `Debug` prints the same. There is no
constructor that takes an IAM bearer, a refresh token or a silicon's long-lived `stk-` token, and
nothing here prompts. **Never a credential, always a short-lived token**: a carbon or a silicon
obtains one from IAM by their own means — `iam login --app-id 'tos>spacestation' --org o`, `iam
silicon-login --app-id 'tos>spacestation'`, or the browser — and hands that over; Space Station
exchanges it once and holds the resulting session itself.

Two actions produce a session without keeping one, and both are brokered by the backend, because
only the backend is the registered IAM Application and only it holds that secret:

**Browser at hand.** `space_station::login(url, org, visit)` binds `127.0.0.1:0`, hands
`{url}/api/auth/login?org={org}&cli={port}&state={nonce}` to `visit` — which opens or prints the
link and returns at once — and waits for the one request the backend redirects there. What
arrives is the **short-lived token** IAM minted (`?slt=…&state=…`), not a session: `login` checks
`state` and then does what `exchange` does, `POST /api/auth/session`, so the `sscli-` credential
is minted over that POST and never appears in a URL. `cli` is a bare port number and never a URL,
so loopback is the only reachable target. It returns `Auth::session`, bound to `org`, and stores
nothing:

```rust
let auth = space_station::login(&space_station::default_url(), "tos", |link| {
    eprintln!("open this to sign in:\n{link}");
})?;
```

**No browser.** `space_station::exchange(url, slt, org)` posts a short-lived token — minted by
`iam login --app-id` or `iam silicon-login --app-id`, good for two minutes and one exchange — as
`POST /api/auth/session {slt, org}` and returns the `Auth::session` the backend minted for it,
bound to `org`. Anything shaped like a credential is refused before it leaves the machine. This is
the only way a silicon signs in.

Either way the `sscli-` session is its own row: `Space::logout()` ends it and leaves any browser
signed in, and vice versa. A session is bound to one org; for another org, sign in again for it.

**Why nothing here talks to IAM.** The backend is the registered Application, holds its secret
and speaks to IAM through the official `silicon-iam-client`; this crate never talks to IAM at all.
A short-lived token is minted by the `iam` CLI or by the browser, and the backend is the only party
that exchanges it. The one host this crate ever reaches is Space Station — which is also why it
carries no IAM dependency and keeps its MSRV at 1.88.

See [Credentials](/docs/credentials) for what each one may do.

## Space

One method per operation. Each returns a typed value or `Error`.

```
orgs()  me()  app_url()                                    the orgs you are known in, who you are here, where the UI lives

tables()  table(id)  create_table(id, access) -> Key       set_table_access(id, access)
rotate_table_key(id) -> Key   delete_table(id)   overview(window)

windows()  window(id)  create_window(name, access)  update_window(id, name, access)
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

`me()` is `Identity { kind, id, org, tags }`: who this credential is inside the org, with the
tags IAM reported at the login and re-check (see [Credentials](/docs/credentials)); `orgs()` is
`[Org { id, name }]`, the name present once IAM has told Space Station one. Access lists are `&[&str]` of `@actor` ids and IAM tag names; `Key` is a secret
the server shows exactly once (a table key, an API key, a webhook signing secret) and
`AccessToken` is the one it can still read back. `update_window` and `update_notification` take
`Option`s: `None` leaves that half alone. Every returned type is `Serialize`, so printing the JSON
needs no second shape — which is exactly what the CLI does with them.

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
`{app}/o/{org}/windows/{id}`, where `{app}` is where the UI lives according to `GET /me`, and you
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
| `Api { code, message }` | it answered, and said no; `code` is snake_case and stable — `not_a_member` is a session bound to another org |
| `Local(..)` | nothing left the machine: no credential, no org, no `node`, a secret in the code |
| `Io(..)` · `Ws(..)` | |

## Environment

The crate reads no `.env` file and no variable on its own. Two helpers are there for a program
that wants the conventions, and they are the only place the environment is consulted:

| | |
|---|---|
| `default_url()` | `$SPACE_STATION_URL`, else the public Space Station |
| `default_home()` | `$SPACE_STATION_HOME`, else `~/.space-station` — for `Builder::home` and `daemon::Config`. The spool, lock and `auth.json` live there, and the socket too unless the path is too long, see above |

The URL defaults to the *public* host, so point it at a local stack explicitly rather than by
omission. Or skip the helpers and pass your own values to `Space::new`, `exchange`, `login` and
`SpaceClient::builder`.

## Next

[CLI](/docs/cli) · [Credentials](/docs/credentials) · [SQL](/docs/sql) ·
[Space Windows](/docs/space-windows) · [HTTP API](/docs/api)
