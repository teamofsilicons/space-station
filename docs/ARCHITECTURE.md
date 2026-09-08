# Space Station — architecture

The contract every module is built against. `UNDERSTANDING.md` is the intent; this is the
shape. When they disagree, `UNDERSTANDING.md` wins and this file gets fixed.

## Repository

```
Cargo.toml                       rust workspace
crates/
  shared/    space-station-shared   limits, sanitizer, wire types, secret shapes      [crates.io]
  client/    space-station          the interface: SpaceClient + daemon, Auth + Space;
                                    runtime/ is the vendored mission-control.js it embeds  [crates.io]
  cli/       space-station-cli      the `spacestation` binary                        [crates.io]
  backend/   space-station-backend  the server (axum); src/iam_stub.rs + examples/iam-stub.rs, the local IAM
packages/
  space-station/                   @teamofsilicons/space-station: mission_control runtime (CDN)
                                   + `space-station-dev` dev server + publish           [npm]
apps/
  web/                             SolidJS + Vite frontend + docs
infra/local/                       docker compose (clickhouse, postgres, redis)
scripts/test.sh                    every test group, then a results table
scripts/sync-runtime.sh            copies packages/space-station/mission-control.js into crates/client/runtime/
```

Rust 2024 edition, MSRV 1.88 for the published crates; the backend, which is not published, needs
1.98 because `silicon-iam-client` does. Sync where the task is simple (client library, daemon,
CLI); tokio where it is not (backend).

## Vocabulary

| word | meaning |
|---|---|
| org | an IAM organization. Everything is scoped to one. |
| carbon | a human. Public id `alice` (`^[a-z0-9_-]{3,30}$`, no colon) |
| silicon | a machine identity. Public id `bot:tos` (`handle:org_id`, always has a colon) |
| actor | a carbon or a silicon. Stored and sent **without** `@`; UIs prepend `@` for display |
| table | a logical table inside an org. `table_id` is `^[a-z0-9]{1,50}$`, unique in the org |
| record | one JSON object sent to a table |
| cursor | per (org, table) `UInt64` starting at 1, assigned once, by the flusher, when a batch lands in ClickHouse |
| watermark | per (org, table): while this process holds the engine lease, the `to` of its most recent successful flush; otherwise `max(cursor)` from ClickHouse, re-read at most once a flush interval and never allowed to move backwards. Every watermark handed out uses this value, never the counter |
| space window | processor (JS) → SiliconJSON (≤ 64 KB) → renderer (iframe) |
| version | one named pair (processor, renderer) of a window; `metadata.processor_version` and `renderer_version` are both this name, or `dev` |
| access list | `["@alice", "@bot:tos", "tech"]`: `@` + actor id, `webhook:` + webhook id (recipients only), otherwise a tag name, matched exactly and case-sensitively against the actor's IAM tag names. Union. **Matched, never validated**: the backend cannot ask IAM which tags or members exist, so a mistyped entry (`Tech`, `@alcie`) is accepted and grants nothing. Renaming a tag in IAM changes access, by design |

Whoever creates something is its `created_by` and is appended to its access list at create time;
whoever writes an access list is appended to it too, so a table, window or notification can never
be orphaned and every access check is one membership test: *the list contains the actor or one of its
tags*. Reads and writes (PUT, POST versions, DELETE, subscribe, `state`) use the same check; list
endpoints filter by it.

All size limits are **UTF-8 byte lengths of the JSON text as sent** (runtime:
`new TextEncoder().encode(JSON.stringify(v)).length`; Rust: `serde_json::to_vec(v).len()`).
Receivers bound the received text; nobody re-serialises to decide.

## Credentials

| shape | for | stored as | resolves to |
|---|---|---|---|
| `table-{table_id}-{32hex}` | the client library / daemon | sha256, shown once | (org, table) |
| `spacewindow-{32hex}` | access token: dev server + CLI processors | AES-256-GCM under `SS_KEY` (viewable), plus sha256 for lookup, `last_used_at` | (org, actor). One live per (org, actor); rotatable |
| `apikey-{32hex}` | programmatic read of tables / notifications | sha256, shown once | (org, scopes). No actor: acts for the org within its scope |
| `whsec-{32hex}` | signing outgoing webhooks | AES-256-GCM under `SS_KEY` | shown once per (re)creation |
| cookie `ss_session` | the browser | 32 random bytes; row keyed by sha256; holds the Application `oat_`/`ort_` encrypted | an actor, bound to one org |
| `sscli-{…}` | a terminal | the same session row, presented as a bearer | an actor, bound to one org |
| `slt` | what a person or a silicon hands Space Station to sign in | never stored; exchanged once at IAM within 2 minutes | a session |

Raw `Authorization: Bearer` is accepted only for `sscli-`, `spacewindow-` and `apikey-`. Space
Station never receives an IAM bearer of any kind: not a silicon's `sat_`, not a carbon's `cat_`,
not an `stk-`. What it receives is a **short-lived token** minted by IAM for this Application,
which it exchanges once for an Application session that it alone holds.

**Secret regex** (defined once, in `shared::secrets`):
`\b(spacewindow|apikey|whsec|table-[a-z0-9]{1,50})-[0-9a-f]{32}\b|\b(sat|cat|rft|oat|ort)_[A-Za-z0-9_-]{43}\b`.
The backend applies it to processor and renderer bodies on every version create
(`secret_in_code`); the dev server and `windows publish` run it as a pre-flight.

`SS_KEY` is 32 bytes (hex in env). Encrypted values are never logged.

## Identity (crate `backend`, module `iam`)

Written against the live Silicon IAM contract as observed on 2026-09-05 in a testing environment.
The crate named `silicon-iam` (now a `0.0.0` placeholder on crates.io) implemented a PKCE flow the
service no longer has; the maintained client is **`silicon-iam-client`** (1.2.1, Rust 1.98), which
types every call below: `Client::builder(url)?.credential(Credential::application(app_id,
secret)).environment(EnvironmentKey::new(key)?).auto_update(false).build()?`,
`system().negotiate()`, `oauth().login(app_id, slt, &Mutation::new())`, `oauth().refresh(app_id,
ort, &Mutation::with_key(k))`, `oauth().authorization(..)` → `Option<ApplicationAuthorization>`,
`oauth().introspect(..)`, `oauth().revoke(.., &mutation)`, and `webhook::{WebhookVerifier,
WebhookSecretKeyring, WebhookSecret}` with `VerifiedWebhook::verify_testing_environment`. The
backend uses it with **`auto_update(false)`** — the crate would otherwise run `cargo update`
against the host's manifest at runtime — and with an explicit `User-Agent` (IAM's edge answers an
HTML 403 to any request without one). There is one HTTP client, the official one, and no fallback.

**How anyone signs in.** IAM mints a *short-lived token* (`slt`, 2 minutes, single use) for an
Application, and the Application exchanges it. Three ways to obtain one, one way to spend it:

| who | how the slt is minted |
|---|---|
| a carbon in a browser | the backend redirects to `{IAM}/api/v1/login?app_id={APP}&redirect_uri={SS_ORIGIN}/api/auth/callback&org_id={org}`; IAM signs them in (OTP; `000000` in a testing environment) and 302s back with `?slt=` |
| a carbon in a terminal | `iam login --app-id {APP} --org {org}` prints it |
| a silicon | `iam silicon-login --app-id {APP}` prints it — the only way a silicon can sign in to an Application |

(The browser row is the contract, not today's reality: IAM's hosted login UI answers a 404 and its
edge refuses `GET /api/v1/login` with a loopback `redirect_uri`, so against the real service a
carbon currently signs in through the terminal rows; the stub serves the browser row locally. See
`docs/EXTERNAL-BUGS.md`.)

