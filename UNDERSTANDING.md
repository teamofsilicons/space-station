# Product intent

Space Station is an observability tool for Carbons and Silicons. A signed-in account owns tables of JSON records, live Space Windows, notifications, delivery webhooks, access tokens, and API keys. All capabilities belong in the Rust library first, the CLI exposes them, and the browser presents the parts that benefit from pixels.

Silicon Accounts replaces the former identity system. Silicon Apps replaces the former distribution system. Silicon Developer is the place to configure apps and sign-in, accessible through the platform CLIs. There are no organizations or identity testing environments. Use permanent account UUIDs for ownership and mutable `c:`/`si:` handles for display. Browser and CLI sign-in lasts until token expiry, revocation, or logout.

## Records

`SpaceClient::record` is non-blocking. It stamps record and system metadata, sanitizes large strings and file-like values, and hands records to a durable machine-local daemon. One connection carries all table keys. Acknowledgements remove data from the spool only after the server accepts it; retries preserve unacknowledged records.

The backend stages records in Redis, then flushes to ClickHouse every second or 16 MB. Each record has one stable cursor within its account and table. A watermark is the highest persisted cursor available to queries. UUID-based deduplication handles retry duplicates. Tables are logical: the SQL guard rewrites them to the correct storage partition and rejects unsafe sources and mutations.

## Space Windows

A window contains a versioned JavaScript processor and HTML renderer. The processor queries records, subscribes to triggers, exposes typed tools, and returns at most 64 KB of SiliconJSON. Callbacks are queued and failures do not advance cursors. The renderer runs in a sandboxed iframe with the resulting JSON, tools, and metadata, without account credentials. Silicons consume the processor output and tools directly through the CLI.

The JavaScript development server uses the same runtime and a private access token from `.env`. Published code is checked for accidentally embedded secrets. Carbon and Silicon users can create, inspect, version, and publish windows through the same library and CLI.

## Notifications

Notifications are always-on queries triggered by records or cron schedules, with delay and cooldown. Results include a deduplication key, text, and metadata. Notification history is append-only. Account-owned webhooks receive signed deliveries. The browser and CLI expose configuration, history, and developer errors.

## Interfaces

The browser has Tables, Space Windows, Notifications, and Settings, and uses [Silicon UI](https://ui.teamofsilicons.com). Accounts can switch saved contexts. The CLI and Rust package expose all resource creation, reading, editing, deletion, key rotation, queries, runtime tools, and session operations. Capabilities requiring a browser return a URL.

The browser event package supports framework-independent analytics and explicit event recording. The local daemon, runtime, and SDKs remain usable independently of the UI.

## Implementation principles

Use Rust for the backend, CLI, and main library; SolidJS for the frontend; PostgreSQL, ClickHouse, and Redis for storage. Prefer synchronous APIs for straightforward client work and asynchronous event-driven code for the server. Keep code small, preserve trust-boundary validation and error handling, and validate the real flows before publishing. There is no automated test suite.

Deploy the native backend on AWS and frontend on Vercel. Publish libraries and native packages. Credentials never enter source control or frontend bundles.
