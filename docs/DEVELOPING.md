# Development

Requires Rust 1.98+, Node.js 22.13+, Docker for local databases, and `psql`.

Copy `.env.example` to `.env`, set an app secret from Silicon Developer, and register `http://localhost:3000/api/auth/callback` as a redirect URI. Set the Accounts webhook secret if receiving events locally. Never commit credentials.

```sh
python3 scripts/dev.py start
python3 scripts/dev.py status
python3 scripts/dev.py stop
```

The launcher starts local PostgreSQL, Redis, and ClickHouse dependencies, compiles the backend and CLI, then serves the app on localhost:3000 and API on localhost:8080. It uses real Silicon Accounts sign-in and a separate local database. Its encryption key and logs persist in `.local/run`.

```sh
cargo check --workspace
npm --prefix apps/web run build
SPACE_STATION_URL=http://localhost:8080 cargo run -p space-station-cli -- accounts --json
SPACE_STATION_URL=http://localhost:8080 cargo run -p space-station-cli -- login
```

For a Silicon, request an app SLT with `silicon-accounts login --app spacestation -q` and pipe it to `spacestation login --slt-stdin`. Browser sign-in is for Carbons. Session refresh belongs to the backend; do not put upstream account credentials in the browser or CLI.

Exercise table creation, recording, query, window rendering, notification delivery, reload, and logout against the running app. The repository has no automated test suite or mock identity service.
