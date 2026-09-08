# Rust SDK user walkthrough — 2026-09-05

The SDK was exercised as a developer using a standalone Rust application, outside the repository
test harness. The application lives in the ignored `.local/sdk-user` directory and depends on
`crates/client` by path. It ran against the local backend on port 8080, IAM stub on 8099, and the
real local Postgres, Redis and ClickHouse services. The identity was `@bob` in `tos`; test data
stayed in its own `sdkv1` table. Credentials were kept in private local files and are not included
in this report.

| User action | Observed result |
|---|---|
| Exchange a short-lived IAM token using `space_station::exchange`; read identity; create a table with `Space::create_table` | Signed in as bob, table created, table key saved privately |
| Record one JSON object with `SpaceClient`, call `flush`, exit the application | `flush` returned true; querying through the Rust API returned the record at cursor 1 |
| Record from a home path longer than the Unix socket limit | Delivery succeeded at cursor 2; the socket used the short temporary path while the spool remained under the requested home |
| Record while the destination refused connections, with a 300 ms flush timeout; exit | The error callback reported the connection failure; `flush` returned false; daemon status showed one unacknowledged record persisted on disk |
| Start a separate foreground daemon against that spool with the working backend URL | The stored record was replayed, acknowledged once, and appeared at cursor 3; unacknowledged count returned to zero |
| Run another short-lived Rust application against the already-running daemon | The record was handed off successfully, appeared at cursor 4, and daemon status remained at zero unacknowledged records |
| Run CLI `record` while a local connection relay was initially unavailable, then bring the relay online | The retry delivered the record; CLI exited 0 with empty stdout and a confirmation on stderr; the Rust query showed it once at cursor 5 |

The separate daemon used for this walkthrough was stopped after its spool was empty. The local
application and five sample records remain available for inspection. No deployment or public
service changes were made.

The reconnect walkthrough also verified a fix: a transient connection error from an earlier
attempt must not make the CLI report failure after the Rust client confirms successful delivery.
Permanent record validation errors and server rejections still fail the command.

Automated verification accompanied this walkthrough: 48 Rust client tests, 26 CLI tests, the
client doctest, and clippy passed. All 71 runtime tests passed on both Node 22.13.1 (the declared
minimum) and Node 26.8.1. The separate first-day browser walkthrough found and rechecked the
same-line `export default` processor bug; the original processor now renders live data and its
renderer can call its tool successfully.
