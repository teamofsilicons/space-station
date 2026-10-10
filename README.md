# Space Station

Space Station records application events, queries them with read-only SQL, and turns them into live Space Windows and notifications. Every table, window, notification, webhook, and key belongs to a Carbon or Silicon account. Silicon Accounts supplies sign-in; Silicon Apps distributes the CLI.

Open [Space Station](https://spacestation.teamofsilicons.com), browse the [docs](https://spacestation.teamofsilicons.com/docs), or install the command:

```sh
silicon-apps install spacestation
spacestation accounts --json
spacestation login
spacestation login status --json
```

A Silicon gets a one-use app token from its signed-in Accounts CLI:

```sh
silicon-accounts login --app spacestation -q | spacestation login --slt-stdin
spacestation tables ls
```

For macOS and Windows while Silicon Apps target validation is unavailable, use the native archives on [GitHub Releases](https://github.com/teamofsilicons/space-station/releases). The website installer supports macOS and Linux:

```sh
curl -fsSL https://spacestation.teamofsilicons.com/install.sh | sh
```

Accounts are keyed by their permanent UUID. Public handles (`c:alice`, `si:tos`) identify them for display. Browser cookies and protected CLI credential files survive restarts. The backend encrypts Accounts refresh tokens and serializes refreshes; a network outage does not sign anyone out. A session ends on logout, revocation, or its Accounts expiry.

## Rust and JavaScript

```toml
[dependencies]
space-station = "0.4"
```

```rust,ignore
use space_station::{Auth, Space, SpaceClient};
let space = Space::new(space_station::default_url(), auth)?;
let rows = space.query("SELECT count() FROM orders", &Default::default())?;
let recorder = SpaceClient::new("table-orders-…")?;
recorder.record(serde_json::json!({"id": "o-42", "amount": 12.5}));
```

The Rust library exposes every operation. The CLI is a shell over it; the browser shows tables, live windows, and notifications. For Space Window development, install `@teamofsilicons/space-station`; for browser event recording, use `@teamofsilicons/space-station-web`.

## Repository

- `crates/shared`: limits, wire types, record sanitizing, secret detection.
- `crates/client`: account API, recorder, daemon, and embedded Space Window runtime.
- `crates/cli`: native `spacestation` command and private local sessions.
- `crates/backend`: Axum API, Accounts integration, SQL guard, ingestion, live views, and notifications.
- `apps/web`: SolidJS frontend using [Silicon UI](https://ui.teamofsilicons.com) and public docs.
- `packages/`: JavaScript runtime and browser recording SDK.
- `infra/production`: native AWS deployment; `infra/apps`: Silicon Apps publishing.

## Development and production

See [development](docs/DEVELOPING.md), [architecture](docs/ARCHITECTURE.md), [Accounts migration](docs/ACCOUNTS-MIGRATION.md), [production operations](infra/production/README.md), and [release publishing](infra/apps/README.md).

Automated test suites and the old identity fixtures have been removed. Validate changes with workspace builds, the frontend typecheck/build, and real account, ingestion, query, and logout smoke checks.

[MIT](LICENSE).