The backend spends it at `POST {IAM}/api/v1/app-auth/tokens` with HTTP Basic `{APP}:{ask_ secret}`,
an `Idempotency-Key`, and a **form-encoded** body `app_id={APP}&slt={slt}` (JSON is refused with
415). The answer, under `Cache-Control: no-store`, is
`{access_token: oat_…, refresh_token: ort_…, token_type: Bearer, expires_in: 1800, scope, actor:
{principal_id, type, public_id}, org_id}`. `org_id` is the organization the login was bound to and
is what Space Station scopes the session to; an unscoped login (no `org_id`) is refused by Space
Station with `org_required`, because everything here lives inside an org. A login bound to an
org the actor is not a member of never reaches us: IAM answers `organization_context_forbidden`.

`APP` is the canonical `{org}>{handle}` id, `tos>spacestation`. One Application serves every
organization: alice, a member of `acme`, signs in to `tos>spacestation` bound to `acme` and
introspection reports her `acme` membership.

**Refresh**: the same endpoint with `refresh_token=` instead of `slt=`, under the session row lock,
reusing the row's `refresh_key` as the `Idempotency-Key` until it succeeds. Rotation is
mandatory and reuse is fatal: presenting a consumed `ort_` answers `400 invalid_grant` and IAM
revokes the whole family, so the row is deleted on that code and kept on anything else.
**Introspection**: `POST /api/v1/oauth/introspect`, Basic, form `token=` → `{active, actor_type,
org_id, membership_id, principal_id, session_id, scope, audience, issued_at, expires_at,
authorization_epoch, authorization}` or exactly `{"active": false}`; do not send `X-Org-ID`.
**Revocation**: `POST /api/v1/oauth/revoke`, Basic, form `token=`, `Idempotency-Key`; unknown
tokens are 200.

**What an Application may read: nothing.** `directory/self`, `directory/members`,
`/organizations`, `/me`, tags, carbon search — every one answers `403 forbidden` to an
Application access token, silicon or carbon, whatever scopes it carries, by design. Introspection
is the only question IAM answers about a token, and since IAM 1.2.0 it answers it fully: an
**org-bound** access token introspects with an `authorization` snapshot `{principal_id,
public_id, actor_type, organization_id, org_id, membership_id, membership_version,
authorization_epoch, org_role, tags: [{id, name}], scopes, audience, testing_environment_id}`,
which the contract calls "a synchronous bootstrap/resynchronization snapshot; webhook snapshots
are asynchronous updates, not prerequisites for initial access". `tags` needs the
`memberships.read` scope (ours has it); `null` means undisclosed and `[]` means no tags. Refresh
tokens and unscoped tokens carry no `authorization`.

`Identity { kind: carbon|silicon (from the colon), id, org, tags: [name] }` is built as: `kind`,
`id` and `org` from the session row (set at exchange from `actor.public_id` and `org_id`), and
`tags` from the snapshot. The backend introspects at exchange and whenever `checked_at` is older
than 60 s, and each time writes the snapshot's `public_id`, `principal_id`, `membership_id` and
`tags` into the session row **and** into the directory mirror — so tags are known the moment
anyone signs in, without waiting for a webhook, and `tags: null` leaves the mirror's tags as they
were. A different `org_id` or `membership_id` ends the session. `active: false` does **not** end it
by itself: IAM flips a held access token to inactive when the member's `authorization_epoch`
moves (a tag or role change) or when a sibling family is revoked, while the member is still
active — so the re-check **refreshes first** and re-introspects the new token, and the session
ends only when IAM refuses the refresh (`400 invalid_grant`). `display_name` is gone: the public
id is the handle people know.

**The directory mirror** is one table, `iam_members(org, actor, membership_id, principal_id,
kind, status, tags jsonb, org_name, version, updated_at)`, written by introspection (the
synchronous snapshot at login and on every re-check) and by the webhook receiver (asynchronous
updates) from `data.current.members[]` rows. A non-removal event carries full rows: `membership
{id, status, tags: [{id, name}], version}`, `organization {org_id, name}` and `principal
{public_id, type}`. A **removal** event (`organization.membership.removed.v1`,
`organization.silicon.removed.v1`) carries **tombstones** instead — `{"authorization": "removed",
"resource": {id: <membership uuid>, principal_id, principal_type: silicon|carbon, status:
"removed", type: "organization_membership", version}}`, with no `principal`, `organization` or
`membership` object and no `removed_at` — which is why the mirror, `access_tokens` and `sessions`
carry `membership_id` and `principal_id`: a tombstone is resolved to (org, actor) by membership
id, else by principal id. Rows arrive on `organization.membership.created|reactivated|removed|
updated.v1`, `organization.silicon.created|removed|updated.v1` and the tag events. Nothing
arrives on activation, and nothing needs to: the snapshot at login is the bootstrap, and the
mirror only lets Space Station notice a change between two re-checks and list the orgs an actor
belongs to. Membership itself is never taken from the mirror; the login proved it and
introspection re-proves it. `org_name` feeds `GET /orgs`.

**Org list.** The sidebar shows the orgs where the mirror holds an active membership for the
actor, plus the session's own org, and an "add organization" affordance that starts an org-bound
login. A session is bound to one org; switching orgs is another login, which IAM completes
without a prompt when its own session is still good. A browser holds one session at a time: the
callback that opens the new one **ends the session the presented `ss_session` cookie belonged
to** (revoking its Application tokens) before it sets the new cookie, so switching org never
leaves an orphaned row behind. The terminal's `sscli-` row is a different row and is untouched.

**Token families, as IAM runs them today.** Two families minted from the same IAM login session
coexist and both refresh. A **new IAM login by the same actor** (a silicon calling
`silicon-auth/token` again, a carbon signing in on a second device) makes the older families'
refresh answer `400 invalid_grant` while their access tokens stay active until expiry. Because
every re-check **refreshes first** (below), an older Space Station session of that actor ends at
its **next re-check** — the first request it makes more than 60 s after its last check, so about
a minute after its next command, not at the access token's expiry — with `invalid_grant`; the
second device wins. (Observed live 2026-09-05 17:48, rid `01a07181-ff54-7363-9d8e-7c7019e3ef92`:
a fresh `iam silicon-login` for `bot:tos`, and the older terminal session's next command a minute
later answered `401`.) Revoking one family's refresh token flips sibling access tokens of the
same login to `active: false` although their refresh still works, which is the other reason a
re-check refreshes before it gives up.

**Webhook receiver** (`POST /api/iam/webhook`, also at the registered `/webhooks/api/`): read the
raw bytes; verify `X-Silicon-IAM-Signature: v1=hex(hmac_sha256(secret, "{X-Silicon-IAM-Timestamp}.{raw body}"))`
in constant time against `SILICON_IAM_WEBHOOK_SECRET` (and `SILICON_IAM_WEBHOOK_SECRET_PREVIOUS`
while a rotation drains), refuse a timestamp more than 5 minutes off. Two envelopes: production
is `{spec_version, event_id, event_type, occurred_at, organization_id, aggregate, data}`; a
testing environment sends `{"test": {testing_key, metadata: {…the same…}, data}}`, accepted
only when `testing_key` equals `SILICON_IAM_TEST_KEY` (constant time) and never logged. Then:
`INSERT event_id INTO iam_events` (duplicate → still 204); upsert every full `current.members[]`
row into the mirror (highest membership version wins); for each row whose `membership.status` is
not `active`, and for each **tombstone** row on `*.removed.v1` — resolved to (org, actor) through
the mirror by `resource.id` (membership) or `resource.principal_id`, and through `sessions` /
`access_tokens` by the same ids — revoke that actor's `access_tokens`, `sessions` and delivery
webhook in that org and drop them from every notification's recipients; answer 204 and log any
event that verifies but yields no rows. Everything else is ignored.

