# Space Station architecture

The Rust package is the primary interface. The CLI delegates operations to it; the SolidJS frontend presents the same account-owned resources. The native backend uses Axum, PostgreSQL for configuration, ClickHouse for records, and Redis for staging and delivery coordination.

## Identity and ownership

Silicon Accounts authenticates Carbons and Silicons. Accounts have immutable UUIDs and mutable display handles (`c:alice`, `si:tos`). HTTP routes use the current authenticated account; there is no organization selector or `/orgs` API. The `account_owners` mapping associates each UUID with one storage namespace. Physical legacy `org`/`org_id` SQL columns remain for migration compatibility and never establish authority on their own.

Carbon browser sign-in uses authorization code, state, and PKCE. A Silicon supplies an app-bound, two-minute, one-use SLT from `silicon-accounts login --app spacestation`. The backend exchanges either with its private app credentials. The browser receives an HttpOnly persistent cookie; the CLI receives an opaque `sscli-` session. Upstream access and refresh tokens remain encrypted on the backend. Refresh is serialized per session, and a temporary upstream failure preserves the session. Logout, revocation, or token expiry ends it.

The Accounts webhook verifies an HMAC over the timestamp and raw body, checks the receiving app, and deduplicates event IDs. Handle updates change the display identity without changing the UUID or storage namespace. An account deletion or removal of app access disables its owner mapping. A sign-out notice marks sessions for introspection: Accounts identifies the revoked session, so logging out of the CLI does not also delete an independent browser login.

Browser account contexts use an opaque public `context_id`, the hash of the session secret. Ordinary cookie-authenticated resource requests carry this marker to reject stale tabs after an account switch. Explicit account selection uses the saved-account cookie, an Origin check, and a live target session; it also works after the previous session expired. Session cookies expire with the Accounts refresh-token lifetime, and `/me` renews their remaining lifetime. The saved-account cookie is retained independently so a short-lived selected account cannot strand other saved logins.

A Carbon signing into the CLI completes the same browser flow. The backend forwards a one-use, two-minute handoff code and the CLI's state to its loopback listener; the CLI exchanges these for its session token. A Silicon posts its SLT directly, with no browser requirement. CLI profiles are isolated by backend URL and profile name, and stored credentials include the absolute expiry for offline status checks. The library itself never reads authentication files or environment variables.

Access lists and old creator handles remain stored as historical metadata. They no longer grant another account access: each UUID owns its namespace and every resource in it.

## Credentials

- Table keys authorize record ingestion into one account-owned table and are shown once.
- Space Window access tokens authorize processor work for the owning account and can be rotated.
- API keys authorize account operations within their explicit scopes.
- Browser and CLI sessions identify a Carbon or Silicon, with expiry set by Accounts.

