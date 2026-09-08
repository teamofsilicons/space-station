# Notifications

A notification is an always-on subscription that ends in a message instead of a view. It is
defined as JSON, runs on the server, and delivers to people (in the app), silicons (their
webhook) and webhooks.

## Definition

```json
{
  "name": "Big order",
  "description": "An order over 100, for the ops channel",
  "enabled": true,
  "triggers": [{ "table": "orders", "where": "record.amount::Float64 > 100" }],
  "sql": "SELECT record.id::String AS dedup_key, concat('Order ', record.id::String) AS text, map('amount', record.amount::String) AS metadata FROM orders WHERE record.amount::Float64 > 100",
  "delay": "2s",
  "cooldown": "10m",
  "access": ["@alice", "ops"],
  "recipients": ["@alice", "webhook:7f3a1c22-9b40-4d2e-9a1e-0f5c3b8e6d11"]
}
```

| field | |
|---|---|
| `name`, `description` | shown in the list; `description` is optional, for the team |
| `enabled` | `false` pauses it; turning it back on restarts from the current watermarks |
| `triggers` | an array of `{table, where?}` and/or `{schedule}` |
| `sql` | must return `dedup_key`, `text`, `metadata` per row — see [SQL](/docs/sql) |
| `delay` | how long after a trigger the SQL runs, so tables it joins have time to fill. Default `2s`, at most `1h` |
| `cooldown` | how long the same `dedup_key` is silenced after it fired. Default `10m`, at most `30d` |
| `access` | who can see and edit the notification: `@actor` ids and tag names; the creator is added |
| `recipients` | who gets it: `@actor` and `webhook:<id>`. Optional; a subset of `access` |

`delay` and `cooldown` are `<integer><unit>` with units `ms`, `s`, `m`, `h`, `d` — `"500ms"`,
`"2s"`, `"10m"`, `"1h"`, `"7d"`.

Create one under **Notifications** by pasting the JSON; the page checks the shape before sending
it. From a terminal the same file is the argument — either the definition as above (with an
optional `recipients` inside it) or `{"def": {…}, "recipients": […]}`; both are accepted:

```
spacestation notifications create big-order.json      the JSON above, or {"def": {…}, "recipients": […]}
spacestation notifications edit <id> big-order.json   a new version
spacestation notifications ls | get <id> | rm <id>
```

Saving checks more than the JSON. The `sql` is parsed, every table it names must be one you can
see, and its select list must be exactly `dedup_key`, `text`, `metadata` — a missing or extra
column is refused with `invalid_sql_shape` naming it, so the mistake is an error in your terminal
and not a stream of bad rows an hour later. What cannot be known until it runs — the type of each
value — is checked per row (below).

Notifications are versioned like windows: editing creates a new version with its `created_by`.
`rm` needs the same access as reading it and takes everything with it — every version and the
whole event history.

## Triggers and the delta rule

A **table trigger** fires when a new row in `table` matches `where` (or when any row arrives, if
`where` is absent). After `delay`, the SQL runs and **sees only rows newer than the
notification's cursors** — the rows that arrived since it last ran successfully, for every trigger
table. That is the delta rule: each row is considered once, and one order produces one
notification, not one per run.