**Handshake at boot**: `GET {IAM}/api/version` with `Silicon-IAM-Supported-API-Versions: v1` must
answer `service: silicon-iam`, `selected_api_version: v1` and the header
`Silicon-IAM-API-Version: v1`, or the process exits. (This one route ignores the testing header.)

**Testing environments** are the same service and contract on isolated data, selected by the
`X-Testing-Environment-Key` header. `SILICON_IAM_TEST_KEY`, when set, is sent on every IAM request
and switches the webhook receiver to the test envelope. The key is root authority over the
environment: env or secret store only, never a URL, log, test name or fixture.

Config: `SILICON_IAM_URL`, `SILICON_IAM_APP_ID` (canonical), `SILICON_IAM_APP_SECRET` (`ask_`, 47
chars), `SILICON_IAM_WEBHOOK_SECRET` (caller-chosen, 32–512 chars; we use `whs_` + 43),
`SILICON_IAM_WEBHOOK_SECRET_PREVIOUS` (optional), `SILICON_IAM_TEST_KEY` (optional), `SS_KEY`,
`SS_ORIGIN`, `CLICKHOUSE_URL` (admin), `CLICKHOUSE_QUERY_PASSWORD`, `DATABASE_URL`, `REDIS_URL`,
`PORT`, `SS_ALLOW_PRIVATE_WEBHOOKS` (dev only: lets a webhook URL resolve to a loopback or private
address, and permits `http://` **only** to such hosts — a public `http://` URL is refused even with
it). `Config::from_env` reads `./.env` underneath the real environment.

Registering the Application (once, by an org owner with the `iam` CLI): `iam app create
spacestation --name "Space Station" --base-url https://spacestation.teamofsilicons.com
--webhook-url https://spacestation.teamofsilicons.com/webhooks/api/ --webhook-secret <whs_…>`.
There are no redirect URIs to register and no scopes to request; a login names its redirect URI.
The generated `ask_` secret is shown once. In a testing environment, `iam --test <env> app
import 'tos>spacestation'` mirrors the production Application with a fresh test-only secret (quoted
in a shell: `>` is a redirect).

**IAM stub** (`crates/backend/examples/iam-stub.rs`, also the library module `src/iam_stub.rs`
the integration tests start in-process): the contract above on a loopback port, seeded from a
JSON file of orgs, carbons (with tags per org) and silicons.
`GET /api/version` with the two headers; `GET /api/v1/login` renders a page listing the seeded
carbons (or `?as=<carbon>` skips it) and 302s to `redirect_uri?slt=`; `POST
/api/v1/app-auth/short-lived-tokens` (bearer = a stub `cat_`/`sat_` from its own login/silicon
routes) → `{slt, expires_in: 120}`; `POST /api/v1/app-auth/tokens` (Basic canonical id, form,
single-use slt, rotating refresh, reuse → `400 invalid_grant` + family revoked, `Cache-Control:
no-store`); `/oauth/introspect`; `/oauth/revoke`; `/login/challenges` + `/verify` with `000000`;
`/silicon-auth/token`. Introspection of an org-bound token carries the `authorization` snapshot
with the seeded tags. Directory routes answer `403 forbidden` to Application tokens, exactly like
the real service. `POST /_stub/deliver {url, org, event_type, members | tombstones, envelope?,
event_id?, version?}` signs and posts a webhook in either envelope so the mirror is testable
without a tunnel: `members: [{actor, tags?, status?}]` become full rows, and a `.removed.v1`
event type produces tombstone rows (`tombstones: [{actor}]`, or the `members` named) of the live
shape. Errors `{error: {code, message, request_id}}`. Listens on `127.0.0.1`.

## Ingest

```
app ──SpaceClient.record()──▶ unix socket ──▶ daemon ──WS──▶ backend ──▶ redis ──flusher──▶ clickhouse
                                                 │ spool (disk)                    (1 s | 16 MB)
```

**Library** (`crates/client`). `SpaceClient::new(table_key)` parses `table_id` from the key.
`record(value)` is non-blocking: it stamps metadata, sanitises, and pushes the line into a bounded
channel (10 000); when full the line is dropped and `on_error(queue_full)` fires. A sender thread
writes lines to the daemon's unix socket. If the connect fails (ENOENT/ECONNREFUSED) the
sender tries `flock(LOCK_EX|LOCK_NB)` on an open fd of `~/.space-station/daemon.lock`; the winner
unlinks a stale socket, binds, and runs the daemon on a thread in-process (same code path); a
loser reconnects. `Drop` and `flush()` block — at most 5 s (`FLUSH_TIMEOUT`, or
`Builder::flush_timeout`) — until the daemon **in this process** has had every line this client
recorded **acked** by the server (not merely spooled), so a one-shot program that records and
exits still delivers; when the daemon is another process, which outlives this one, `flush()`
waits only until the lines are in its spool. `flush()` returns `false` when the bound ran out
first, and the spool keeps the lines for the next daemon. Per-record rejections belong to the daemon that sent the batch: with
the in-process daemon they fire this client's `on_error`; with a separate `spacestation daemon`
they go to that process's log and `daemon status`, never back to the recorder. Errors are
events (`on_error`), never panics.

The socket is `<home>/daemon.sock` **when that path is under 104 bytes** — the macOS `sun_path`
limit, applied on Linux too although it allows 108 (`daemon::SOCKET_PATH_MAX`) — and otherwise `<system temp dir>/space-station-<16 hex of
sha256(home)>.sock` (`daemon::socket_path(home)`, one function used by the daemon, the client and
`daemon status`, which prints it). A deep `SPACE_STATION_HOME` (a per-test temp dir, a CI
workspace) would otherwise bind with `EINVAL`, which the daemon reports naming the path, its
length and the limit; with the fallback such a home costs nothing but the socket's location, the
spool and lock stay inside it, and two homes never share a daemon because the hash differs.

Metadata stamped by the library:

```json
{"record_id": uuid, "table_id": "orders", "event_ts_ms": 1725000000000,
 "system": {"hostname","os","arch","cpu","cores","ram_mb"},      // cached once
 "cpu_pct": 12.5, "gpu_pct": null, "ram_pct": 41.0, "disk_free_mb": 120000}
```

Sanitising (`shared::sanitize`): each string value > 32 KB is cut in the middle to
`"abc...[SIZE]...xyz"` keeping the first and last 16 KB minus the marker, snapped to UTF-8
boundaries, `SIZE` = original byte length; values that look like files (data URI prefix, magic
bytes, base64 shape, non-UTF-8 or control-char heavy) become `"[FILETYPE:SIZE]"`; a record still
> 256 KB is rejected locally (`size_exceeded`).

**Daemon** (`crates/client`, module `daemon`; also `spacestation daemon`). One per machine, any
number of table keys. Spool `~/.space-station/spool.jsonl`, one line per record
`{"seq","key","metadata","record"}`, plus `spool.cursor` holding the last acked seq. `seq` is
monotonic for the life of the spool: on boot `next_seq = max(cursor, last parseable seq) + 1`;
unparseable lines (including a trailing partial one) are skipped and counted, never fatal. Append
and truncate share one mutex; appends are write + flush (no fsync); when everything is acked and
the file is > 64 MB it is `ftruncate(0)` with `spool.cursor` untouched. `~/.space-station` is
0700; spool, cursor, auth.json and the socket are 0600.

Send loop: one in-flight batch; a batch is the oldest unacked lines regardless of key, up to
8 MB; on `ok` → `cursor = last seq`; on `rejected` → `duplicate` counts as acked silently,
`unauthorized`/`size_exceeded`/`invalid` are dropped and reported through `on_error`, everything
else acked. WS ping every 20 s; a batch without an ack for 30 s, or a missed pong, closes the
socket; on any disconnect the batch is rebuilt (with anything new) and resent under a new
`batch_id`. Reconnect with backoff 1 s → 30 s.

WS `wss://…/api/ws/ingest`, JSON text frames:

