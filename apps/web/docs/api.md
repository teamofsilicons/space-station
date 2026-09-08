# HTTP API

Everything is under `/api`. JSON in, JSON out. Errors are
`{"error": {"code": "snake_case", "message": "…"}}` — branch on `code`.

Every route here is already a method on the [Rust package](/docs/rust) and a command in the
[CLI](/docs/cli); reach for this page when you are writing something in another language.

Authenticate with the `ss_session` cookie (the app), or `Authorization: Bearer` with an `sscli-`
terminal session (carbons and silicons alike), a `spacewindow-` access token or an `apikey-`
key. Those three are the only bearer shapes accepted; an IAM token of any kind is
`401 unsupported_bearer`. A cookie-authenticated request that mutates, and every WebSocket
upgrade, must also carry a matching `Origin` — a bearer needs none, since no browser sends one by
itself. A session is bound to one org: `/orgs/{org}/…` of any other answers `403 not_a_member`.
Access lists apply to every route except for API keys, which see their whole scope. Actor ids are
sent without `@`. Deletes answer `204 No Content`.

## Auth

| | |
|---|---|
| `GET /auth/login?org=&next=/` | redirects to IAM's login for this Application, bound to `org`; IAM comes back to `/auth/callback?slt=`, the backend exchanges that short-lived token, sets `ss_session` and sends the browser to `next`. IAM asks nothing when its own session is good, which is how a signed-in browser switches org. No `org` is `400 org_required` |
| `GET /auth/login?org=&cli={port}&state={nonce}` | the same door for a terminal: the callback does not exchange the token but redirects to `http://127.0.0.1:{port}/?slt=…&state={nonce}` instead of setting a cookie, and the terminal spends it at `POST /auth/session` — an `sscli-` credential never travels in a URL. `cli` is a bare port (1024–65535), never a URL |
| `GET /auth/callback?slt=` | IAM returns here. In a browser it first ends the session the presented `ss_session` cookie belonged to (one session per browser), then sets the new cookie |
| `POST /auth/session` | `{slt, org}` → `{token}`: a short-lived token the `iam` CLI minted for this Application (`iam login --app-id 'tos>spacestation' --org o`, `iam silicon-login --app-id 'tos>spacestation'`) becomes an `sscli-` session bound to `org`. The token is single-use and two minutes old at most; anything else is refused |
| `POST /auth/logout` | ends the session presented: the cookie (under the `Origin` rule, and cleared), or an `sscli-` bearer — the terminal's own row, the browser untouched. Any other bearer is `401 unsupported_bearer`: it has no row to end |
| `GET /me` | `{id, kind, org, app}` for every session, and for a `spacewindow-` bearer — `org` is the one it is bound to, `app` where the UI lives, so a client can build window links |
| `GET /orgs` | `[{id, name}]`: the orgs Space Station knows you in — those you have signed in to, plus those where IAM's events have shown your membership; `name` is the org name IAM sent, else the id. See [Credentials](/docs/credentials) |
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
| `/api/ws/mission-control?org=` | the runtime's channel: `subscribe` / `subscribed` / `trigger` / `state` / `notification` |

The mission-control socket authenticates with the cookie or an `sscli-` / `spacewindow-` bearer
— API keys cannot open it. A closed socket with code `4401` means the
credential was refused, `4403` no access; both mean *this credential, this actor*, so reconnecting
with the same one will not help.
