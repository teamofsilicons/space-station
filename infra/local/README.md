# local stack

From the repo root, `python3 scripts/dev.py start` builds and starts the entire v1 stack at
`http://localhost:3000`, including the fixture IAM login (org `tos`, Alice). It creates the
separate `space_station_v1` Postgres database and uses Redis database 6; it preserves data and
leaves `.env` untouched. `python3 scripts/dev.py status` shows the processes; `stop` stops only
the processes that launcher started. Logs are under `.local/run`. Requires Python 3, `psql`,
Rust 1.98+, Node 22.13+, and Docker. The commands below are the individual-service alternative.

```
docker compose -f infra/local/docker-compose.yml up -d --wait   # clickhouse, postgres, redis
docker compose -f infra/local/docker-compose.yml down -v        # wipe
cp .env.example .env
```

| service    | url                                                        |
|------------|------------------------------------------------------------|
| clickhouse | http://dev:dev@localhost:8123/space_station (native: 9000) |
| postgres   | postgres://dev:dev@localhost:5433/space_station            |
| redis      | redis://localhost:6379 (AOF, appendfsync everysec)         |

Postgres is on **5433**, not 5432: a Postgres installed on the machine itself usually owns 5432,
and a backend that quietly connected to it would build its schema in the wrong database. The
compose file maps `5433:5432`; `DATABASE_URL` in `.env.example` matches.

Then, in three terminals, all from the repo root:

```
cargo run -p space-station-backend --example iam-stub     # Silicon IAM look-alike on 127.0.0.1:8099 (prints the seeded logins)
cargo run -p space-station-backend                        # the server on :8080 (creates the ClickHouse/Postgres schema)
npm --prefix apps/web install && npm --prefix apps/web run dev   # the app on :3000
```

## Signing in

A carbon in the browser: open http://localhost:3000 and pick an org; the backend sends you to the
stub's login page, which lists the seeded carbons — or add `&as=alice` to its URL to skip it — and
IAM (the stub) brings you back signed in, bound to that org.

A carbon or a silicon from a terminal, in a fourth window. A session is bound to one org, so
`login` and `auth` take `--org` (or `$SPACE_STATION_ORG`, or the org stored by `use`):

```
export SPACE_STATION_URL=http://localhost:8080

cargo run -p space-station-cli -- login --org tos   # opens the stub's page; the slt comes back on loopback and the CLI spends it
cargo run -p space-station-cli -- whoami
```

Without a browser — a silicon always — `auth` takes a **short-lived token** (slt) that IAM minted
for this Application, and nothing else: the CLI never prompts for, accepts or stores a `stk-`, an
IAM bearer or a refresh token. Against the real IAM the `iam` CLI prints one (`iam silicon-login
--app-id 'tos>spacestation'`; `iam login --app-id 'tos>spacestation' --org tos` for a carbon). Against
the stub, the same two steps IAM takes: the silicon signs in to IAM with the `stk-` the stub printed
at boot, then asks IAM for a token for the Application:

```
sat=$(curl -s -X POST http://127.0.0.1:8099/api/v1/silicon-auth/token \
  -H 'Content-Type: application/json' -H "Idempotency-Key: $(uuidgen)" \
  -d '{"silicon_id":"bot:tos","silicon_token":"stk-0123456789abcdef0123456789abcdef"}' | jq -r .access_token)
slt=$(curl -s -X POST http://127.0.0.1:8099/api/v1/app-auth/short-lived-tokens \
  -H "Authorization: Bearer $sat" -H 'Content-Type: application/json' -H "Idempotency-Key: $(uuidgen)" \
  -d '{"app_id":"tos>spacestation"}' | jq -r .slt)             # org_id defaults to the silicon's own

cargo run -p space-station-cli -- auth "$slt" --org tos   # or `auth -` with the token on stdin, or $SPACE_STATION_TOKEN
```

The real `iam` CLI (1.2.1) speaks to the stub too: `iam --url http://127.0.0.1:8099 silicon-login
--sid bot:tos --stk stk-0123456789abcdef0123456789abcdef --app-id 'tos>spacestation'` prints the
same `{slt, expires_in: 120}` (set `SILICON_IAM_HOME` to a scratch directory, mode 0700, and
`SILICON_IAM_AUTO_UPDATE=false` first, so it touches neither your own `~/.silicon-iam` nor its own
binary). The slt is single-use and dies after two minutes; the `sat_` stays with the silicon and
never reaches Space Station.

Against the real IAM the browser row is currently closed — IAM's hosted login page answers a 404
and its edge refuses loopback redirect URIs (`docs/EXTERNAL-BUGS.md`) — so a carbon takes the
terminal path there too: `iam --test "$SILICON_IAM_TEST" login --email <you> --code 000000` once,
then `iam --test "$SILICON_IAM_TEST" login --app-id 'tos>spacestation' --org tos -o json` for an
slt and `space-station auth <slt> --org tos`.

