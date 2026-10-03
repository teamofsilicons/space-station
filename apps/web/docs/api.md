# HTTP API

Everything is under `/api`. JSON in, JSON out. Errors are
`{"error": {"code": "snake_case", "message": "…"}}` — branch on `code`.

The [Rust package](/docs/rust) and [CLI](/docs/cli) wrap the management routes. This page also
describes browser login and context handling for clients written in other languages.

Authenticate with the `ss_session` cookie (the app), or `Authorization: Bearer` with an `sscli-`
terminal session (carbons and silicons alike), a `spacewindow-` access token or an `apikey-`
key. Those three are the only bearer shapes accepted; an IAM token of any kind is
`401 unsupported_bearer`. A cookie-authenticated request that mutates, and every WebSocket
upgrade, must also carry a matching `Origin` — a bearer needs none, since no browser sends one by
itself. A session is bound to one account and org: `/orgs/{org}/…` of any other org answers `403 not_a_member`.
Access lists apply to every route except for API keys, which see their whole scope. Actor ids are
sent without `@`. Deletes answer `204 No Content`.

After `GET /me`, cookie clients send its `context_id` as `x-spacestation-context` on organization
requests, context changes and logout. Cookie WebSockets use `account_context` in the query.
A mismatched marker returns `409 context_changed`; reload the selected context before continuing.
The marker is not a credential. Keep requests, caches and background work bound to that context.

## Auth

| | |
|---|---|
| `GET /auth/login?identity_kind=carbon&next=/` | choose `carbon` or `silicon`; the backend binds that choice to a sealed, expiring callback state. IAM selects one account and organization. Optional `org` must match that selection; it does not select an IAM org. `next` is a validated app path |
| `GET /auth/login?identity_kind=carbon&display=popup&attempt_id={uuid}` | popup variant; `display` and `attempt_id` are supplied together. After verified backend success, the callback posts only `{type:"spacestation:login", attempt_id, status}` to the exact app-origin opener. Check origin, popup window and attempt ID; then reload. Blocked popups use the full-page flow; closure is cancellation |
| `GET /auth/login?org=&cli={port}&state={nonce}` | terminal Carbon flow: the callback exchanges the SLT and verifies its identity, then redirects the same SLT to `http://127.0.0.1:{port}/?slt=…&state={nonce}`. The terminal verifies its state and redeems the backend receipt at `POST /auth/session` without a second IAM exchange. `cli` is a bare port (1024–65535); an `sscli-` credential never travels in a URL |
| `GET /auth/callback?slt=&state=` | validates the initiated attempt and selected identity kind before establishing a session. Browser login adds a separate saved context, sets HttpOnly cookies, and either completes the popup or redirects to `next`; other saved sessions remain available |
| `POST /auth/session` | `{slt, org?}` → `{token}`: exchanges a fresh SLT or redeems its matching two-minute backend receipt for an `sscli-` session. IAM mints fresh SLTs with `iam login --app-id 'spacestation' --grant-org o` or `iam silicon-login --app-id 'spacestation'`. Optional `org` must match the selected organization |
| `GET /auth/contexts` | `[{context_id, actor, org, selected}]`: saved sessions in this browser's group, separate from directory membership |
| `POST /auth/context` | `{context_id}` selects one saved browser session; requires the current context marker and same-origin request. Returns `204`; reload to discard the old context's caches and requests |
| `POST /auth/logout` | ends only the presented session. Browser logout requires its context marker and same-origin request, then selects a remaining saved context if present. An `sscli-` bearer ends only that terminal session; other bearers return `401 unsupported_bearer` |
| `GET /me` | `{id, kind, org, app, context_id}` for a session or `spacewindow-` bearer. `context_id` is null for a window bearer; `app` is the UI origin, used for window links |
| `GET /orgs` | `[{id, name}]` containing only the selected session's organization. `name` is IAM's reported name, else its id; saved browser contexts are listed separately |
| `GET /orgs/{org}/me` | `{kind, id, org, tags}` — the tags as IAM last reported them: at your login, at the re-check about once a minute, and by webhook in between |
| `POST /iam/webhook` | Silicon IAM's membership events, verified by signature; not for clients. The same receiver also answers at the root path `/webhooks/api/` (outside `/api`, with and without the slash), the URL registered with IAM |

## Tables

