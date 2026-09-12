# Space Station

Space Station is an observability tool. An app sends JSON **records** to a **table** inside an
**organization**; Space Station keeps them in order and hands them back through read-only SQL.
Org → tables → records, where a table is logical: one ClickHouse table holds everything and the
backend rewrites `FROM orders` to your org and that table id, so the separation is a mirage made
at query time. On top of the records sit **space windows** — a JavaScript processor turns rows
into one small JSON document (SiliconJSON, at most 64 KB) that a sandboxed iframe renderer draws,
live — and **notifications**, the same queries running always-on, ending in a message instead of
a view. It is for carbons (people) and silicons (machine identities) in a
[Silicon IAM](https://backend.iam.teamofsilicons.com/docs/client/) organization.

## The crate is the interface

```
space-station          the Rust package: every capability, one method each     [crates.io]
  └─ space-station-cli the `spacestation` binary: a clap tree over it         [crates.io]
apps/web               a subset of it, and the only place with pixels
```

Everything Space Station can do is a method on the crate: orgs, tables, space windows,
notifications, webhooks, access tokens, API keys, read, write, patch, delete, and the recording
path itself. The CLI adds argument parsing and output formatting and **not one capability the
crate lacks** — if it is in the CLI it is a method first. The web app is a subset of the same
surface. Where something genuinely needs pixels — a live renderer, a graph — the crate returns a
URL into the app and the CLI prints it, instead of inventing a terminal renderer. Carbons and
silicons both get all of it; the differences left are how they sign in, and where a notification
finds them.

The split is also one of state. The crate is stateless: `Auth` is a value the caller builds,
`Space` is a function of `(url, Auth)`, nothing reads the environment or the home directory, and
the one thing that rotates — the Application session IAM issued — lives in the backend, which is
the only party that ever holds it. The CLI is the stateful shell: it owns
`~/.space-station/auth.json`, the current org, and the browser.

## The smallest real thing

```toml
[dependencies]
space-station = "0.1"
```

Install the CLI on macOS or Linux (Intel/x86_64 and ARM64), without Rust or sudo:

```sh
curl -fsSL https://spacestation.teamofsilicons.com/install.sh | sh && export PATH="$HOME/.local/bin:$PATH"
```

Then run `spacestation login --org tos`. The installer verifies SHA-256 checksums, sets up
PATH for future terminals, and installs a private Node.js runtime if needed for window commands.
The Rust library is also published on crates.io.

Open [Space Station](https://spacestation.teamofsilicons.com) to sign in with Silicon IAM.
The CLI and crate default to `https://backend.spacestation.teamofsilicons.com`.
Install the JavaScript development server for Space Windows:

```sh
npm i -g @teamofsilicons/space-station
```

Recording — a table key, a background daemon, nothing blocking:

```rust
use space_station::SpaceClient;

let ss = SpaceClient::new("table-orders-…")?;                   // the key shown once when the table was created
ss.record(serde_json::json!({"id": "o-42", "amount": 12.5}));   // non-blocking, never panics
```

Managing — an identity, an org, and typed calls:

```rust
use space_station::{Auth, Space};

let url = space_station::default_url();
let auth = space_station::exchange(&url, slt, "tos")?;         // slt: what `iam login --app-id` / `iam silicon-login --app-id` printed
let space = Space::new(&url, auth)?.org("tos");
let key = space.create_table("orders", &["@alice", "tech"])?;   // the table key, shown once
let rows = space.query("SELECT count() FROM orders", &Default::default())?;
let link = space.window_url("w_01")?;                           // a window id; pixels live in the app
```

`Auth` is who you are, as a value: `Auth::session` for a carbon or a silicon signed in from a
terminal (from `space_station::login(url, org, visit)`, the loopback half of a browser sign-in, or
`space_station::exchange(url, slt, org)`, which hands the backend a short-lived token), and
`Auth::access_token` and `Auth::api_key` for code. Nothing takes an IAM bearer, a refresh token or
a silicon's long-lived `stk-` token, and nothing in the crate talks to IAM. `Space` is one method
per operation, synchronous, one request and one answer each.

`record()` stamps metadata, sanitises the value and hands it to the machine's daemon, which
spools it to disk and ships it. What arrives:

```json
{"metadata": {"record_id": "8b1e…", "table_id": "orders", "event_ts_ms": 1725000000000,
              "system": {"hostname": "…", "os": "…", "arch": "…", "cpu": "…", "cores": 8, "ram_mb": 16384},
              "cpu_pct": 12.5, "gpu_pct": null, "ram_pct": 41.0, "disk_free_mb": 120000},
 "record": {"id": "o-42", "amount": 12.5}}
```

The server adds two columns when the batch lands — `cursor` (per table, from 1, assigned once and
never reused) and `registered_ts_ms` — and from then on it is queryable, by a window, a
notification, the crate, the CLI or an API key:

```sql
SELECT count() AS n, sum(record.amount::Float64) AS revenue FROM orders
```

## How the pieces fit

```
app ──record()──▶ space-station ──unix socket──▶ daemon ──WS──▶ backend ──▶ redis ──flusher──▶ clickhouse
                  the Rust crate                 │ spool (disk)                     (1 s | 16 MB)

space-station ──HTTP──▶ backend                  every other call: tables, windows, notifications, keys
  ▲
  └── space-station-cli · apps/web               a clap tree and a browser over the same methods

query / subscribe ──▶ mission control ──▶ processor (JS) ──▶ SiliconJSON ──▶ renderer (iframe)
                                          untrusted: no credentials, no I/O but the bridge

trigger ──▶ notification (delay · sql · cooldown) ──▶ the app · a silicon's webhook · an org webhook
```

One daemon per machine carries every table key on it over one WebSocket, so many apps and orgs
cost one connection; nothing is batched on purpose, and nothing leaves the spool until the server
acks it. A carbon looks at the renderer; a silicon reads the SiliconJSON and calls the processor's
tools. Notifications run the same SQL under the same guard, always on.

## Who you are

Space Station is a Silicon IAM **Application** (`tos>spacestation`), and one Application serves
every organization. Signing in is the same act for everyone: IAM mints a **short-lived token**
(two minutes, one use) for this Application, and the backend exchanges it once for an Application
session that it alone holds and refreshes.

| who | how |
|---|---|
| a carbon in a browser | the app sends you to IAM's login page for the org you picked; IAM brings you back signed in |
| a carbon in a terminal | `spacestation login --org <org>` does the same through a loopback port — or `iam login --app-id 'tos>spacestation' --org <org>` prints the token and `spacestation auth <slt> --org <org>` spends it |
| a silicon | `iam silicon-login --app-id 'tos>spacestation'` prints the token; `spacestation auth <slt> --org <org>` spends it |

A session is bound to exactly one org; another org is another login, which IAM completes without
a prompt while its own session is good. Space Station never sees an IAM bearer of any kind — not a
silicon's `stk-`, not a `sat_` or a `cat_`, not a refresh token, not the Application's `ask_`
secret outside the backend — and nothing in the crate or the CLI ever prompts for one. The backend
speaks to IAM through the official `silicon-iam-client` (1.2.1, with its automatic self-update
disabled; exchange, refresh, introspection, revocation, webhook verification — see
`docs/ARCHITECTURE.md`, "Identity"), which is why the backend needs Rust 1.98 while the published
crates stay at 1.88 and carry no IAM dependency.

An Application may read nothing about the directory, but IAM tells it who just signed in: the
introspection of an org-bound session carries the member's id, membership and **tags**, so tags are
known the moment anyone signs in and re-read every minute — no waiting for a webhook. IAM's
webhooks are the asynchronous updates in between (a tag renamed, a member removed), kept in a
mirror the backend also writes at every login. Access lists (`["@alice", "@bot:tos", "tech"]`) are
matched against that.

## What is in the repo

```
Cargo.toml                       rust workspace, edition 2024, MSRV 1.88 (backend: 1.98)
crates/
  shared/    space-station-shared   limits, sanitizer, wire types, secret shapes   [crates.io]
  client/    space-station          the interface: SpaceClient + daemon, Auth + Space,
                                    and runtime/mission-control.js, vendored for publishing
  cli/       space-station-cli      the spacestation binary: one clap tree over the crate
  backend/   space-station-backend  the server (axum): ingest, flusher, the SQL guard, identity
                                    (the official silicon-iam-client), mission control, notifications;
                                    src/iam_stub.rs + examples/iam-stub.rs, the local IAM look-alike
packages/
  space-station/                   @teamofsilicons/space-station: the mission_control runtime
                                   (CDN) + the space-station-dev dev server              [npm]
apps/
  web/                             SolidJS + Vite frontend and the documentation at /docs
infra/local/                       docker compose: clickhouse, postgres, redis
scripts/test.sh                    every test group, then a results table
docs/ARCHITECTURE.md               the contract every module is built against
```

Sync where the task is simple (the crate, the daemon, the CLI), tokio where it is not (the
backend). The backend and the app are not published.

## Production

The live deployment and verification evidence are in [docs/PRODUCTION-READINESS.md](docs/PRODUCTION-READINESS.md).
Native infrastructure, deployments and recovery are documented in [infra/production/README.md](infra/production/README.md).

## Run it locally

From this checkout, the one-command v1 launcher builds the backend and CLI, starts the local
IAM fixture and web app, and waits for readiness:

```sh
python3 scripts/dev.py start
# Open http://localhost:3000 → organization tos → Alice
python3 scripts/dev.py status
python3 scripts/dev.py stop
```

Requires Rust 1.98+, Node 22.13+, Docker and `psql`. It uses a separate Postgres database
(`space_station_v1`) and Redis database (`6`) so integration tests can run alongside it;
ClickHouse records persist in the shared local store. It does not replace `.env` or the
user's CLI login. Logs and its persistent encryption key live in gitignored `.local/run`.
`CARGO_TARGET_DIR` can reuse an existing build directory. The terminal prints the built CLI
path; set `SPACE_STATION_URL=http://localhost:8080` when using it. This launcher uses fixture
IAM identities; real IAM configuration is described below.

To start each service yourself:

```sh
docker compose -f infra/local/docker-compose.yml up -d --wait   # clickhouse :8123, postgres :5433, redis :6379
cp .env.example .env

# then, in three terminals, from the repo root
cargo run -p space-station-backend --example iam-stub           # IAM look-alike on 127.0.0.1:8099, prints the logins
cargo run -p space-station-backend                              # the server on :8080, creates the schema
npm --prefix apps/web install && npm --prefix apps/web run dev  # the app on :3000, log in as alice or bob
```

Postgres is on host port 5433, not 5432, because a Postgres already installed on the machine
usually owns 5432; `.env.example` says so. A second stack on the same machine needs its own host
literal, not just its own ports — cookies are per host — so run it on `127.0.0.1` or `[::1]`
(`infra/local/README.md`).

Then sign in from a terminal. A carbon opens a browser and gets the short-lived token back on
loopback, which the CLI spends for a session; a silicon hands over a short-lived token it got from
IAM (here, the stub) by its own means:

```sh
export SPACE_STATION_URL=http://localhost:8080                # else the CLI talks to the public Space Station

cargo run -p space-station-cli -- login --org tos             # a carbon: pick alice or bob on the stub's page
cargo run -p space-station-cli -- auth "$slt" --org tos       # a silicon: an slt the stub minted (two curls, in infra/local/README.md)
cargo run -p space-station-cli -- whoami
```

`.env` is read by the backend, from its working directory and underneath the real environment, and
by `space-station-dev` from its `--dir`. By nothing else: the stub and the app have local defaults,
and the CLI's host comes from the environment, where it defaults to the *public* one — hence that
`export`.

Details, and how to point at IAM's testing environment instead of the stub, in
[infra/local/README.md](infra/local/README.md) and [docs/DEVELOPING.md](docs/DEVELOPING.md).

## Test it

```sh
scripts/test.sh
```

It runs every group, says what each covers, and ends with a results table: `shared` (limits,
sanitizer, secret scanner, table keys), `client` (spool and seq, daemon election, batch building,
acks and rejections, `Auth` and the `Space` calls, publish pre-flight, the embedded runtime and
node spawn), `backend` (SQL guard, access lists, cron, crypto, ingest and flusher units, the IAM
stub, client and sessions, the webhook receiver), `backend-integration` (the whole server against
the docker services and the IAM stub: every login door, ingest → flush → query → trigger →
notification → webhook, the IAM webhook into the directory mirror and a removal tombstone), `cli`
(the clap tree over the crate, `auth.json`, a login through a fake browser, an slt exchange that
never prompts, output shapes), `runtime`
(queue semantics, delta/snapshot, the tool type table, the sandbox constant, the 64 KB bound) and
`web` (typecheck, build, and the unit tests for the dev panel, one-time secrets, notification
definitions and the agent prompt). One backend at a time: the integration test takes the Redis
engine lease.

## Documentation

The user-facing docs are `apps/web/docs/*.md`, served at `/docs`: getting started, space windows,
SQL, notifications, the Rust package, the CLI, credentials, the HTTP API. For the internals,
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) is the contract every module was built from,
[UNDERSTANDING.md](UNDERSTANDING.md) is the intent behind it, and
[docs/DEVELOPING.md](docs/DEVELOPING.md) is for changing the code.

## Status

The completed local v1 acceptance, manual user walkthroughs, startup commands and remaining
public-login dependency are recorded in [V1 readiness](docs/V1-READINESS.md).

What is built is what `scripts/test.sh` covers, end to end against the local stack, and the
identity path has also been run against Silicon IAM's testing environment by hand. There is no AWS
deployment yet; `infra/local` is the development environment. Registering the Application with IAM is
in `docs/ARCHITECTURE.md`, "Identity", and in `docs/DEVELOPING.md`.

[MIT](LICENSE).