```json
→ {"batch_id": uuid, "records": [{"key": "table-orders-…", "metadata": {…}, "record": {…}}]}
← {"batch_id": uuid, "status": "ok"}
← {"batch_id": uuid, "status": "rejected", "rejected": [{"record_id": uuid, "code": "duplicate", "reason": "…"}]}
← {"batch_id": uuid, "status": "rejected", "code": "batch_too_large"}     // frame > 8 MB; socket closed
```

Codes: `unauthorized` (bad key: every record under it), `duplicate`, `size_exceeded`, `invalid`.
Records not listed in `rejected` were accepted.

**Backend** (`backend::ingest`). Keys resolve through an in-memory `sha256 → (org, table)` map
(refreshed from Postgres on miss and every 60 s); `metadata.table_id` is ignored. Per record:
byte bound (≤ 264 KB for the object) and `serde_json` parse → `size_exceeded`/`invalid`. Then
**one Redis `EVAL` per batch**: for each remaining record `if SET dedup:{record_id} NX EX 300 then
RPUSH staging <row> else rejected += record_id`. The ack is sent only after the script returns.
Any Redis/Postgres error while handling a batch closes the socket without an ack; the daemon's
resend is the retry.

**Engine lease.** Exactly one backend process runs the flusher and the notification engine: it
holds `SET lease:engine NX PX 5000`, renewed every 2 s, and stops draining and firing the moment
renewal fails. SIGTERM: stop accepting ingest batches, finish the in-flight flush, release the
lease. Horizontal scaling is out of scope; the lease exists for deploys.

**What an operator sees** (`tracing` at `info`, stderr, `RUST_LOG` filters it; each line once, on
the change): `space station listening on <addr>`; `engine lease acquired: this instance flushes
and fires notifications` when this process becomes the holder — at boot or when it wins the lease
back later, the same line; `engine lease lost: the flusher and the engine stop here` when a
renewal fails (it goes passive: it still serves HTTP and accepts ingest); `engine lease held by
another instance; this one is passive` when it boots while another holds it. Taking the lease
also says what the previous holder left behind, or nothing on a clean handover: `engine lease
taken with <n> staged rows waiting; re-landing interrupted flush <flush_id> of <m> rows first`
when a `flushing` key was found (re-inserted unchanged before anything new), else `engine lease
taken with <n> staged rows waiting; draining them`. A bind failure names the address and stops
the process: `cannot listen on 0.0.0.0:8080: address already in use` — the usual cause being a
second backend on the same `PORT`. Warnings cover retries (`flush failed, retrying: …`), a flush
ClickHouse refused (`flush <id> refused (…), inserting row by row`), unparseable staging rows
moved to `dead`, an ingest batch dropped without an ack, IAM refusals; nothing at `info` or above
ever carries a token, a key or a secret.

**Flusher** (`backend::store::flush`). Every 1 s or when `staging` reaches 16 MB: `LRANGE` the
head, build one `INSERT … FORMAT JSONEachRow`, assigning `cursor` from a per (org, table) counter
seeded at its first use with `watermark + 10000`, or 1 when ClickHouse holds no row for that
table yet (if the seed query fails the flush fails and rows
stay in Redis), and `registered_ts_ms = now`. Each drain gets a `flush_id` (uuid) reused on every
retry; before the INSERT write `SET flushing {flush_id, n, rows-with-cursors}`; send the INSERT
with `insert_deduplication_token={flush_id}`; on success `LTRIM staging n -1`, `DEL flushing`,
broadcast `Flushed{org, table, from, to}` per touched table. On boot or lease acquisition, if
`flushing` exists, re-INSERT it unchanged (ClickHouse drops a block it already has), then
LTRIM/DEL. Connection and timeout errors retry the same flush every second; an HTTP 4xx / parse
error re-inserts the same rows one by one under the same `flush_id`, moving each failing row to
`RPUSH dead` with a `dev_errors` row for its org. One INSERT at a time, so `Flushed.to` is
monotonic per (org, table). This is the only place cursors are assigned.

## Storage

ClickHouse, bootstrapped by the admin user at boot:

```sql
CREATE TABLE IF NOT EXISTS records (
  org_id           LowCardinality(String),
  table_id         LowCardinality(String),
  cursor           UInt64,
  record_id        UUID,
  event_ts_ms      Int64,
  registered_ts_ms Int64,
  metadata         JSON,
  record           JSON
) ENGINE = MergeTree ORDER BY (org_id, table_id, event_ts_ms)
  SETTINGS non_replicated_deduplication_window = 100;

CREATE SETTINGS PROFILE IF NOT EXISTS ss_query SETTINGS
  readonly = 1 CONST, allow_ddl = 0 CONST, max_execution_time = 10 CONST,
  max_result_rows = 100000 CONST, max_result_bytes = 16000000 CONST, result_overflow_mode = 'throw' CONST,
  max_memory_usage = 2000000000 CONST, output_format_json_quote_64bit_integers = 1 CONST,
  SQL_org = '' CHANGEABLE_IN_READONLY;
CREATE USER IF NOT EXISTS ss_query IDENTIFIED WITH sha256_password BY '…' SETTINGS PROFILE 'ss_query';
GRANT SELECT ON space_station.records TO ss_query;
CREATE ROW POLICY IF NOT EXISTS org_only ON space_station.records FOR SELECT USING org_id = getSetting('SQL_org') TO ss_query;
```

The backend sends `SQL_org=<org>` as a URL parameter on every `ss_query` request and no other
settings; it appends ` FORMAT JSONEachRow`. The rewrite is the mirage; the row policy is the
boundary. ClickHouse is spoken to over HTTP with `reqwest`. JSON paths come back as `Dynamic`;
docs tell users to cast (`record.price::Float64 > 5`), that 64-bit integers inside `record`
arrive as JSON numbers (send large ids as strings), and that key order is not preserved.
`output_format_json_quote_64bit_integers = 1` means **every** `UInt64`/`Int64` value in a result
is a JSON string — the three columns, and any expression of that type: `count()`, `countIf()`,
`sum()` of an integer, `::Int64`, `toUnixTimestamp64Milli()`; docs tell users to wrap with
`toFloat64()`/`toInt32()` when they want a number. The overview windows and the lag sample are
measured on `registered_ts_ms`; `event_ts_ms` is the sort key.

Postgres (migrations in `crates/backend/migrations`), each definition stored once with a
`current_version` pointer:

```
tables(org, id, key_hash, access jsonb, created_by, created_at, key_rotated_at, PK(org, id))
windows(id, org, name, access jsonb, created_by, created_at, current_version → window_versions.id,
        state jsonb, state_version, produced_at)
window_versions(id, window, name, processor, renderer, created_by, created_at)
access_tokens(org, actor, membership_id, principal_id, token_hash, token_enc, created_at, last_used_at, PK(org, actor))
notifications(id, org, created_by, created_at, enabled, recipients jsonb, cursors jsonb, last_cron_at,
              current_version → notification_versions.id)
notification_versions(id, notification, def jsonb {name, description, triggers, sql, delay, cooldown, access},
                      created_by, created_at)
notification_events(id bigserial, notification, org, dedup_key, text, metadata jsonb, created_at)   -- append only
webhooks(id, org, url, secret_enc, actor null, created_by, created_at)   -- actor set = a silicon's own delivery webhook
api_keys(id, org, key_hash, scopes text[], created_by, created_at, last_used_at)
sessions(id_hash, actor, kind, org, membership_id, tags jsonb, oat_enc, ort_enc, expires_at, refresh_key, checked_at, created_at)
iam_members(org, actor, membership_id, principal_id, kind, status, tags jsonb, org_name, version, updated_at, PK(org, actor))  -- the directory mirror
dev_errors(id, org, source, ref, message, detail jsonb, created_at)      -- notification engine + delivery failures only
iam_events(event_id PK, received_at)
```

