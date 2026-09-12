# Developing Space Station

What to know before changing the code. `UNDERSTANDING.md` is the intent, `docs/ARCHITECTURE.md`
is the shape every module was built against; when this file disagrees with either, they win.

## The layering, and the rule that follows from it

```
space-station-shared     limits, wire types, sanitizer, secret shapes — no I/O
space-station            the interface: every capability, one method each
space-station-cli        a clap tree over it, plus formatting
apps/web                 a subset of the same surface
```

**Nothing may exist in the CLI that is not first a method on `space-station`.** If a command
needs a new call, the call is written in `crates/client` and the CLI arm becomes argument parsing
and printing. The same rule holds for the app: it is a subset, not a sibling. Anything that needs
pixels stays in the app, and the package returns a URL for it (`window_url`) rather than growing a
terminal renderer.

The second rule is about state. **The package is stateless**: `Auth` is a value, `Space` is a
function of `(url, Auth)`, nothing in `crates/client` reads the environment or the home directory
on its own (the `default_*` helpers are the one, opt-in exception), and nothing writes. The one
thing that rotates — the Application session IAM issued, whose refresh token dies on reuse — is
held and refreshed by the backend alone; the package never sees it. The recording half keeps a
spool on purpose, in the home it is given. **The CLI is stateful**: `store.rs` owns
`~/.space-station/auth.json` and the org `use` chose, `main.rs` reads the `SPACE_STATION_*`
variables and opens the browser. A file read, an environment read or a prompt belongs in
`crates/cli`, never in `crates/client`.