The Rust package reads no file and no variable; the CLI reads no `.env`, and `SPACE_STATION_URL`
defaults to the *public* host — without the export, `login` would send you to the real Space
Station. `SPACE_STATION_HOME` moves `~/.space-station`, which is how to keep a test identity and
its spool out of your own.

A terminal session (`sscli-…`) and a browser session (`ss_session`) are separate rows on the same
account: `logout` in one leaves the other signed in.

**A second local stack needs a second host, not just a second port.** Browsers scope cookies per
host and ignore the port, so two apps on `localhost:3000` and `localhost:3100` share one
`ss_session` cookie and sign each other out at every login (a callback ends the session the cookie
it was handed belonged to). Run the second on `127.0.0.1` or `[::1]` — `SS_ORIGIN=http://127.0.0.1:3100`
and open the app at that address — with its own `PORT`, its own Postgres database
(`psql postgres://dev:dev@localhost:5433/postgres -c 'CREATE DATABASE space_station_two'`) and its own
Redis db index (`redis://localhost:6379/1`), or the two backends also fight over the engine lease.

## Tags come from the login; webhooks update them

An Application can read nothing about IAM's directory, but introspecting the session it just
opened returns the member's id, membership and tags, so `whoami` shows the seeded tags right after
`login` or `auth`, and the backend re-reads them every minute into its `iam_members` mirror. IAM's
webhooks are the updates in between; the stub sends none on its own. `POST /_stub/deliver` signs
and posts the webhook IAM would send, so the mirror can be changed by hand:

```
curl -s -X POST http://127.0.0.1:8099/_stub/deliver -H 'Content-Type: application/json' -d '{
  "url": "http://localhost:8080/webhooks/api/", "event_type": "organization.membership.updated.v1",
  "org": "tos", "members": [{"actor": "alice", "tags": ["tech", "oncall"]}, {"actor": "bot:tos", "tags": ["ops"]}]}'

curl -s -X POST http://127.0.0.1:8099/_stub/deliver -H 'Content-Type: application/json' -d '{
  "url": "http://localhost:8080/webhooks/api/", "event_type": "organization.silicon.removed.v1",
  "org": "tos", "tombstones": [{"actor": "bot:tos"}]}'
```

The body is `{url, org, event_type, members | tombstones, envelope?, event_id?, version?}`. A member
row takes `actor`, `tags` (names) and `status` (default `active`; anything else revokes that
actor's tokens, sessions and webhook in the org). A `.removed.v1` event type carries
**tombstones** exactly as IAM sends them — `{"authorization": "removed", "resource": {id,
principal_id, principal_type, status, type, version}}`, no member object — which the receiver
resolves by membership or principal id; `tombstones: [{actor}]` names who, and `members` named on
such an event become tombstones too. `"envelope": "test"` sends the testing-environment envelope
instead of production's; `event_id` repeats a delivery, `version` orders it. The signature uses the
seed's webhook secret, which is what `.env.example` carries.

## Who reads `.env`

The backend, from its working directory (hence "from the repo root") and only underneath the real
environment, so an exported variable still wins; and `space-station-dev`, from its `--dir`.
Nobody else. The stub defaults to `127.0.0.1:8099` and the app to `http://localhost:8080`, so
neither needs it.

`SS_ORIGIN` must be the app's own origin: it is what the cookie's Origin rule is checked against,
and it is the `app` in `GET /me` that `space-station windows open` follows.

Real IAM — its testing environment, which is the same service on isolated data: `SILICON_IAM_URL`,
`SILICON_IAM_APP_ID` (quoted, `'tos>spacestation'`: unquoted, sourcing it in bash redirects into a
file named `spacestation`; the backend's dotenv strips one pair of quotes), `SILICON_IAM_APP_SECRET`
(the `ask_`), `SILICON_IAM_WEBHOOK_SECRET`, `SILICON_IAM_TEST_KEY` (the environment key, sent on
every IAM request; root authority over the environment) and `SILICON_IAM_TEST` (the environment id,
what `iam --test` takes) live in `.env.test`, which `.gitignore` keeps out of the repo (`.env.*`).
Load it over `.env` (`set -a; . ./.env; . ./.env.test; set +a`) and never print, log or paste its
contents; the backend never logs the key either. The backend reaches IAM through the official
`silicon-iam-client` with its self-update disabled, and needs Rust 1.98 for it. How to register the Application and import it into a
testing environment is in `docs/DEVELOPING.md`. The webhook URL registered there ends in
`/webhooks/api/`; the backend answers the IAM webhook at that root path and at `/api/iam/webhook`
alike, and IAM needs a public HTTPS URL to reach it (a tunnel, in development).

## One backend at a time

`scripts/test.sh` runs every test group against this stack and prints a results table. Only one
backend may run at a time — the server and the integration test both take the Redis engine
lease, and only the holder flushes and fires notifications; the other looks alive and does
nothing. Stop `cargo run -p space-station-backend` before the integration test, and never run
two on one machine.
