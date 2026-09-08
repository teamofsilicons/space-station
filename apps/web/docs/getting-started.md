# Getting started

Space Station takes JSON records for a **table** inside your **organization**, keeps them in
order, and lets you build [Space Windows](/docs/space-windows) (live views) and
[Notifications](/docs/notifications) on top of them.

Org → Tables → Records. A table is logical: one ClickHouse table holds everything and the
backend scopes every query to your org and your table ids, so `FROM orders` just works. Records
are whatever JSON your app cares about; plan what you send, then build views around it.

Everything here is available three ways, and they are the same thing seen from different angles:
the [Rust package](/docs/rust) is the interface, the [CLI](/docs/cli) is a command tree over it,
and this app is a subset of that.

For this v1 checkout, start everything with `python3 scripts/dev.py start` from the repository
root, then open `http://localhost:3000`. Choose organization `tos` and Alice on the local IAM
page. This local environment uses fixture identities; it does not sign you into real IAM.
The backend needs Rust 1.98+, Node 22.13+, Docker, and `psql` on your PATH.
Export `SPACE_STATION_URL=http://localhost:8080`, then use `target/main/debug/spacestation` in place of `spacestation`
below (or the binary path printed by the launcher when `CARGO_TARGET_DIR` is set).

## 1. Create a table

In the app: name your organization, sign in with Silicon IAM, open **Tables** and create one.
From a terminal, the same two steps — a session is bound to the one org it signs in to:

```
spacestation login --org tos       a carbon with a browser
spacestation auth <slt> --org tos  instead, with a short-lived token the iam CLI minted: `iam login --app-id 'tos>spacestation' --org tos`
                                    for a carbon, `iam silicon-login --app-id 'tos>spacestation'` for a silicon (see the CLI page)
spacestation tables create orders --access @alice,tech
```

A table id is unique in the org and matches `^[a-z0-9]{1,50}$`. You are shown the **table key**
once:

```
table-orders-9f3c1a…            table-{table_id}-{32 hex}
```

It is stored hashed. There is one key per table; rotating it (`spacestation tables rotate
orders`, or Tables → rotate key) invalidates the old one at once. Access to a table is a list of
`@actor` ids and IAM tag names — the union of them can see it; whoever creates it is added to the
list. Tags come with the login itself — IAM reports them when Space Station verifies your session
— and IAM's webhooks keep them current; see [Credentials](/docs/credentials). An access list is
**matched, never validated**: Space Station cannot ask IAM whether `tech` or `@bob` exist, so a
mistyped entry is accepted and simply grants nobody anything. Check spelling and case (`tech`,
not `Tech`) when someone who should see a table does not.

`spacestation tables rm orders` deletes the table **and its records**. The row goes at once —
the key stops working and the id is free to reuse — and the records are removed by a background
ClickHouse mutation, so a query in the next second may still see a few.

## 2. Send records

For a first record, use the CLI with the table key you just saved. It uses the same Rust
client and daemon as your application, and does not require a login:

```sh
export SPACE_STATION_TABLE_KEY='<your table key>'
spacestation record '{"id":"o-42","customer":"ada","amount":12.5}'
# Or read one JSON object from stdin:
cat event.json | spacestation record -
```

For continuous recording from your application, use the published Rust package:

```toml
[dependencies]
space-station = "0.1"
```

```sh
curl -fsSL https://spacestation.teamofsilicons.com/install.sh | sh && export PATH="$HOME/.local/bin:$PATH"
# Optional development server for Space Windows:
npm i -g @teamofsilicons/space-station
```

```rust
use space_station::SpaceClient;

let ss = SpaceClient::new(&std::env::var("SPACE_STATION_TABLE_KEY")?)?; // org + table come from the key
ss.record(serde_json::json!({ "id": "o-42", "customer": "ada", "amount": 12.5 })); // non-blocking
```