| | | |
|---|---|---|
| `GET /orgs/{org}/tables` | | `[{id, records, watermark, access, created_by, created_at}]` |
| `POST /orgs/{org}/tables` | `{id, access?}` | `{key}` — shown once |
| `PUT /orgs/{org}/tables/{t}` | `{access}` | |
| `POST /orgs/{org}/tables/{t}/rotate-key` | | `{key}` |
| `DELETE /orgs/{org}/tables/{t}` | | the table now, its records by a background mutation |
| `GET /orgs/{org}/tables/overview?window=5h` | `1m 5m 15m 1h 5h 1d 7d 30d` | totals, top 5, avg lag |
| `POST /orgs/{org}/query` | `{sql, restrict?}` | `{rows, watermarks}` |

`restrict` is `{table: {from?, to?}}`; a missing `to` becomes the table's watermark at query
start, and the effective bounds come back as `watermarks`. In `rows`, every 64-bit integer is a
JSON string — `cursor`, `event_ts_ms`, `registered_ts_ms`, and any `count()`, integer `sum()` or
`::Int64` — see [SQL](/docs/sql); the `watermarks` values are numbers.

## Space Windows

| | | |
|---|---|---|
| `GET /orgs/{org}/windows` | | |
| `POST /orgs/{org}/windows` | `{name, access?}` | |
| `GET /orgs/{org}/windows/{id}` · `PUT` · `DELETE` | `{name?, access?}` | `DELETE` takes the window's versions with it |
| `GET /orgs/{org}/windows/{id}/versions` | | `[{id, name, processor, renderer, created_by, created_at}]` |
| `POST /orgs/{org}/windows/{id}/versions` | `{name, processor, renderer}` | `secret_in_code` if refused |
| `GET /orgs/{org}/windows/{id}/state` | | `{json | null, metadata: {processor_version, renderer_version, produced_at, is_live}}` |
| `GET /orgs/{org}/access-token` | | `{token, last_used_at}` |
| `POST /orgs/{org}/access-token/rotate` | | `{token}` |

## Notifications

| | | |
|---|---|---|
| `GET /orgs/{org}/notifications` | | |
| `POST /orgs/{org}/notifications` | `{def, recipients?}` | `def` is the [definition](/docs/notifications); `400 invalid_sql_shape` when its select list is not `dedup_key, text, metadata` |
| `GET /orgs/{org}/notifications/{id}` · `PUT` · `DELETE` | `{def, recipients?}` on `PUT` | `DELETE` takes its versions and its whole event history |
| `GET /orgs/{org}/notifications/{id}/events` | | `[{id, dedup_key, text, metadata, created_at}]` |
| `POST /orgs/{org}/notifications/{id}/subscribe` · `DELETE` | | adds / removes you |
| `POST /orgs/{org}/notifications/{id}/test` | | `{rows, error?, last_trigger_at | null}` |

## Settings

| | | |
|---|---|---|
| `GET /orgs/{org}/webhooks` | | |
| `POST /orgs/{org}/webhooks` | `{url}` | `{id, url, secret}` — shown once. `https://` only, no private addresses: `400 invalid_url` |
| `DELETE /orgs/{org}/webhooks/{id}` | | also removes `webhook:{id}` from every notification's recipients |
| `PUT /orgs/{org}/silicon-webhook` | `{url}` | `{url, secret}`; the caller's own; silicons only |
| `DELETE /orgs/{org}/silicon-webhook` | | removes the caller's own delivery webhook; silicons only |
| `GET /orgs/{org}/api-keys` | | |
| `POST /orgs/{org}/api-keys` | `{scopes: ["tables", "notifications"]}` | `{id, key}` — shown once |
| `DELETE /orgs/{org}/api-keys/{id}` | | |
| `GET /orgs/{org}/dev-errors` | | notification, delivery and record-deletion errors |

## API key scopes

`tables` → `GET /tables`, `GET /tables/overview`, `POST /query`.
`notifications` → `GET /notifications`, `GET /notifications/{id}`, `GET …/events`.
Everything else answers `401 unauthorized` to an API key.

## WebSockets

| | |
|---|---|
| `/api/ws/ingest` | the daemon's channel: `{batch_id, records}` in, `{batch_id, status, rejected?}` out |
| `/api/ws/mission-control?org=&account_context=` | the runtime's channel: `subscribe` / `subscribed` / `trigger` / `state` / `notification`; cookie clients include their context marker, bearer clients do not need it |

The mission-control socket authenticates with the cookie or an `sscli-` / `spacewindow-` bearer
— API keys cannot open it. A closed socket with code `4401` means the
credential was refused, `4403` no access; both mean *this credential, this actor*, so reconnecting
with the same one will not help.
