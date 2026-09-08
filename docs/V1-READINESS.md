> Production deployment and SolidJS migration: see [PRODUCTION-READINESS.md](PRODUCTION-READINESS.md). The report below records the earlier local v1 review.

# V1 local acceptance — 2026-09-05

The backend, Rust client, CLI and minimal web app run together at
http://localhost:3000. The backend readiness endpoint is
http://localhost:8080/api/health. The local launcher uses fixture IAM identities;
choose organization `tos`, then Alice. No public deployment or package publication was performed.

```sh
python3 scripts/dev.py start
python3 scripts/dev.py status
python3 scripts/dev.py stop
```

The launcher builds the backend and CLI, reuses the last build directory, starts the IAM fixture
and web app, and waits for healthy services. It leaves `.env` and the user's CLI login alone.
Local app data survives restarts. Logs are in `.local/run`; this machine's CLI is
`target/fix-backend/debug/spacestation`. For local CLI use:

```sh
export SPACE_STATION_URL=http://localhost:8080
target/fix-backend/debug/spacestation login --org tos
target/fix-backend/debug/spacestation tables ls
```

## Fixes made during acceptance

- Made Redis flush cleanup atomic and idempotent, preventing a crash from trimming the next
  batch of acknowledged records. Temporary ClickHouse failures keep staged records for retry.
- Added bounded backend readiness checks for Postgres, Redis and ClickHouse.
- Added CLI recording through the Rust client and kept terminal JSON output separate from
  window diagnostics. Successful reconnect delivery no longer reports an earlier transient
  connection error as a final failure.
- Fixed valid same-line default exports in processors, cleanup when navigating away during
  startup, malformed tool-schema feedback and failed notification-test exit codes.
- Completed basic notification editing/pausing, table refresh, cancellable forms, visible
  errors and restart controls. The frontend remains plain forms, lists and an iframe renderer.
- Updated documentation and stale tests left by the earlier refactor.

## Verification

`scripts/test.sh` completed with exit code 0. Shared, client, backend, backend integration, CLI,
runtime and web groups all passed. The web group includes TypeScript, an optimized Next build
and 39 component/helper tests. Runtime tests also passed on Node 22.13.1 and 26.8.1. Rust format
checks and client/backend/CLI clippy checks passed. Full suite output is in
`.local/run/final-checks.log`.

Independent exploratory sessions used the running product, with real local databases, beyond
the automated suites:

- [First-day browser user](qa-first-day.md): sign-in, tables, live window publishing/rendering,
  processor tools, notifications and settings. This session found the inline-export bug and
  confirmed the original processor works after the fix.
- [Rust SDK user](qa-sdk.md): standalone Rust consumer, one-shot delivery, long home paths,
  offline persistence, replay by a separate daemon, shared-daemon ingestion and reconnect.
  Five records were observed exactly once at cursors 1–5.
- [CLI operator](qa-operator.md): table ingestion/query, notification delivery with independently
  verified webhook HMAC, access grants/removals, key rotation and scoped API-key restrictions.

A final CLI session also opened the browser user's published `dayone v3` window: `windows json`
returned the saved three-order snapshot, `windows tool … order_detail` returned the selected
order, and `windows run` emitted parseable SiliconJSON on stdout while the view URL stayed on
stderr. The demo notification webhooks use a separate local receiver at port 8944; details
and its shutdown instruction are in the operator report.

A clean launcher stop/start succeeded. The dev-panel shortcut and logout worked; Alice could
sign into the separate `acme` organization, see its empty tables and `sales` tag, then switch
back to `tos`, where the demo data remained intact. Browser-tool handling of a native key-rotation
confirmation timed out in one tester's session; key rotation itself was verified through the CLI.

## Real IAM verification and launch dependency

A separate temporary backend used the real IAM testing environment, without the login fixture.
A silicon signed into IAM, obtained a short-lived application token and exchanged it through
the Space Station CLI. Identity and `ops` tags appeared immediately and remained valid after
the session recheck. The user created a table, ingested and queried records, rotated the table
key, confirmed the old key was rejected, and used a table-scoped API key to read while writes
and notification access were refused. The two accepted records had cursors 1 and 2. The
temporary API key was removed and the session logged out before stopping that backend.

Real IAM **browser login remains blocked upstream**: a fresh request to
https://auth.iam.teamofsilicons.com returned Vercel `404 DEPLOYMENT_NOT_FOUND`; the IAM login
redirect with a localhost callback returned HTTP 403. This is tracked in
[EXTERNAL-BUGS.md, issue 12](EXTERNAL-BUGS.md). Real IAM terminal authentication works; the
local browser walkthrough uses the explicit IAM fixture. A public carbon-facing launch needs
the hosted IAM login restored and the production callback checked. Hosting, public DNS and
registry publication are the next release step, after review.