`record()` never blocks and never panics. It stamps metadata, sanitises the value (see
[limits](#limits)) and hands the line to the daemon. When the in-process queue (10 000 records)
is full the record is dropped and `on_error(queue_full)` fires. `flush()` and `Drop` block — at
most 5 seconds — until the daemon has **acked** everything this client recorded,
so a one-shot program that records and exits still delivers; if the server cannot be reached in
that time `flush()` returns `false` and the lines stay in the spool for the next daemon on the
machine. Point the crate at your Space Station with `SpaceClient::builder(key).url(..)` or
`SPACE_STATION_URL` through `default_url()`; the default is the public host.

The same crate is how you manage everything else — tables, windows, notifications, keys — with
`Auth` and `Space`. See [the Rust package](/docs/rust).

## 3. The daemon

One daemon per machine carries records for any number of table keys over a single WebSocket, so
many apps and orgs on one host cost one connection. The first `SpaceClient` on a machine starts
it in-process (a lock file elects one); `spacestation daemon run` runs it in the foreground
instead, and `spacestation daemon status` says whether one is listening and how many records are
still unacked.

There is no local batching: records go out as soon as possible, in batches of up to 8 MB. Every
record is written to `~/.space-station/spool.jsonl` before it is sent and stays there until the
server acks it, so a crash or an offline stretch loses nothing. Rejections come back per record
with a code:

| code | meaning |
|---|---|
| `unauthorized` | the key resolves to nothing; every record under it is dropped |
| `duplicate` | this `record_id` was seen in the last 5 minutes; treated as delivered |
| `size_exceeded` | the record is over the limit |
| `invalid` | not a JSON object |

**Where a rejection surfaces** depends on who is running the daemon. The server answers the
daemon, not your process: with the in-process daemon (the usual case) a rejection fires the
`on_error` hook of the `SpaceClient` that started it, as `Error::Rejected { record_id, code,
reason }`; when a separate `spacestation daemon` carries your records, it is that process that
logs the rejection and counts it in `spacestation daemon status`, and your `on_error` sees
nothing — a rotated key shows up as `unauthorized` in the daemon's output, not in the app's. `flush()` follows the same line: with
the in-process daemon `true` means the server acked every record (rejected ones included); with a
separate daemon, which outlives your program, `true` means every record is in its spool. Either
way, check the daemon when the count on the server does not match.

`SPACE_STATION_HOME` moves `~/.space-station` (the spool, the lock, `auth.json`, the runtime).
The daemon's unix socket is `<home>/daemon.sock` as long as that path fits — the OS caps a socket
path at 104 bytes on macOS and 108 on Linux — and for a home inside a deep temp or CI directory
that would exceed it, the socket moves to `<system temp dir>/space-station-<hash of home>.sock`
instead, so only its location changes and two homes still never share a daemon.
`spacestation daemon status` prints where it is.

## 4. What a record looks like on the server

```json
{
  "metadata": {
    "record_id": "8b1e…", "table_id": "orders", "event_ts_ms": 1725000000000,
    "system": { "hostname": "…", "os": "…", "arch": "…", "cpu": "…", "cores": 8, "ram_mb": 16384 },
    "cpu_pct": 12.5, "gpu_pct": null, "ram_pct": 41.0, "disk_free_mb": 120000
  },
  "record": { "id": "o-42", "customer": "ada", "amount": 12.5 }
}
```

The library adds `metadata` (`system` is cached once per machine; the gauges are sampled at send
time and `null` when they cannot be measured). When a batch lands, the server adds two columns:
`cursor` — per table, starting at 1, assigned once and never reused — and `registered_ts_ms`.
Rows are stored in `event_ts_ms` order. The Tables overview (`spacestation tables overview
--window 5h`) measures "event → registered lag" as `registered_ts_ms - event_ts_ms` over the last
100 records.

## 5. Read it back

```
spacestation query "SELECT count() AS n, sum(record.amount::Float64) AS revenue FROM orders"
```

`record` and `metadata` are JSON columns, so cast every path you compare or sum, and note that
`count()` comes back as the string `"42"` — ClickHouse quotes every 64-bit integer; `toInt32(count())`
if you want a number. That and the rest of the dialect is in [SQL](/docs/sql). From there, a [Space Window](/docs/space-windows)
turns the same query into a live view, and a [Notification](/docs/notifications) turns it into a
message.

## <a id="limits"></a>Limits

All sizes are UTF-8 bytes of the JSON text as sent.

| what | limit | then |
|---|---|---|
| one string value | 32 KB | cut in the middle: `abc...[SIZE]...xyz`, SIZE = original bytes |
| a value that looks like a file (data URI, magic bytes, base64 blob, binary) | — | replaced by `"[FILETYPE:SIZE]"` |
| one record | 256 KB | refused before it leaves the process (`size_exceeded`) |
| one batch | 8 MB | the daemon splits; the server refuses a bigger frame |
| SiliconJSON, tool args, tool results | 64 KB | the processor run is a dev error |

## Next

[Space Windows](/docs/space-windows) · [SQL](/docs/sql) · [Notifications](/docs/notifications) ·
[Rust package](/docs/rust) · [CLI](/docs/cli) · [Credentials](/docs/credentials) ·
[HTTP API](/docs/api)