Redis: `staging` (list), `dead` (list), `flushing`, `dedup:*` (5 min), `lease:engine`.

## The mirage (`backend::sql`)

`sql::guard(&Identity | Org, sql, restrict) -> String` is a `sqlparser` `VisitorMut`
(ClickHouse dialect, feature `visitor`):

1. exactly one statement, a `Query`;
2. `pre_visit_query` rejects `SETTINGS`, `FORMAT` and `INTO OUTFILE` at every nesting level and
   pushes that query's CTE names on a scope stack (popped in `post_visit_query`);
3. `pre_visit_table_factor` is an allowlist: `Table{args: None, single-part name}` that is an
   in-scope CTE (left alone) or a visible table (rewritten to
   `(SELECT cursor, record_id, event_ts_ms, registered_ts_ms, metadata, record FROM space_station.records
   WHERE org_id = '…' AND table_id = '…' [AND cursor > a] [AND cursor <= b]) AS name`);
   `Derived` (recursed); the relation of a Join whose operator is `ArrayJoin`/`LeftArrayJoin`
   (a column expression: no table check, no rewrite, `args` must be `None`); everything else
   (table functions, `db.table`, `system.*`) is refused;
4. `pre_visit_expr` refuses `InList` elements that are bare or compound identifiers not qualified
   by a FROM alias or `record`/`metadata` (`x IN (system.one)`);
5. a trigger `where` is parsed with `Parser::parse_expr` and spliced into the AST, never as text.

Visible tables = the org's tables whose access list matches the identity (one Postgres read,
cached with the identity for 60 s); API keys and the notification engine see every table in the
org. Notifications are checked at save time with the saver's identity (every table named must be
visible) and run org-scoped without re-resolving anyone.

`restrict` is `{table: {from?, to?}}`. The server fills a missing `to` with the table's watermark
at query start **for every referenced table** and returns the effective bounds as `watermarks`.

Guard tests include: CTE shadowing of `records`, nested `SETTINGS`, `x IN (system.one)`,
`ARRAY JOIN items AS i` (accepted by the guard; ClickHouse still needs the `::Array(JSON)` cast in
a derived table, because a JSON path is `Dynamic`), a recursive CTE named `records`, `url()`,
`remote()`, `system.tables`, `db.table`, `INSERT`, `UNION`.

## Mission control

### Server side (`backend::windows::live`, `backend::triggers`)

`POST /orgs/{org}/query {sql, restrict?}` → `{rows, watermarks: {table: to}}` for every bearer
kind (cookie, `sscli-`, `spacewindow-`, `apikey-` with `tables` scope). Rows are JSON objects;
every 64-bit integer arrives as a string (quoted 64-bit, see Storage) and the runtime coerces
exactly three keys to `Number` — `cursor`, `event_ts_ms`, `registered_ts_ms` — and nothing else:
a `count() AS n` reaches the processor as `"42"`. Inside the processor `mission_control.query(sql)`
resolves to the **rows array alone**; the watermarks are kept by the runtime (`mission_control.data`).

`GET /orgs/{org}/tables` → `[{id, records, watermark, access, created_by, created_at}]` for
visible tables: this is where a runtime gets its starting watermarks.

WS `/api/ws/mission-control?org=…` — auth decided at upgrade from `Cookie: ss_session` (with the
Origin rule) or `Authorization: Bearer <spacewindow-…|sscli-…>`; the credential is re-validated when
its 60 s cache entry expires at the next message; failure closes with `4401`; `4403` for no
access. Liveness is protocol ping/pong (20 s).

```
→ {"type":"subscribe","id":"sub1","triggers":[{"table":"orders","where":"record.price::Float64 > 5"}]}
← {"type":"subscribed","id":"sub1","watermarks":{"orders":130}}
← {"type":"trigger","id":"sub1"}                                   // a ping; the client decides what to run
→ {"type":"unsubscribe","id":"sub1"}
→ {"type":"state","window":"…","version":"v3"|null,"json":{…}}      // json omitted when unchanged
← {"type":"notification","event_id","notification","name","dedup_key","text","metadata","fired_at"}
← {"type":"error","id":"sub1"|null,"code":"…","message":"…"}   // id echoes the frame that failed
```

`triggers::register(org, table, where, on_hit)`. On `Flushed{org, table, from, to}` the module
runs, once per distinct (org, table, where), `SELECT 1 FROM t WHERE cursor > from AND cursor <= to
AND (where) LIMIT 1` through the guard (`where` absent = any row), and calls every `on_hit`:
subscriptions send `trigger`, notifications arm a timer, cron is a timer calling the same
`on_hit`.

`state`: stored only when the sender passes the window's access list, `version` equals the
window's current published version name, and `json` ≤ 64 KB; `version: null` (dev) is accepted
but never stored; violations answer `error` with `forbidden | version_not_current | too_large`.
The server stamps `produced_at` on receipt. `is_live` = a state for the current version was
received < 30 s ago. Last write wins. `GET /orgs/{org}/windows/{id}/state` → `{json | null,
metadata: {processor_version, renderer_version, produced_at, is_live}}`.

`notification` frames go to every mission-control socket whose actor is in the recipients.

### Runtime (`packages/space-station/mission-control.js`, one file, three roles)

Processor code is **untrusted**: it runs with no credential and no I/O except the bridge.

- **host** — the only role that holds credentials and talks HTTP/WS. Browser: the app page (cookie
  to its own origin) or the dev page (to `127.0.0.1:4747`, which injects the token). Node: the
  `windows run` / dev-server process with `SPACE_STATION_ACCESS_TOKEN`. It creates the processor
  sandbox and the renderer iframe and relays between them.
- **processor** — Browser: an `<iframe sandbox="allow-scripts" srcdoc>` (opaque origin, never
  `allow-same-origin`) whose srcdoc carries
  `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src <cdn> 'unsafe-inline' 'unsafe-eval'">`
  so it cannot `fetch`; it loads the runtime from the CDN and receives the code by `postMessage`.
  Node: `node --permission --allow-fs-read=<runtime dir> mission-control.js processor` spawned
  with `env: {}`; the same bridge as JSON lines over stdio. The child never sees the token, `.env`
  or `~/.space-station`.
- **renderer** — `<iframe sandbox="allow-scripts" srcdoc>` with the bridge injected; all external
  requests allowed; no cookies (`SANDBOX = "allow-scripts"` is one exported constant, asserted by
  a test). The host accepts a frame's message only if `event.source === frame.contentWindow` and
  sends with `targetOrigin "*"`.

Bridge verbs (host ⇄ processor): `→ load {code, window, version}`, `← tables`, `→ tables {…}`,
`← query {id, sql, restrict}`, `→ result {id, rows, watermarks} | error {id, code, message}`,
`← subscribe {id, triggers}`, `→ subscribed {id, watermarks}`, `→ trigger {id}`,
`← unsubscribe {id}`, `← state {json}`, `→ tool {id, name, args}`, `← tool_result {id, result} |
tool_error {id, code, message}`, `← json {json}` (for the renderer), `← dev_error {…}`.
Host ⇄ renderer: `→ update {json, metadata, tools}`, `← tool {id, name, args}`,
`→ tool_result {id, result} | tool_error {id, code: unknown_tool|invalid_args|timeout|failed, message}`.

Processor lifecycle:

1. `tables` → `W[t]` for every trigger table;
2. seed SiliconJSON from the cached state (host fetched `GET …/state`) or `{}`;
3. run `init(json)`; every `mission_control.query(sql)` inside init is sent with
   `restrict {t: {to: W[t]}}` for the trigger tables; the result must be a JSON object ≤ 64 KB;
4. `subscribe` each subscription; `data.tables[t] = {cursor: W[t], watermark: W[t]}`; if
   `subscribed.watermarks[t] > cursor` for a trigger table, run that subscription once (this is
   also the reconnect catch-up);