The third rule is about credentials. **Never ask for a credential; always ask for a short-lived
token.** Nothing in the package, the CLI or the app prompts for, receives or stores a silicon's
`stk-`, an IAM bearer (`sat_`, `cat_`), a refresh token or the Application secret. A person or a
silicon obtains a short-lived token (`slt`) from IAM by their own means — the browser, `iam login
--app-id 'tos>spacestation' --org <org>`, `iam silicon-login --app-id 'tos>spacestation'` — and hands
it over; the backend exchanges it once (`POST /api/v1/app-auth/tokens`, HTTP Basic with the
canonical Application id, a form body) and holds the resulting Application session itself. The
backend is the **only** thing in the repo that talks to IAM, and it does so through the official
**`silicon-iam-client`** (1.4.1): `crates/backend/src/iam/client.rs` builds it with
`auto_update(false)` — the crate would otherwise run `cargo update` on *our* manifest at runtime —
and an explicit `User-Agent` (IAM's edge answers an HTML 403 without one), and wraps the calls the
contract in `docs/ARCHITECTURE.md`, "Identity", names: a fail-closed version handshake at boot,
exchange, refresh, introspection (with the `authorization` snapshot), revocation, and the webhook
verifier. The published crates carry no IAM dependency at all.

## Who owns what

| | |
|---|---|
| `crates/shared` | the limits, the ingest wire types, the sanitizer, the secret regex. No I/O. Everything depends on it; it depends on nothing of ours |
| `crates/client` | the interface. Recording: `SpaceClient` (`record.rs`), the spool (`spool.rs`), the daemon (`daemon.rs`). Managing: `Auth` (`auth.rs`) and `Space` (`api.rs`), with the returned shapes in `types.rs` and the Node host in `windows.rs`. Plus `runtime/mission-control.js`, vendored. Sync, unix-only: `std` + `libc` + `tungstenite` + `ureq` |
| `crates/cli` | the `spacestation` binary: one clap tree and one match (`main.rs`), the state the package refuses to hold (`store.rs`: `auth.json` under `flock`, tmp + rename), and the formatting (`out.rs`: aligned columns for `ls`, JSON otherwise, secrets alone on stdout). No HTTP of its own |
| `crates/backend` | one module per idea in ARCHITECTURE: `ingest`, `store` (`clickhouse`, `flush`, `lease`), `sql` (the guard), `triggers`, `live`, `windows`, `tables`, `tokens`, `webhooks`, `notifications`, `iam` (`client`: the one IAM client; `session`: the rows behind `ss_session` and `sscli-`; `webhook`: the receiver that feeds the `iam_members` mirror), `access`, `dev_errors`, `http`; `iam_stub` and `examples/iam-stub.rs` for development. tokio |
| `packages/space-station` | `mission-control.js`: one file, three roles (host, processor, renderer) and the Node CLI, plus `bin/space-station-dev.js` |
| `apps/web` | the SolidJS + Vite app, and `docs/*.md` — the user-facing documentation served at `/docs` |

Add a module only when ARCHITECTURE names one; reach for a new dependency last.

## Build, test, lint

```sh
export CARGO_TARGET_DIR=$PWD/target/mine    # one per person or worktree: cargo locks the target dir
cargo fmt                                   # root rustfmt.toml, max_width 120
cargo clippy -p <crate> --all-targets       # clean, and no `allow` without a comment saying why
```

Edition 2024, MSRV 1.88 for the published crates — do not raise it there; the backend, which is
not published, asks for 1.98 because `silicon-iam-client` does (its `Cargo.toml` says so; the
workspace default stays 1.88). Node ≥ 22.13 for the runtime, `windows run` and the app.

| | |
|---|---|
| everything | `scripts/test.sh` — every group, what each covers, a results table, non-zero on a failure |
| shared | `cargo test -p space-station-shared` |
| the package | `cargo test -p space-station` — the spool and the daemon, `Auth`, and the `Space` calls |
| backend units | `cargo test -p space-station-backend --lib` — guard, access, cron, crypto, ingest, flusher; no services |
| backend end to end | `cargo test -p space-station-backend --test '*'` — needs the docker stack; brings up its own IAM stub on its own port and signs its own IAM webhooks |
| the CLI | `scripts/sync-runtime.sh && cargo test -p space-station-cli` — `auth.json`, a login through a fake browser, an slt exchange that never prompts |
| the runtime | `npm --prefix packages/space-station test` — real Node children, a fake backend on 127.0.0.1 |
| the app | `npm --prefix apps/web run check-all` (tsc + build + unit tests) |

**One backend at a time.** A running server and the integration test both take `lease:engine` in
the local Redis, and only the holder flushes and fires notifications — the other looks alive and
does nothing. Stop `cargo run -p space-station-backend` before `--test '*'`, and never run two at
once. They scope themselves to a random org and wipe nothing, so leave the services up; the
`#[ignore]`d `throughput` test in `tests/core.rs` measures the ingest path's records per second.

The backend says which it is. At `info` (stderr; `RUST_LOG` filters): `space station listening
on <addr>`, then `engine lease acquired: this instance flushes and fires notifications` for the
holder or `engine lease held by another instance; this one is passive` for the other; `engine
lease lost: the flusher and the engine stop here` when it loses it and the same `engine lease
acquired` line when it wins it back; and, on taking the lease, `engine lease taken with <n>
staged rows waiting; re-landing interrupted flush <id> of <m> rows first` when a `flushing` key
was left by a process that died mid-INSERT (or `…; draining them` when only staged rows wait; a
clean handover says nothing). A second backend on the same `PORT` stops at once with `cannot
listen on 0.0.0.0:8080: address already in use` — the address is in the message, so a private
stack's `PORT` mistake is one line to read. Nothing logged carries a token or a key.

## The local stack, and logging in

The terminals are in `infra/local/README.md`. `cp .env.example .env` is for the backend:
`config::from_env` reads that file from the process's working directory and puts it *underneath*
the real environment, so run the server from the repo root and an exported variable still wins.
Nothing else in Rust reads it — the package reads no variable at all, and the CLI takes
`SPACE_STATION_URL`, `SPACE_STATION_HOME`, `SPACE_STATION_ORG` and the rest from the environment
alone, the URL defaulting to the public host — and on the JS side only `space-station-dev` does,
from its `--dir`.

The compose file maps Postgres to host port **5433** (`DATABASE_URL` in `.env.example` says
so) because a Postgres installed on the machine itself usually owns 5432, and a backend that
quietly connected to that one would create its schema in the wrong database. ClickHouse and Redis
keep their usual ports.

The stub (the library module `crates/backend/src/iam_stub.rs`, run by
`examples/iam-stub.rs` and started in-process by the integration tests) is the live IAM contract
on a loopback port, route by route as the real service answered in its testing environment: the
version handshake, `GET /api/v1/login` (a page listing the seeded carbons, or `?as=<carbon>`) that
302s back with `?slt=`, the carbon and silicon logins, `POST /api/v1/app-auth/short-lived-tokens`,
`POST /api/v1/app-auth/tokens` (single-use slt, rotating refresh, reuse fatal), introspection with
the `authorization` snapshot carrying the seeded tags, revocation, and directory routes that
answer `403 forbidden` to an Application token exactly like the real thing. It seeds from `crates/backend/fixtures/iam-seed.json` (`IAM_STUB_SEED` overrides
the file, `IAM_STUB_ADDR` the address) and prints what it seeded:

```
application       tos>spacestation  (HTTP Basic user; the ask_ secret is in the seed)
carbon            @alice        Alice  acme:owner[sales] tos:owner[tech]
carbon            @bob          Bob  tos:member[ops]
silicon           @bot:tos      Bot  tags: ops  stk: stk-0123456789abcdef0123456789abcdef
```

The tags in brackets are what access lists are matched against; the snapshot at login is how the
backend learns them (next section).

Both kinds of identity sign in from a terminal, and both need the CLI pointed at the local backend
— unset, `SPACE_STATION_URL` means the public Space Station. A session is bound to one org, so
`login` and `auth` need `--org` (or `$SPACE_STATION_ORG`, or the org stored by `use`):

```sh
export SPACE_STATION_URL=http://localhost:8080

cargo run -p space-station-cli -- login --org tos         # a carbon: the stub's page lists alice and bob
cargo run -p space-station-cli -- auth "$slt" --org tos   # anyone: a short-lived token IAM minted for the Application
```

`login` binds a loopback port, sends you through the backend to IAM's login page, receives the
short-lived token on the redirect (`?slt=…&state=…`) and spends it at `POST /api/auth/session` —
the `sscli-` session never appears in a URL. `auth` takes an slt and never a credential; the slt comes from
the `iam` CLI (`iam login --app-id 'tos>spacestation' --org tos`, `iam silicon-login --app-id
tos>spacestation`) against the real IAM, and from two curls — or the same `iam` CLI with `--url
http://127.0.0.1:8099` — against the stub, both in `infra/local/README.md`. Either way the
credential lands in `~/.space-station/auth.json` (0600). `SPACE_STATION_HOME` moves that
directory, which is how to keep a test identity — and a test spool — out of your own.

In a browser: open http://localhost:3000, pick an org, and the stub's page lists the seeded
carbons — pick one, or add `&as=alice` to that URL to skip the page. The session is the
`ss_session` cookie, so `SS_ORIGIN` must be the page's own origin or every mutating request and
every WS upgrade is refused. A terminal session and a browser session are separate rows on the
same account: signing out of one leaves the other alone.

**Two stacks, one browser: cookies are per host, never per port.** A second backend + app on
`localhost:3100` sets the same `ss_session` cookie for `localhost` as the one on `localhost:3000`,
and every callback on one signs the other out (the callback ends the session the presented cookie
belonged to, by design). Run the second stack on a different host literal — `127.0.0.1` or `[::1]`
— with `SS_ORIGIN` to match (`SS_ORIGIN=http://127.0.0.1:3100`), its own `PORT`, its own Postgres
database and its own Redis db index; the browser then holds one cookie per stack.

## Tags come from introspection; webhooks update them

IAM tells an Application nothing about its directory — `/me`, `/organizations`, `directory/*`,
tags and search all answer `403 forbidden` to an Application token — but the one question it does
answer, introspection, answers fully: an org-bound access token introspects with an
`authorization` snapshot (`public_id`, `principal_id`, `membership_id`, `org_role`, `tags`,
`authorization_epoch`, …). `iam::session` introspects at every login and whenever the last check is
older than a minute, and writes the snapshot into the session row and into the `iam_members`
mirror, so `whoami` shows the right tags from the first command (`tags: null` means IAM withheld
them and the mirror's tags stand). On `active: false` it **refreshes first** — IAM flips a held
token to inactive on a tag change or a sibling family's revocation while the member is fine — and
ends the session only when the refresh is refused.

**Webhooks** are the asynchronous updates between two checks. `iam::webhook` verifies each
delivery (`v1=hex(hmac_sha256(secret, "{timestamp}.{raw body}"))`, five minutes of clock
tolerance, current and previous secret, event-id dedup) and upserts `data.current.members[]` into
the mirror in one transaction. A removal event carries **tombstones** rather than member rows —
`{"authorization": "removed", "resource": {id, principal_id, principal_type, status, type,
version}}` — resolved to (org, actor) by membership id or principal id, which is why the mirror
and `access_tokens` store both; the actor then loses their tokens, sessions and delivery webhook
in that org. Membership itself never comes from the mirror — the login proved it and introspection
re-proves it.

The stub sends nothing on its own. `POST /_stub/deliver` signs and posts the event IAM would,
tombstones included for a `.removed.v1` event type (`infra/local/README.md` has the curl); the
integration tests sign deliveries themselves with the same HMAC and post them to the receiver.

## Pointing at the real IAM

Silicon IAM has **testing environments**: the same service and contract on isolated data,
selected by the `X-Testing-Environment-Key` header, with `000000` as every verification code.
`SILICON_IAM_TEST_KEY`, when set, is sent on every IAM request and switches the webhook receiver to
the `{"test": {testing_key, metadata, data}}` envelope, accepted only when the key matches. The key
is root authority over the environment: env or secret store only, never a URL, a log line, a test
name, a fixture or a report.

Everything for it — `SILICON_IAM_URL`, `SILICON_IAM_AUTH_URL` (`https://auth.iam.teamofsilicons.com`
for hosted browser login), `SILICON_IAM_APP_ID` (quoted: `'tos>spacestation'`, or
`. ./.env.test` creates a file named `spacestation`), `SILICON_IAM_APP_SECRET`,
`SILICON_IAM_WEBHOOK_SECRET`, `SILICON_IAM_TEST_KEY` (the key), `SILICON_IAM_TEST` (the
environment id, what `iam --test` takes) and `SS_ORIGIN` — lives in `.env.test` (mode 0600), which
`.gitignore` keeps out of the repo (`.env.*`). Load it over `.env` when you need it and never
print, echo, commit or paste its contents anywhere, this file included:

```sh
set -a; . ./.env; . ./.env.test; set +a
cargo run -p space-station-backend
```

The test environment uses the same browser and terminal login paths as production. IAM's consent
screen lets a carbon choose one or more organizations; `+` in the sidebar starts that flow again:

```sh
iam --test "$SILICON_IAM_TEST" login --email <you> --code 000000        # once; ~3 logins per carbon per 10 min
iam --test "$SILICON_IAM_TEST" login --app-id 'tos>spacestation' --org tos -o json   # reuses that session: prints the slt
spacestation auth <slt> --org tos
iam --test "$SILICON_IAM_TEST" silicon-login --app-id 'tos>spacestation'             # a silicon (stk at the prompt, or a stored session)
```

Registering the Application is done once, by an org owner, with the `iam` CLI (1.5.0). There are no redirect URIs to register. A login names its redirect URI, requests the app's declared
IAM permissions, and lets the person choose organizations; the webhook subscribes to the full event scope. The webhook secret is **caller-chosen**, 32–512
non-whitespace ASCII characters; IAM never generates it. The `ask_` secret is shown once:

```sh
iam app create spacestation --name "Space Station" \
  --base-url https://spacestation.teamofsilicons.com \
  --webhook-url https://spacestation.teamofsilicons.com/webhooks/api/ \
  --webhook-secret "$SILICON_IAM_WEBHOOK_SECRET"
```

In a testing environment the production Application is mirrored with a fresh test-only secret, and
its webhook pointed wherever the test backend is reachable (IAM must resolve and reach a public
HTTPS URL, so a tunnel in development):

```sh
iam --test "$SILICON_IAM_TEST" app import 'tos>spacestation'          # quoted: `>` is a redirect to the shell
iam --test "$SILICON_IAM_TEST" app set-webhook 'tos>spacestation' \
  --webhook-url https://<public host>/webhooks/api/ --webhook-secret "$SILICON_IAM_WEBHOOK_SECRET"
```

`--test` takes the environment's id (never its key), also read from `SILICON_IAM_TEST`. Three
things about the `iam` CLI to respect: give a script its own home (`SILICON_IAM_HOME`, mode 0700,
with `SILICON_IAM_AUTO_UPDATE=false`) rather than your own — 1.2.x locks the store, so parallel
commands are safe, but a script's sessions do not belong next to yours; the CLI updates itself
after a command unless told not to; and the Application's base URL must be a public `https://`
origin — a loopback one is refused at IAM's edge.

## The vendored runtime

`crates/client/src/windows.rs` embeds the runtime with
`include_str!("../runtime/mission-control.js")` and `cargo package` cannot reach outside the
crate, so the published package carries a copy of `packages/space-station/mission-control.js`.
It lives with the crate that spawns Node — the client, since `run_window` and `window_tool` are
methods there and the CLI only calls them.

`scripts/sync-runtime.sh` makes the copy. Run it after every runtime change and commit the copy,
or `spacestation windows run` ships a version nobody else has. `apps/web` needs no copy:
`app/mission-control.js/route.ts` serves the package file straight from the checkout.

## Publishing

Versions live in one place each: `[workspace.package] version` in the root `Cargo.toml` for the
crates, `packages/space-station/package.json` for npm. Bump, then publish in dependency order —
unchanged, because the layering is the dependency order: crates.io resolves the `version` next to
each `path` dependency, so a crate cannot go out before the one under it.

```sh
scripts/sync-runtime.sh                                     # first: the package bakes in whatever is there
git commit -am "…"                                          # cargo package refuses a dirty tree

cargo package --workspace --exclude space-station-backend   # pre-flight: builds all three as published
npm pack ./packages/space-station --dry-run                 # pre-flight: the published file list

cargo publish -p space-station-shared
cargo publish -p space-station                              # once shared is live on crates.io
cargo publish -p space-station-cli                          # once the package is live
npm publish ./packages/space-station --access public
```

`cargo package -p space-station` alone fails until `space-station-shared` is on crates.io — it
resolves the path dependency against the index — so the pre-flight is `--workspace`, which builds
the three against each other. Check that the client's `.crate` really carries
`runtime/mission-control.js`, and that npm lists nothing beyond `package.json` and the `files`
entries.

`space-station-backend` is `publish = false` and `apps/web` is private; neither goes out.

## House style

- Less code. If it can be done in less, do it in less — smaller code is reliable code.
- A module doc comment at the top of every file saying what it owns and why: prose, not a label.
  It is the first thing the next person reads.
- Event/callback driven. Sync where the task is simple (the package, the daemon, the CLI), tokio
  where it is not (the backend). On the client side errors are events (`on_error`), never panics.
- `std` and the dependencies already here before a new one.
- Tests grouped by what they test, named for what they assert, real behaviour over mocks: a real
  Node `--permission` child, real ClickHouse, the real client crate in the backend's own test.
- No dead code, no silent TODOs, nothing commented out. Delete it; git remembers. Never log a
  secret, and never widen a limit anywhere but `shared::limits`.
- Writing it once is v0.0. Iterate: smaller, faster, more reliable, more elegant, maintainable.
- The codebase is a form of art.