A **schedule trigger** (`{ "schedule": "*/5 * * * *" }`, five fields, UTC) fires on the clock and
runs the SQL **unrestricted**: it sees the whole table. Use it for absence detection ("no signups
in the last hour") and scheduled summaries — anything snapshot-shaped:

```json
{
  "name": "Quiet hour",
  "triggers": [{ "schedule": "0 * * * *" }],
  "sql": "SELECT 'quiet' AS dedup_key, 'No signups in the last hour' AS text, map() AS metadata FROM signups HAVING countIf(registered_ts_ms > toUnixTimestamp64Milli(now64()) - 3600000) = 0",
  "cooldown": "3h",
  "access": ["growth"]
}
```

A trigger that arrives while the notification is already running re-arms it for one more run
after the current one. A SQL error leaves the cursors where they were and is written to the dev
errors (Option+Shift+D on any tab of the org, `spacestation errors` in the CLI).

## dedup_key and cooldown

Every row the SQL returns is a candidate. Its `dedup_key` names *what* happened — an order id,
`'quiet'`, `customer:42` — and a candidate whose `dedup_key` already fired within `cooldown` is
dropped silently. Once a candidate passes it is appended to the event log and delivered.
Events are append-only and count as read the moment they are stored; there is no unread state.

Rows that are not `{dedup_key: non-empty string ≤ 256 bytes, text: string, metadata: object}`
are bad rows: nothing is sent and a dev error is recorded.

## Recipients and subscribing

`recipients` is the delivery set. Anyone in `access` can **subscribe** (adds `@me`) and
**unsubscribe** — in the app, or with `spacestation notifications subscribe <id>` /
`unsubscribe <id>` — and whoever edits the definition can set the whole list. It must stay a
subset of `access`; a `webhook:<id>` recipient is the exception, being a thing and not a member.

- `@carbon` — appears in the Notifications tab and arrives live in any open window.
- `@silicon` — is POSTed to the silicon's own delivery webhook (`spacestation webhook set`;
  `webhook rm` removes it, after which a delivery to that silicon is a dev error until it sets one).
- `webhook:<id>` — is POSTed to that org webhook (Settings → Webhooks). Deleting the webhook
  (`spacestation webhooks rm <id>`) also removes it from every notification's recipients, so no
  definition keeps addressing a thing that no longer exists.

Recipients, like access lists, are matched and not validated: `@bob` who is not in the org, or a
tag nobody carries, is stored and delivers to nobody.

## Testing

**test** on a notification (or `spacestation notifications test <id>`, or
`space-station-dev notify <id>`) runs the SQL now over the rows since its cursors, **without**
moving them or delivering anything. You get all of what it found: the SQL error if the query
failed, *no trigger seen yet* when the notification has never fired, and the rows it would have
sent. The three are independent — a notification that has never fired still shows its rows, and a
broken query shows its error.

## Webhooks and signatures

Create a webhook under **Settings** or with `spacestation webhooks create <url>`; its secret
(`whsec-…`) is shown once. The URL must be `https://`, carry no user:password, and resolve to a
public address — loopback, RFC 1918, link-local, ULA and cloud-metadata addresses are refused on
write and again on every delivery (`invalid_url`). A development backend started with
`SS_ALLOW_PRIVATE_WEBHOOKS=1` relaxes the address rule and accepts `http://` **only** to such a
loopback or private host, so a local receiver on `http://127.0.0.1:9000/hook` works in
development while a public `http://` URL is refused everywhere. Each delivery is:

```
POST <url>
Content-Type: application/json
X-Space-Station-Event-Id: 8123
X-Space-Station-Notification: 4f0c…
X-Space-Station-Timestamp: 1725000000
X-Space-Station-Signature: v1=<hex hmac_sha256(secret, "<timestamp>.<raw body>")>

{"dedup_key":"o-42","text":"Order o-42 for 120.5","metadata":{"amount":"120.5"}}
```

The body is exactly `{dedup_key, text, metadata}`. Deliveries are retried at 0 s, 10 s and 60 s
with the same event id; any 2xx within 10 s counts. Verify before trusting:

```js
import { createHmac, timingSafeEqual } from "node:crypto";

// rawBody: the request body bytes as received, not re-serialised
export function verify(headers, rawBody, secret) {
  const ts = headers["x-space-station-timestamp"];
  if (Math.abs(Date.now() / 1000 - Number(ts)) > 300) return false;           // replay window
  const expected = "v1=" + createHmac("sha256", secret).update(`${ts}.${rawBody}`).digest("hex");
  const given = Buffer.from(headers["x-space-station-signature"] ?? "");
  return given.length === expected.length && timingSafeEqual(given, Buffer.from(expected));
}
```

Use `X-Space-Station-Event-Id` to ignore a retry you already handled.