5. on `trigger`: coalesce per subscription in one serial queue (definition order wins when several
   are ready). `delta`: `query(sql, restrict {t: {from: this subscription's own cursor for t}})`
   for every trigger table; `snapshot`: `query(sql)`. Then `onTrigger(json, rows)` with 10 s; only
   after it settles with a JSON object ≤ 64 KB advance this subscription's cursor for `t` to
   `result.watermarks[t]` (`data.tables[t]` stays the per-table view: `watermark` the highest seen,
   `cursor` how far every subscription on that table has come), replace
   SiliconJSON, emit `json` and `state`. A throw, timeout, non-object or oversized result is a
   dev error; SiliconJSON and cursors unchanged; next in the queue;
6. tools share the queue and the 10 s limit; `run` gets a structured clone of SiliconJSON and its
   return value (any JSON ≤ 64 KB) is the result; SiliconJSON is never replaced by a tool. Type
   table: `string` → `typeof === "string"`; `number` → finite number; `boolean`; `object` →
   non-null non-array; `array`; a `?` suffix permits absent/undefined only; unknown keys →
   `invalid_args`; args ≤ 64 KB;
7. `state` is sent on every change (coalesced ≤ 1/s) and at least every 10 s;
8. on WS close the host sets `metadata.is_live = false`, reconnects with backoff, re-subscribes,
   and the processor runs the catch-up of step 4 — except on a 4401 or 4403, which mean this
   credential or this actor and cannot be fixed by reconnecting: those become a dev error and stop
   the loop. A server-side failure closes with 1011 and is retried like any other drop.

`mission_control` inside the processor: `query(sql)`, `data`, `json`. Inside the renderer:
`json`, `metadata`, `stale` (`!metadata.is_live`), `tools.<name>(args)`, `on("update", cb)`,
`mount(selector)` (a Vue 3 app with `mission_control` and `stale` in scope if `Vue` is on the
page). The renderer cannot query or subscribe.

Dev errors are local to the run that produced them: the Option+Shift+D panel reads the host's
list, the CLI prints Node's.

### `space-station-dev` (same package, `bin`)

Reads `.env` (`SPACE_STATION_URL`, `SPACE_STATION_ACCESS_TOKEN`), binds `127.0.0.1:4747` only,
serves the identical host page with `./processor.js` + `./renderer.html` (reload on change), and
is the only thing that opens HTTP/WS to the backend, injecting the bearer. Every request and
upgrade requires `Host ∈ {localhost:4747, 127.0.0.1:4747}` and, for the WS and mutating calls,
`Origin` equal to the page's own origin. `.env` and the token are never served.
`space-station-dev publish --window <id> --name <version>` (pre-flight secret regex; the backend
is the gate) and `space-station-dev notify <notification-id>` (runs it against the last known
trigger, or says there was none).

## Notifications (`backend::notifications`)

Definition as in `UNDERSTANDING.md`. `recipients` is the subscription set: create/PUT set it,
subscribe/unsubscribe edit it, delivery reads it; it must stay a subset of `access`. `delay` and
`cooldown` match `^[0-9]+(ms|s|m|h|d)$` (stored as integer ms; `delay ≤ 1h`, `cooldown ≤ 30d`;
defaults `2s`, `10m`). `triggers` may mix `{table, where?}` and `{schedule}` (5-field cron, UTC).

Engine (runs under the engine lease):

- a table hit (via `triggers`) arms a timer at `now + delay` if none is armed for this
  notification; the worker is serial per notification; a hit during a run re-arms;
- a run executes `sql` with `restrict {t: {from: cursors[t]}}` for every trigger table (the server
  fills `to` with the watermark at run time) and on success advances `cursors` to the echoed
  watermarks; a SQL error leaves cursors unchanged and writes `dev_errors`. Cron runs `sql`
  unrestricted. Docs: *with table triggers the sql sees only rows newer than the notification's
  cursors; use a schedule trigger for snapshot-style checks*;
- on create, and on `enabled` false → true, `cursors` are reset to the current watermarks. On boot
  or lease acquisition every enabled notification whose cursors are below the watermarks is
  checked over `(cursor, watermark]` and scheduled; a cron notification whose `last_cron_at`
  skipped an occurrence runs once;
- each row must be `{dedup_key: non-empty string ≤ 256 bytes, text: string, metadata: object}`;
  anything else is a bad row (`dev_errors`, nothing sent). The **shape is also checked at save
  time**: create and PUT plan the SQL and refuse a select list that cannot produce exactly those
  three columns (`400 invalid_sql_shape`, naming the missing or extra column), so a typo is an
  API error and not a silent stream of bad rows. Types are still only known at run time;
- cooldown is one statement: `INSERT INTO notification_events … SELECT … WHERE NOT EXISTS (SELECT 1
  FROM notification_events WHERE notification = $1 AND dedup_key = $2 AND created_at > now() -
  $3::interval)`; a row is delivered only if the INSERT inserted;
- delivery to every recipient: `@carbon` → `notification` WS frame (the tab shows the stored
  events); `@silicon` → its delivery webhook row; `webhook:{id}` → that org webhook. Body is
  exactly `{dedup_key, text, metadata}`; headers `X-Space-Station-Event-Id`,
  `X-Space-Station-Notification`, `X-Space-Station-Timestamp` (unix seconds),
  `X-Space-Station-Signature: v1=<hex hmac_sha256(secret, "{seconds}.{raw body}")>`. Attempts at
  0 s, 10 s, 60 s with the same event id; any 2xx within 10 s is success; the final failure is a
  `dev_errors` row. Webhook URLs are validated on write and on send (`invalid_url`): **https
  only**, no userinfo, the resolved IP refused if loopback / RFC1918 / link-local / ULA / metadata.
  `SS_ALLOW_PRIVATE_WEBHOOKS` relaxes the address rule and permits `http://` **only to a
  loopback or private host** — a public `http://` URL stays refused, so a dev flag cannot leak a
  signed body in clear across the internet. `redirect::Policy::none()`, 10 s timeout, 1 MiB
  response cap.
- `DELETE /orgs/{org}/webhooks/{id}` also **prunes `webhook:{id}` from every notification's
  recipients** in the org, in the same transaction, so no notification keeps addressing a
  recipient that no longer exists; `DELETE /orgs/{org}/silicon-webhook` removes the caller's
  delivery webhook the same way (a notification that still names `@silicon` then records a
  `dev_errors` row at delivery, exactly as if none had ever been set).
- `POST …/notifications/{id}/test` runs the sql now over `(cursors, watermark]` without advancing
  and returns `{rows, error?, last_trigger_at | null}`.

## HTTP API (prefix `/api`)

```
GET  /auth/login?org=[&next=][&cli=&state=]  GET /auth/callback?slt=[&state=]  POST /auth/session {slt, org} → {token}
                                        (with cli: 303 → http://127.0.0.1:{cli}/?slt=…&state=…, the terminal spends it at /auth/session)
POST /auth/logout  GET /me → {id, kind, org, app}  GET /orgs → the mirror's orgs for this actor
GET  /orgs/{org}/tables                 POST {id, access?} → {key}       PUT /tables/{t} {access}
POST /orgs/{org}/tables/{t}/rotate-key → {key}                           GET /tables/overview?window=5h
GET  /orgs/{org}/windows                POST {name, access?}             GET|PUT /windows/{id}
GET  /orgs/{org}/windows/{id}/versions  POST {name, processor, renderer}  GET /windows/{id}/state
GET  /orgs/{org}/access-token → {token, last_used_at}  POST /access-token/rotate → {token}
GET  /orgs/{org}/notifications  POST {def, recipients?}  GET|PUT /notifications/{id}  GET …/{id}/events
POST /orgs/{org}/notifications/{id}/subscribe  DELETE …/subscribe  POST …/test
GET  /orgs/{org}/webhooks  POST {url} → {id, url, secret}  DELETE /webhooks/{id}   (also pruned from recipients)
PUT  /orgs/{org}/silicon-webhook {url} → {url, secret}  DELETE /silicon-webhook   (the caller's; silicons only)
GET  /orgs/{org}/api-keys  POST {scopes} → {id, key}  DELETE /api-keys/{id}
POST /orgs/{org}/query {sql, restrict?} → {rows, watermarks}
GET  /orgs/{org}/dev-errors
POST /iam/webhook                also served at /webhooks/api[/] — the URL registered with IAM
WS   /ws/ingest            WS /ws/mission-control?org=
```

