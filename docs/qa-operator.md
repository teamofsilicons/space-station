# Manual operator acceptance — 5 September 2026

Tested the running local app through the published CLI surface and its Rust client, with
separate Alice and Bob identities. These were individual, observed user actions, separate from
the automated integration suite. The app used the local IAM fixture, Postgres
`space_station_v1`, Redis database 6, and the existing ClickHouse service.

| User action | Observed result |
|---|---|
| Sign in using the IAM CLI, then pass its short-lived token to `spacestation auth - --org tos` | Alice and Bob obtained separate app sessions. Credentials stayed in private local homes. |
| Create `operatororders` and read it back | One-time table key returned; zero records, watermark 0, Alice automatically included in access. |
| Configure a high-value-order notification and a local webhook | Definition, trigger, recipients, delay and cooldown saved successfully. |
| Send an order with `spacestation record`, then query it | `{id: "operator-001", amount: 120.5, customer: "Operator Ada"}` appeared with cursor 1 and watermark 1. |
| Read notification history and inspect webhook delivery | One stored notification and the expected `{dedup_key, text, metadata}` delivery. Independent Python HMAC verification passed. |
| Use a `tables` API key | Listing and querying tables succeeded. Creating tables, reading notifications, reading windows, and accessing another org were refused. |
| Query Alice's private table as Bob | Refused without revealing the table. Adding `ops` access admitted Bob immediately; removing it denied him immediately. |
| Rotate the ingest key | The old key was rejected with CLI exit 1; the new key accepted `operator-002`. The rejected record did not appear in queries. |
| Query `system.tables` | Refused by the SQL guard. |
| Test the notification after its matching event was consumed | Returned no new rows and the previous trigger timestamp. |
| Support the independent first-day browser user | Sent `dayone-001` to `dayoneorders`; its webhook payload and HMAC signature verified. No `dayone-paused` webhook arrived while paused. After enabling, `dayone-002` arrived with a valid signature. |

The retained demo table has two records. Its notification is `Operator high orders`
(`cc572e18-f398-42a7-90f0-9fd0c0b3b310`). Local operator CLI homes are `.local/operator` and
`.local/operator-bob`; all keys and sessions remain outside tracked files.

The temporary webhook receiver remains available at `http://127.0.0.1:8944/events` for the
browser demo and `/operator` for the CLI demo. At handoff its PID is **1158**. Captured raw bodies
and signature headers are in `/tmp/space-station-manual-webhooks.jsonl` (mode 0600). Stop the
receiver after the demo with `kill 1158`, after checking that PID still owns port 8944. This
receiver is separate from `scripts/dev.py` and must be running for these demo webhook URLs to
accept deliveries.

Automated checks also passed: 109 backend unit tests, the complete backend core integration,
the notification integration with real outbound webhooks and cooldown checks, and strict
backend Clippy across all targets. Backend integration tests used Redis database 7, separate
from the app. The throughput benchmark is deliberately ignored.
