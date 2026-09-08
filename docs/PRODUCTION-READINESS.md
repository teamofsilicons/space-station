# Production readiness — 2026-09-06

Live application: https://spacestation.teamofsilicons.com
API: https://backend.spacestation.teamofsilicons.com

The frontend is SolidJS, TypeScript and Vite, with IAM's logo, Plex fonts and visual style.
It is hosted on Vercel. The backend is a native Rust executable on a private EC2 instance,
with native PostgreSQL and Redis; native ClickHouse runs on a second private EC2 instance.
The dedicated ALB terminates HTTPS and supports direct WebSockets. All stack resources are
owned by `space-station-production` in `us-east-1`; the temporary shared-ALB attachments
were removed after DNS cutover.

## Verified against production

- Real IAM browser sign-in as a carbon in `tos`, and the production organization/table UI.
- The published Rust CLI installed from crates.io, signed in through the real IAM browser
  round trip and local completion callback, and returned the correct identity.
- Table creation, durable record ingestion, SQL reads, window creation and processor/renderer
  publication through the CLI. The retained `Launch check` window shows these smoke-test records.
- The browser rendered the production data in its sandbox and showed `Connected · Live`.
  New records appeared through subscriptions. CLI `summary` tools returned the same state.
- Publishing a new version from the CLI while the browser stayed open automatically refreshed
  its processor and renderer. This caught and fixed a missing version-refresh path in the Solid UI.
- IAM application webhook approved and active at `/webhooks/api/`. Two real IAM events were
  replayed after fixing Vercel's exact trailing-slash rewrite, and were stored in `iam_events`.
  The dead-letter queue was empty; unsigned requests returned `401 invalid_signature`.
- The Rust shared/client/CLI crates and the npm runtime are published at version `0.1.0`.
  A clean registry installation of `@teamofsilicons/space-station@0.1.0` succeeded, and its
  development server proxied authenticated requests to the live API. The temporary scoped
  npm publishing credential was revoked after publication.
- PostgreSQL and ClickHouse logical backups were uploaded to S3, downloaded and restored to
  separate probe databases. Both probes passed and removed their temporary databases.
- CloudWatch logs/metrics and native backup timers are active. Health, disk, memory, backup and
  EC2 recovery alarms were OK during the deployment checks. Infrastructure updates and the
  repeat native application deployment completed successfully.

## Regression checks

`scripts/test.sh` passed all seven groups: shared, client, backend, backend integration,
CLI, runtime and web. Counts: 8 shared tests; 48 client tests and one doctest; 111 backend
unit tests; two backend integration tests; 20 CLI unit and six CLI integration tests;
71 runtime tests; 24 frontend tests plus TypeScript and production build.
One explicitly opt-in live-IAM integration test remained ignored by the suite; the real
production IAM browser, CLI and webhook paths were checked separately above.
The frontend checks passed again after the version-refresh correction. Rust formatting,
shell syntax, Python compilation and CloudFormation template validation passed.

## Operations

See `infra/production/README.md` for deployment, backup restore and SSM commands. This is a
single API/database host and a separate single ClickHouse host, with retained encrypted disks,
daily logical backups and EBS snapshots. It has no database replica or automatic application
failover; a native service restart briefly interrupts requests. Capacity and backup alarms
are visible in CloudWatch; an external alert recipient has not been configured.