API keys: `tables` scope → `GET /tables`, `GET /tables/overview`, `POST /query`;
`notifications` scope → `GET /notifications`, `GET /notifications/{id}`, `GET …/events`. Keys
bypass access lists within scope; every other route answers `401 unauthorized` for `apikey-`.

Errors: `{"error": {"code": "…", "message": "…"}}`; codes are snake_case and branchable.

## Frontend (`apps/web`)

The landing page is an **org picker**: signed out it asks for an org handle and starts the
org-bound login; signed in it lists the orgs the mirror knows the actor in, the session's own
first, and any other org by its handle. Choosing an org and switching org are the same act — a link
to `/api/auth/login?org=…&next=…`, which IAM completes without a prompt while its own session is
good — and a page of another org than the session's shows that switch offer instead of its tabs
(the API answers `403 not_a_member`). The sidebar lists the same orgs. An org shows tabs Space
Windows · Tables · Notifications · Settings (Tables is the default tab when no table exists). Tables: overview (count, records, top 5
tables over `1m 5m 15m 1h 5h 1d 7d 30d`, default `5h`, polled every 5 s; average
event→registered lag over the last 100 records), the list, create (asks only for the id; the key
shown once), rotate, access. An empty Tables tab is one big "create your first table" button. Space
Windows: an empty tab reads "Create your first Space Window"; create (name < 20 chars) → the prompt
for an agent + "add code"; versions with `created_by`; the window view runs the processor sandbox
and the renderer iframe through the runtime's host role and shows the access token and its last
use up top. Notifications: list, create, events, subscribe. Settings: webhooks, API keys.
Option+Shift+D toggles the dev panel (local processor/renderer errors + `GET /dev-errors`). `/docs`
holds the documentation and states Vue 3 + D3 via CDN as the default and preferred renderer stack.
The frontend talks to the backend through a same-origin `/api` rewrite.

## The Rust package (`space-station`) — the primary interface

Everything Space Station can do is a method on this crate. The CLI is a shell over it and has no
capability of its own; the web app is a subset. Nothing may be added to the CLI that is not first
a method here. Sync throughout (`ureq`), because every call is one request and one answer.

**The package is stateless.** The same code with the same inputs behaves the same on any machine:
no ambient reads of the environment or the home directory, no writes, no browser. `Auth` is a
value the caller constructs; `Space` is a function of `(url, Auth)`. The one place state is
unavoidable — a refresh token rotates on use and reusing one revokes the whole family — is handled
by emission, not storage: the Application session and its refresh live in the backend, so the package
never rotates anything; `Space::new(url, auth)` is all there is. The ingest half is deliberately stateful (the spool and
the daemon lock are the crash-recovery feature) but never ambient: its home is injected through
`Builder::home` / `daemon::Config`, and `run_window` takes the directory it may unpack the
runtime into. **The CLI is stateful**: it owns `~/.space-station/`, the single signed-in user,
the current org, and the browser.

**Never ask for a credential; always ask for a short-lived token.** Nothing in this crate or the
CLI ever prompts for, receives or stores a silicon's `stk-` token, an IAM bearer, a refresh token
or an Application secret. A silicon or a carbon obtains a short-lived token from IAM by their own
means (`iam silicon-login --app-id`, `iam login --app-id`, or the browser) and hands that over;
Space Station exchanges it once and holds the resulting session itself.

```rust
use space_station::{Auth, Space, SpaceClient};

// Recording (unchanged): a table key, a background daemon, non-blocking.
let ss = SpaceClient::new("table-orders-…")?;
ss.record(serde_json::json!({"id": "o-42", "amount": 12.5}));

// Managing: an identity, an org, and typed calls.
let auth = space_station::exchange(&url, slt_from_the_iam_cli, "tos")?;   // a value; the CLI is what stores one
let space = Space::new(space_station::default_url(), auth)?.org("tos");
let tables = space.tables()?;
let key = space.create_table("orders", &["@alice", "tech"])?;
let rows = space.query("SELECT count() FROM orders", &Default::default())?;
```

**Why nothing here depends on an IAM client.** The backend is the registered Application and
speaks to IAM through the official `silicon-iam-client` against the contract in "Identity". This
crate goes one step further and never talks to IAM at all: a short-lived token is minted by the
`iam` CLI or by the browser, and the backend is the only party that exchanges it — so the
published crates keep MSRV 1.88 and carry no IAM dependency.

`Auth` is how you prove who you are — a value, never a file:

| value | for | what it holds |
|---|---|---|
| `Auth::session(sscli)` | a carbon or a silicon signed in from a terminal | the `sscli-` session the backend minted and refreshes |
| `Auth::access_token(t)` | window/processor code | a `spacewindow-` token |
| `Auth::api_key(k)` | a program acting for the org | an `apikey-` key |

Two actions produce a credential without keeping one. `space_station::login(url, org, visit)`
runs the loopback flow below, calling `visit(link)` so the caller opens (or prints) the link, and
returns `Auth::session`. `space_station::exchange(url, slt, org)` posts a short-lived token — minted by
`iam login --app-id` or `iam silicon-login --app-id` — to `POST /api/auth/session {slt, org}` and
returns the `Auth::session` the backend minted for it, bound to `org`. Neither the package nor the CLI ever handles an IAM
bearer or refresh token; the backend holds the Application session and rotates it. `Auth` implements `Serialize`/`Deserialize` so a caller can persist it verbatim,
and `Auth::describe()` names what it is without revealing what it holds.

`Space` methods, one per operation, each returning a typed value or `Error`:

```
orgs() me() app_url()                                   whoami and where the UI lives
tables() table(id) create_table(id, access) -> Key      set_table_access(id, access)
rotate_table_key(id) -> Key  delete_table(id)  overview(window)
windows() window(id) create_window(name, access) update_window(id, name, access) delete_window(id)
versions(id) publish(id, name, processor, renderer) window_state(id)
run_window(id, runtime_dir, on_line) window_tool(id, runtime_dir, name, args) window_url(id)
notifications() notification(id) create_notification(def, recipients) update_notification(id, …)
delete_notification(id) events(id) subscribe(id) unsubscribe(id) test_notification(id)
webhooks() create_webhook(url) -> Key delete_webhook(id) set_silicon_webhook(url) -> Key delete_silicon_webhook()
api_keys() create_api_key(scopes) -> Key delete_api_key(id)
access_token() rotate_access_token()
query(sql, restrict) dev_errors() logout()
```

`run_window(id, runtime_dir, on_line)` and `window_tool(id, runtime_dir, name, args)` write the
embedded runtime into `runtime_dir` when its bytes differ and spawn `node` on it with only `PATH`
and `SPACE_STATION_ACCESS_TOKEN`;
`run_window` streams `WindowOutput::Json` and `WindowOutput::Diagnostic` events through `on_line`
and returns when the window goes idle. The CLI sends these to stdout and stderr respectively.

Anything that needs pixels stays in the web app, and the package hands back a URL instead of
inventing a terminal renderer: `window_url(id)` is `{app}/o/{org}/windows/{id}`, where `{app}`
comes from `GET /me`, which needs no `?org=` because a session is bound to its org. Nothing in
the package opens a browser: `login(url, org, visit)` hands the link to `visit` and the caller
decides what to do with it.