`SS_KEY` protects stored tokens and webhook secrets. Keep it stable across deployments. Accounts app and webhook credentials are backend-only `SILICON_ACCOUNTS_*` settings. See `.env.example` and [the migration procedure](ACCOUNTS-MIGRATION.md).

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
`daemon status`, which prints it). A deep `SPACE_STATION_HOME` (a temporary profile directory, a CI
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

**Backend** (`backend::ingest`). Keys resolve through an in-memory `sha256 → (namespace, table)` map
(refreshed from Postgres on miss and every 60 s), requiring an active account owner; `metadata.table_id` is ignored. Per record:
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
moved to `dead`, an ingest batch dropped without an ack, Accounts refusals; nothing at `info` or above
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
`RPUSH dead` with a `dev_errors` row for its account partition. One INSERT at a time, so `Flushed.to` is
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

The backend sends `SQL_org=<account storage namespace>` as a URL parameter on every `ss_query` request and no other
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
tables(org, id, key_hash, access jsonb, created_by, created_at, key_rotated_at, retired_at, PK(org, id))
windows(id, org, name, access jsonb, created_by, created_at, current_version → window_versions.id,
        state jsonb, state_version, produced_at)
window_versions(id, window_id, name, processor, renderer, created_by, created_at)
access_tokens(org, actor, membership_id, principal_id, token_hash, token_enc, created_at, last_used_at, PK(org, actor))
notifications(id, org, created_by, created_at, enabled, recipients jsonb, cursors jsonb, last_cron_at,
              current_version → notification_versions.id)
notification_versions(id, notification, def jsonb {name, description, triggers, sql, delay, cooldown, access},
                      created_by, created_at)
notification_events(id bigserial, notification, org, dedup_key, text, metadata jsonb, created_at)   -- append only
webhooks(id, org, url, secret_enc, actor null, created_by, created_at)   -- actor set = a silicon's own delivery webhook
api_keys(id, org, owner_uuid, key_hash, scopes text[], created_by, created_at, last_used_at)
account_sessions(id_hash, account_uuid, actor, kind, access_token_enc, refresh_token_enc,
                 access_expires_at, expires_at, checked_at, browser_group, secret_enc, world, created_at)
account_login_receipts(login_hash PK, id_hash, created_at)
account_handoffs(code_hash PK, state_hash, secret_enc, expires_at)
account_owners(uuid, namespace, actor, kind, active, updated_at)  -- stable account ownership of storage partitions
dev_errors(id, org, source, ref, message, detail jsonb, created_at)      -- notification engine + delivery failures only
account_events(event_id PK, received_at)
```

Redis: `staging` (list), `dead` (list), `flushing`, `dedup:*` (5 min), `lease:engine`.

## The mirage (`backend::sql`)

The SQL planner is a `sqlparser` `VisitorMut`
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

Visible tables are every table in the authenticated account's namespace (one Postgres read,
cached for 60 seconds). API keys with the `tables` scope use the same namespace. Notifications
are checked against their owner's tables when saved, and the engine loads jobs only for active
account owners. Historical access arrays and tags do not participate in authorization.

`restrict` is `{table: {from?, to?}}`. The server fills a missing `to` with the table's watermark
at query start **for every referenced table** and returns the effective bounds as `watermarks`.

## Mission control

### Server side (`backend::windows::live`, `backend::triggers`)

`POST /query {sql, restrict?}` → `{rows, watermarks: {table: to}}` for every bearer
kind (cookie, `sscli-`, `spacewindow-`, `apikey-` with `tables` scope). Rows are JSON objects;
every 64-bit integer arrives as a string (quoted 64-bit, see Storage) and the runtime coerces
exactly three keys to `Number` — `cursor`, `event_ts_ms`, `registered_ts_ms` — and nothing else:
a `count() AS n` reaches the processor as `"42"`. Inside the processor `mission_control.query(sql)`
resolves to the **rows array alone**; the watermarks are kept by the runtime (`mission_control.data`).

`GET /tables` → `[{id, records, watermark, access, created_by, created_at}]` for
visible tables: this is where a runtime gets its starting watermarks.

WS `/api/ws/mission-control` — auth decided at upgrade from `Cookie: ss_session` (with the
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

`state`: stored only when the window belongs to the sender's account, `version` equals the
window's current published version name, and `json` ≤ 64 KB; `version: null` (dev) is accepted
but never stored; violations answer `error` with `forbidden | version_not_current | too_large`.
The server stamps `produced_at` on receipt. `is_live` = a state for the current version was
received < 30 s ago. Last write wins. `GET /windows/{id}/state` → `{json | null,
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
  requests allowed; no cookies (`SANDBOX = "allow-scripts"` is one exported constant). The host accepts a frame's message only if `event.source === frame.contentWindow` and
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
subscribe/unsubscribe edit it, and delivery reads it. A recipient must be the owning account
(`@c:handle` or `@si:handle`) or one of its configured `webhook:{id}` destinations. `delay` and
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
  or lease acquisition every enabled notification of an active account owner whose cursors are below the watermarks is
  checked over `(cursor, watermark]` and scheduled; a cron notification whose `last_cron_at`
  skipped an occurrence runs once;
- each row must be `{dedup_key: non-empty string ≤ 256 bytes, text: string, metadata: object}`;
  anything else is a bad row (`dev_errors`, nothing sent). The **shape is also checked at save
  time**: create and PUT plan the SQL and refuse a select list that cannot produce those
  three columns (`400 invalid_sql_shape`, naming any missing column), so a typo is an
  API error and not a silent stream of bad rows. Types are still only known at run time;
- cooldown is one statement: `INSERT INTO notification_events … SELECT … WHERE NOT EXISTS (SELECT 1
  FROM notification_events WHERE notification = $1 AND dedup_key = $2 AND created_at > now() -
  $3::interval)`; a row is delivered only if the INSERT inserted;
- delivery to every recipient: `@c:alice` → `notification` WS frame (the tab shows the stored
  events); `@si:bot` → its delivery webhook row; `webhook:{id}` → that account webhook. Body is
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
- `DELETE /webhooks/{id}` also **prunes `webhook:{id}` from every notification's
  recipients** in the account, in the same transaction, so no notification keeps addressing a
  recipient that no longer exists; `DELETE /silicon-webhook` removes the caller's
  delivery webhook the same way (a notification that still names `@si:bot` then records a
  `dev_errors` row at delivery, exactly as if none had ever been set).
- `POST …/notifications/{id}/test` runs the sql now over `(cursors, watermark]` without advancing
  and returns `{rows, error?, last_trigger_at | null}`.

## HTTP API

All application paths begin with `/api`. Authentication uses `/auth/login`, `/auth/callback`, `/auth/session`, `/auth/contexts`, `/auth/context`, and `/auth/logout`. `/me` describes the active account. Silicon browser sign-in posts `{slt, browser: true, identity_kind: "silicon"}` to `/auth/session` and receives cookies; direct CLI sign-in posts `{slt}` and receives `session_token`. Accounts webhooks arrive at `/api/accounts/webhook`. Resource paths are implicit account scope: `/tables`, `/windows`, `/notifications`, `/webhooks`, `/api-keys`, `/access-token`, `/query`, and `/dev-errors`. WebSocket routes use the same authenticated account or table-key scope.

The SQL guard rewrites only permitted table references and the ClickHouse row policy independently confines query execution to the account's storage partition. No client-provided account handle selects a different owner's data.

## Frontend and runtime

The frontend uses Silicon UI component styles adapted to SolidJS. The workspace has Tables, Space Windows, Notifications, and Settings. Account context changes invalidate cached resource state. Only terminal session errors return the user to sign-in; network and server errors remain retryable.

Processor code runs without browser cookies or credentials. Renderer iframes receive SiliconJSON and typed tools through the runtime bridge. The same `mission-control.js` source is bundled into the Rust client and used by the JavaScript development server; synchronize it with `scripts/sync-runtime.sh`.

## Distribution and operation

Silicon Apps distributes native CLI packages described by `apps.yaml` and owns package updates. `accounts --json` and `login status --json` expose the required discovery contract. The backend is a prebuilt native AWS service, and Vercel serves the frontend. See [release publishing](../infra/apps/README.md) and [production operations](../infra/production/README.md).

Verification uses builds and live smoke checks. Automated suites, identity fixtures, and testing-environment integration have been removed.
