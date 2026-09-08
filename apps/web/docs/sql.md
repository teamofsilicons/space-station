# SQL

Everything that reads records — Space Window subscriptions, tools, notifications, `Space::query`
in the [Rust package](/docs/rust), `spacestation query`, API keys — speaks ClickHouse SQL
through one door.

## The mirage

You write `FROM orders`. There is no ClickHouse table called `orders`: every org's records live in
one physical table, and the backend parses your query, checks every table reference and rewrites
it to a sub-select scoped to your org and that table id. A row policy on the ClickHouse user
enforces the org boundary a second time. You only ever see your org's tables, and only the ones
your access lists let you see (API keys and the notification engine see the whole org).

Queries run read-only (`readonly=1`, `allow_ddl=0`) with a 10 s limit, at most 100 000 rows /
16 MB per result, and no access to system tables or logs.

## Columns

Every table has the same columns:

| column | type | |
|---|---|---|
| `cursor` | UInt64 | per table, from 1, assigned once when the batch lands; rising, never reused, but not gapless |
| `record_id` | UUID | from the client library |
| `event_ts_ms` | Int64 | when the app recorded it; the sort order |
| `registered_ts_ms` | Int64 | when the server registered it |
| `metadata` | JSON | what the library stamped: `system`, `cpu_pct`, `ram_pct`, … |
| `record` | JSON | what the app sent |

## Casting JSON paths

`record` and `metadata` are ClickHouse `JSON` columns, so a path such as `record.amount` has a
dynamic type. Cast it to use it:

```sql
SELECT record.id::String AS id, record.amount::Float64 AS amount
FROM orders
WHERE record.amount::Float64 > 5 AND metadata.cpu_pct::Float64 < 80
ORDER BY event_ts_ms DESC
LIMIT 50
```

- Cast every path you compare, sum or sort on: `::String`, `::Float64`, `::Int64`, `::Bool`,
  `::Array(String)`, `::DateTime64(3)` and so on.
- **64-bit numbers.** Integers inside `record` arrive as JSON numbers and lose precision above
  2^53. Send large ids as strings, and read them as `::String`.
- Key order inside `record` is not preserved.
- Missing paths are `NULL`; use `ifNull(record.x::Float64, 0)` when that matters.

## 64-bit integers come back as strings

ClickHouse quotes every `UInt64` and `Int64` in JSON output, so nothing is lost in transit above
2^53 — and so **every value of those types is a string** in what the query API, the CLI, the
package and a notification's rows hand you. That is the three columns, and it is also every
expression of that type, which is where it surprises:

| expression | type | arrives as |
|---|---|---|
| `count()`, `countIf(…)`, `uniq(…)` | UInt64 | `"42"` |
| `sum(record.qty::Int64)`, `max(cursor)` | Int64 / UInt64 | `"1180"` |
| `record.n::Int64`, `toInt64(…)`, `toUnixTimestamp64Milli(now64())` | Int64 | `"1725000000000"` |
| `sum(record.amount::Float64)`, `avg(…)`, `record.n::Int32`, `toFloat64(count())` | Float64 / Int32 | `12.5`, `42` |

So `{"n": "42"}` is not a bug, and `rows[0].n > 5` in JavaScript compares a string. Either cast
in SQL to a type that is not 64-bit —

```sql
SELECT toInt32(count()) AS n, toFloat64(sum(record.qty::Int64)) AS qty FROM orders
```

— or `Number()` it where you read it. One exception: the space window runtime turns exactly
`cursor`, `event_ts_ms` and `registered_ts_ms` into numbers before your processor sees a row,
because it needs them itself; nothing else is touched. `toFloat64` is safe for anything that fits
in 2^53 — every count and every millisecond timestamp for the next 280 000 years.

## Arrays

A JSON path is `Dynamic` and `ARRAY JOIN` wants an `Array`, so cast it in a derived table and
`ARRAY JOIN` that:

```sql
SELECT i.sku::String AS sku, count() AS n
FROM (SELECT record.items::Array(JSON) AS items FROM orders) ARRAY JOIN items AS i
GROUP BY sku ORDER BY n DESC LIMIT 10
```

`::Array(JSON)` for an array of objects (reach into each one with `i.sku::String`),
`::Array(String)` and friends for arrays of scalars. The cast cannot be written where the
`ARRAY JOIN` is — `::` there does not parse and `CAST(…)` there reads as a table function and is
refused — and `ARRAY JOIN record.items` without it is a ClickHouse type mismatch (`Code: 53`).
`arrayJoin(record.items::Array(String))` in the SELECT list works too, for scalars.

## Notifications: every row is a notification

A notification's SQL must return `dedup_key` (non-empty string ≤ 256 bytes), `text` (string) and
`metadata` (an object — `map(...)` renders as one). The select list is checked when the
notification is saved — a missing or extra column is `400 invalid_sql_shape` naming it — and the
types are checked on every row at run time (a bad row is a dev error, and nothing is sent):

```sql
SELECT record.id::String AS dedup_key,
       concat('Order ', record.id::String, ' for ', record.amount::String) AS text,
       map('amount', record.amount::String, 'customer', record.customer::String) AS metadata
FROM orders
WHERE record.amount::Float64 > 100
```

## Bounds

In a `delta` subscription and in a table-triggered notification the SQL only sees rows with
`cursor` in `(last cursor, watermark]` for each trigger table; a `snapshot` subscription, a
schedule-triggered notification and a plain query see everything up to the watermark at query
start. Watermarks are returned with every result so the client can carry on from them.

You can ask for a range yourself: `POST /query` takes `restrict: {table: {from?, to?}}` and
`Space::query(sql, restrict)` takes the same map. `from` is exclusive, `to` inclusive, and a
missing `to` becomes that table's watermark at query start — which is what comes back in
`watermarks`.

## What is refused

- Anything that is not exactly one `SELECT` (with `WITH`, `UNION`, sub-selects and joins allowed).
- `SETTINGS`, `FORMAT`, `INTO OUTFILE` — at any nesting level.
- Table functions: `url()`, `remote()`, `file()`, `s3()`, `mysql()`, …
- Qualified names (`db.table`), `system.*`, and any table your access lists do not include.
- Identifiers inside `IN (…)` that are not columns of the query (`x IN (system.one)`).

A refused query is an error with a `code` — a dev error in a window, `dev_errors` for a
notification, `Error::Api { code, .. }` from the package, a message in the CLI.

A table your access list does not include is refused the same way, and an access list is
**matched, never validated**: Space Station cannot ask IAM which tags exist, so `["Tech"]` on a
table whose readers carry the tag `tech` is saved without complaint and lets nobody but its
author in. When a query you expect to work answers `table_not_found`, check the spelling and the
case of the tag before anything else.
