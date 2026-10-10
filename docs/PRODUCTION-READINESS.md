# Production readiness — Silicon Accounts cutover

Live application: https://spacestation.teamofsilicons.com
API: https://backend.spacestation.teamofsilicons.com
Publisher: `si:tos`; Silicon Apps ID: `spacestation`.

The frontend is SolidJS, TypeScript and Vite using the Silicon UI foundations and controls, hosted on Vercel. The backend is a native Rust executable on a private EC2 instance with PostgreSQL and Redis. ClickHouse runs on a second private EC2 instance. The stack is `space-station-production` in `us-east-1`.

## Accounts migration

All application routes operate on the signed-in Carbon or Silicon account. Permanent Accounts UUIDs own data; mutable handles identify accounts in the UI. Organization selectors, organization routes, IAM dependencies and fixtures, the previous updater, and automated test suites are removed.

Migration 0011 assigns the retained `tos` storage partition to the verified `si:tos` UUID. The pre-cutover production inventory contained 40 tables, 25 windows and no notifications. Existing record partitions and table keys remain in place. Historical numbered migrations are immutable. Old IAM sessions require a new Accounts sign-in.

Browser cookies and protected CLI credentials retain the session until refresh-token expiry, revocation or logout. Temporary network failures preserve the saved session. The backend serializes refreshes and verifies signed Accounts events; account switching and logout affect the selected session.

## Release verification

The workspace compilation, frontend TypeScript/production build, shell syntax, Python compilation and whitespace checks passed. Frontend dependency audit reported no vulnerabilities. The real Accounts SLT grant and introspection returned the expected `si:tos` UUID and application audience. A fresh production PostgreSQL backup completed successfully before the migration.

Release versions are Rust shared/client/CLI `0.4.0`, npm runtime `0.2.0`, and browser telemetry `0.1.1`. All three Rust crates and both npm packages are published. Local checks against real Silicon Accounts passed CLI persistence across invocations, single-use token replay rejection, table creation, record ingest and query, window creation, access-token refresh, and logout. Production deployment and native package validation are recorded below when completed.

## Operations

See [production operations](../infra/production/README.md) and [Accounts cutover](ACCOUNTS-MIGRATION.md). This deployment has one API/database host and one ClickHouse host, retained encrypted disks, daily logical backups and EBS snapshots. It has no database replica or automatic application failover; service restarts briefly interrupt requests. The installer refuses rollback across the Accounts migration boundary.

Silicon Apps currently validates Linux targets only. macOS and Windows binaries are distributed through the website and GitHub release until Apps workers for those targets become available.