## CLI (`space-station`)

One `clap` tree over the package, plus formatting. Human tables for `ls`, JSON for everything else
(pretty on a TTY, compact when piped), secrets alone on stdout with a one-line note on stderr.
`--org` (or `SPACE_STATION_ORG`, or the org stored by `use`) scopes every org command; a silicon's
org is the suffix of its id and needs no flag.

```
login [--no-browser]              a carbon: opens IAM in a browser, stores the session
auth <slt> [--org o]              a short-lived token from `iam login --app-id` / `iam silicon-login --app-id`
                                  (or `-` for stdin, or $SPACE_STATION_TOKEN); never prompts for a credential
logout                            forgets the stored credential (and ends a terminal session server-side)
whoami                            id, kind, org, tags
orgs                              the orgs you belong to          use <org>   the default for later commands

tables ls | get <id> | create <id> [--access a,b] | access <id> --access a,b
       rotate <id> | rm <id> | overview [--window 5h]
windows ls | get <id> | create <name> [--access a,b] | edit <id> [--name n] [--access a,b] | rm <id>
        code <id>                       the current version's processor and renderer (ls/get/edit print a summary, never code)
        versions <id> | publish <id> --name v --processor f --renderer f
        run <id> | json <id> | tool <id> <name> <json> | open <id>
notifications ls | get <id> | create <file> | edit <id> <file> | rm <id>
        events <id> | subscribe <id> | unsubscribe <id> | test <id>
        (<file> is the bare definition, or {def, recipients})
webhooks ls | create <url> | rm <id>            org webhooks; create prints the secret once
webhook set <url> | rm                          this silicon's own delivery webhook
keys ls | create --scopes tables,notifications | rm <id>
token show | rotate                             the access token processors use
query "<sql>"
errors                                          notification dev errors
daemon [run | status]
```

`open` prints `window_url(id)` and opens it when a browser is available; every command that has a
useful page prints its link on stderr, so a terminal session can always hand off to the UI.

The CLI persists `Auth` as `~/.space-station/auth.json` (0600, `flock`, tmp + rename) together
with the org chosen by `use`. `GET /me` answers every session with `{id, kind, org, app}`, so
links can be built; `POST /auth/logout` accepts an `sscli-` bearer so `logout` ends the row, and
refuses any other bearer rather than pretending.

## Signing in from a terminal

The backend brokers every login, because only it holds the Application secret.

- **Browser at hand** (`spacestation login --org o`): `space_station::login` binds `127.0.0.1:0`
  and asks the caller to open `{backend}/api/auth/login?org=o&cli={port}&state={nonce}`. The
  backend 302s to IAM's login page naming `{SS_ORIGIN}/api/auth/callback` as the redirect; IAM
  comes back with `?slt=`; because `cli` was present (a bare port, so loopback is the only
  reachable target) the backend does **not** exchange it: it 303s the browser to
  `http://127.0.0.1:{port}/?slt=…&state={nonce}` instead of setting a cookie, forwarding the
  short-lived token untouched. The listener checks `state`, answers one page, exits, and the CLI
  spends the slt exactly as the no-browser row does — `POST /api/auth/session {slt, org}` — so
  the `sscli-` credential is minted over that POST and **never travels in a URL** (browser
  history, proxy logs, the Referer of the "you may close this tab" page). What appears in the URL
  is single-use and dead in two minutes.
- **No browser** (`spacestation auth <slt> --org o`, or a silicon): the person or machine mints an
  slt with the `iam` CLI (`iam login --app-id 'tos>spacestation' --org o`, `iam silicon-login
  --app-id 'tos>spacestation'`) and hands it over; `space_station::exchange` posts it to
  `POST /api/auth/session {slt, org}` and gets `{token: sscli-…}` back. The slt is single-use and
  two minutes old at most, which is the whole point.

`sscli-…` is its own session row, so ending the terminal does not sign out the browser. It is
presented as `Authorization: Bearer`, resolves through the same `sessions` lookup and the same
refresh, and is not subject to the `Origin` rule (a bearer is not attached by a browser).

Two local stacks and one browser: cookies are scoped per **host**, never per port, so two
backends whose `SS_ORIGIN`s differ only by port both set `ss_session` for `localhost` and sign
each other out on every callback. A second stack runs on another host literal — `127.0.0.1` or
`[::1]` — with `SS_ORIGIN` to match.

## Deleting

`DELETE /orgs/{org}/tables/{table}`, `…/windows/{id}` and `…/notifications/{id}`, each requiring
the caller to pass the resource's access list. A window takes its versions with it and a
notification takes its versions and its event history, by `ON DELETE CASCADE`. Deleting a table
also removes its records: the row goes from Postgres at once and the backend issues
`DELETE FROM records WHERE org_id = … AND table_id = …` to ClickHouse, which is a mutation and
therefore asynchronous — the docs say so, and the table id is free to reuse immediately.

## Tests

`scripts/test.sh` runs groups — `shared` (limits, sanitizer, secret scanner, table keys),
`client` (spool and seq, daemon election, batch building, acks; `Auth` as a value, loopback login,
slt exchange; publish pre-flight, embedded runtime, node spawn; `scripts/sync-runtime.sh` runs
first), `backend` (unit: guard — one test runs the rendered SQL on ClickHouse — access, cron,
crypto, config, IAM stub + client + session, webhook receiver), `backend-integration` (docker
services + the IAM stub: every login door, ingest → flush → query → trigger → notification →
webhook, session refresh, IAM webhook → mirror + revoke, tombstones), `cli` (auth.json lifecycle
and lock, login through a fake browser, an slt exchange that never prompts, org resolution,
output shapes, the full command tree), `runtime` (`node --test`:
queue semantics, delta/snapshot, tool type table, sandbox constant, secret regex, 64 KB bound),
`web` (typecheck, build, and the unit tests for the dev panel, one-time secrets, notification
definitions and the agent prompt) — prints what each group covers, and ends with a pass/fail table.

## Runtime host API (what `apps/web`, the dev server and the CLI call)

`mission-control.js` is one CommonJS-compatible script with no imports. In a browser it defines
`window.SpaceStation`; under Node it is also a CLI.

```js
const mc = SpaceStation.host({
  runtimeUrl,                 // absolute URL of this same file, for the processor iframe's <script> and CSP
  api,                        // {base: "/api", headers?: {}} — fetch base; cookie in the app, header via the dev proxy
  ws,                         // absolute ws(s) URL of /api/ws/mission-control?org=…
  org, window,                // window: {id, name, version: {name, processor, renderer} | null}
  code,                       // optional {processor, renderer} — dev code; version must be null when given
  mount,                      // HTMLElement that receives the renderer iframe
  onJson, onStatus, onError,  // callbacks; onStatus gets {is_live, produced_at, connected}
  onNotification,             // a notification frame whose recipients name the viewing actor
})
mc.errors     // dev errors of this run (processor + renderer), newest last
mc.destroy()
```

Node surface (`process.argv[1]` is this file):

```
node mission-control.js run        --url U --org O --window W          host: runs the current published version until idle 10 min
node mission-control.js tool       --url U --org O --window W --name N --args JSON   one-shot on the cached state
node mission-control.js dev        [serve [--port 4747] [--dir .]] | publish --window W --name V | notify <notification-id>
node mission-control.js processor                                      the credential-less child (stdio bridge)
```

`run`, `tool` and `dev` read `SPACE_STATION_ACCESS_TOKEN` (and `dev` reads `.env` in `--dir`) from
the environment; `processor` is spawned with `env: {}`.

`GET /orgs/{org}/me → Identity {kind, id, org, tags}` gives a page the caller's id and tags
inside the org its session is bound to; `GET /me → {id, kind, org, app}` needs no `?org=` for
anyone, carbon or silicon, because the session already names its org.
