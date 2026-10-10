# HTTP API

Everything is under `/api`. JSON in, JSON out. Errors are
`{"error": {"code": "snake_case", "message": "…"}}` — branch on `code`.

The [Rust package](/docs/rust) and [CLI](/docs/cli) wrap the management routes. This page also
describes browser login and context handling for clients written in other languages.

Authenticate with the `ss_session` cookie or `Authorization: Bearer` carrying a CLI session,
`spacewindow-` access token or `apikey-` key. Silicon Accounts tokens are exchanged by the
backend, not accepted directly as Space Station credentials. Mutating cookie requests and
cookie WebSocket upgrades must carry a matching `Origin`.

Every management route uses the authenticated account's UUID. Carbons and Silicons have the
same management surface. Deletes return `204 No Content`.

After `GET /me`, browser clients send `context_id` as `x-spacestation-context` on requests,
context changes and logout. Cookie WebSockets use `account_context` in the query. A mismatch
returns `409 context_changed`; reload before continuing. The marker is not a credential.

## Auth

| Route | Behavior |
|---|---|
| `GET /auth/login?next=/` | Starts Carbon sign-in through Silicon Accounts with PKCE and sealed callback state. `next` must be an app path |
| `GET /auth/login?display=popup&attempt_id={uuid}` | Popup variant. Verified callback sends only `{type:"spacestation:login",attempt_id,status}` to the exact app-origin opener |
| `GET /auth/login?cli={port}&state={nonce}` | Carbon terminal flow. Callback redirects a one-time handoff to `http://127.0.0.1:{port}/?code=…&state=…`; redeem it at `/auth/session` |
| `GET /auth/callback?code=&state=` | Exchanges an authorization code, verifies the account and creates a browser session or CLI handoff |
| `POST /auth/session` | `{slt}` exchanges a single-use Silicon Accounts token; `{code,state}` redeems a CLI handoff. Returns `{session_token,expires_at}` and identity details |
| `POST /auth/session` with `{slt,browser:true,identity_kind:"silicon"}` | Same-origin Silicon browser sign-in. Sets persistent HTTP-only cookies; no session credential is returned to JavaScript |
| `GET /auth/contexts` | `[{context_id,actor,uuid,expires_at,selected}]`: saved browser accounts |
| `POST /auth/context` | `{context_id}` selects a saved session. Requires same origin and the current context marker; reload afterward |
| `POST /auth/logout` | Ends only the selected session. Other saved browser or terminal sessions stay available |
| `GET /me` | `{id,uuid,kind,app,context_id,expires_at}`. UUID is stable; `id` is the display handle. `app` is the UI origin |
| `POST /accounts/webhook` | Signed Silicon Accounts events; integration endpoint, not a client management route |

Sessions persist until expiration, revocation or logout. Transient failures do not clear them.

## Tables

| | | |
|---|---|---|
| `GET /tables` | | `[{id, records, watermark, access, created_by, created_at}]` |
| `POST /tables` | `{id}` | `{key}` — shown once |
| `POST /tables/{t}/rotate-key` | | `{key}` |
| `DELETE /tables/{t}` | | the table now, its records by a background mutation |
| `GET /tables/overview?window=5h` | `1m 5m 15m 1h 5h 1d 7d 30d` | totals, top 5, avg lag |
| `POST /query` | `{sql, restrict?}` | `{rows, watermarks}` |

`restrict` is `{table: {from?, to?}}`; a missing `to` becomes the table's watermark at query
start, and the effective bounds come back as `watermarks`. In `rows`, every 64-bit integer is a
JSON string — `cursor`, `event_ts_ms`, `registered_ts_ms`, and any `count()`, integer `sum()` or
`::Int64` — see [SQL](/docs/sql); the `watermarks` values are numbers.

## Space Windows

| | | |
|---|---|---|
| `GET /windows` | | |
| `POST /windows` | `{name}` | |
| `GET /windows/{id}` · `PUT` · `DELETE` | `{name?}` | `DELETE` takes the window's versions with it |
| `GET /windows/{id}/versions` | | `[{id, name, processor, renderer, created_by, created_at}]` |
| `POST /windows/{id}/versions` | `{name, processor, renderer}` | `secret_in_code` if refused |
| `GET /windows/{id}/state` | | `{json | null, metadata: {processor_version, renderer_version, produced_at, is_live}}` |
| `GET /access-token` | | `{token, last_used_at}` |
| `POST /access-token/rotate` | | `{token}` |

## Notifications

| | | |
|---|---|---|
| `GET /notifications` | | |
| `POST /notifications` | `{def, recipients?}` | `def` is the [definition](/docs/notifications); `400 invalid_sql_shape` when its select list is not `dedup_key, text, metadata` |
| `GET /notifications/{id}` · `PUT` · `DELETE` | `{def, recipients?}` on `PUT` | `DELETE` takes its versions and its whole event history |
| `GET /notifications/{id}/events` | | `[{id, dedup_key, text, metadata, created_at}]` |
| `POST /notifications/{id}/subscribe` · `DELETE` | | adds / removes you |
| `POST /notifications/{id}/test` | | `{rows, error?, last_trigger_at | null}` |

## Settings

| | | |
|---|---|---|
| `GET /webhooks` | | |
| `POST /webhooks` | `{url}` | `{id, url, secret}` — shown once. `https://` only, no private addresses: `400 invalid_url` |
| `DELETE /webhooks/{id}` | | also removes `webhook:{id}` from every notification's recipients |
| `PUT /silicon-webhook` | `{url}` | `{url, secret}`; the caller's own; silicons only |
| `DELETE /silicon-webhook` | | removes the caller's own delivery webhook; silicons only |
| `GET /api-keys` | | |
| `POST /api-keys` | `{scopes: ["tables", "notifications"]}` | `{id, key}` — shown once |
| `DELETE /api-keys/{id}` | | |
| `GET /dev-errors` | | notification, delivery and record-deletion errors |

## API key scopes

`tables` → `GET /tables`, `GET /tables/overview`, `POST /query`.
`notifications` → `GET /notifications`, `GET /notifications/{id}`, `GET …/events`.
Everything else answers `401 unauthorized` to an API key.

## WebSockets

| | |
|---|---|
| `/api/ws/ingest` | the daemon's channel: `{batch_id, records}` in, `{batch_id, status, rejected?}` out |
| `/api/ws/mission-control?account_context=` | the runtime's channel: `subscribe` / `subscribed` / `trigger` / `state` / `notification`; cookie clients include their context marker, bearer clients do not need it |

The mission-control socket authenticates with the cookie or an `sscli-` / `spacewindow-` bearer
— API keys cannot open it. A closed socket with code `4401` means the
credential was refused, `4403` no access; both mean *this credential, this actor*, so reconnecting
with the same one will not help.
