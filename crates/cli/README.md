# space-station-cli

`spacestation` is Space Station from the terminal — account tables, space windows, notifications,
webhooks, keys, queries and the ingest daemon, under one binary.

It is a shell over the [`space-station`](https://crates.io/crates/space-station) crate and has no
capability of its own: **anything the command does, the crate does**, and nothing can appear here
that is not first a method there. What this crate adds is the tree, the flags, the columns, and
the links back to the web app.

```sh
curl -fsSL https://spacestation.teamofsilicons.com/install.sh | sh && export PATH="$HOME/.local/bin:$PATH"
```

Supports macOS, Linux, and Windows 10 1803 or newer. Windows uses native local sockets and
owner-only access controls for credentials and recorded data. `windows run` and `windows tool`
also need Node >= 22.13 on `PATH`; nothing else does.

Silicon Apps installs updates automatically; the CLI does not replace its own executable.

## Signing in

```sh
spacestation login                          # a carbon signs in through the browser
silicon-accounts login --app spacestation -q | spacestation login --slt-stdin
spacestation auth <slt>                      # equivalent token exchange; - reads stdin
spacestation login status --json             # account and session status
spacestation --profile work login            # an independent account profile
spacestation accounts --json                 # app id, Accounts service, source, docs and crate
spacestation report-bug "short summary" --details "steps and observed output" --pr-ref owner/repo#123
spacestation whoami                          # immutable UUID, public id, account kind
spacestation logout
```

A silicon asks Silicon Accounts for an app-bound `slt_` token, valid once for two minutes.
The backend exchanges it and keeps the Accounts access and refresh tokens. Space Station never
receives a silicon's STK. The CLI accepts only short-lived tokens for sign-in, through a positional
argument, `--slt-stdin`, `-`, or `$SPACE_STATION_TOKEN` with `auth`.

Carbon browser sign-in uses authorization code and PKCE through the backend. The CLI listens on
`127.0.0.1`, verifies callback state, and redeems a one-time handoff for its stable `sscli-` session.
`--no-browser` prints the sign-in link.

Sessions remain saved until their server-issued expiry, revocation, or explicit logout. Temporary
network failures and server errors keep the credential. `login status --json` reports
`authenticated: true, verified: false` when an unexpired saved session could not be checked.
The stored absolute expiration is enforced offline as well. Confirmed
expiration or revocation removes it and requires a fresh sign-in.

Each profile acts as one carbon or silicon. Every management request derives the account from
the credential. Use `--profile personal` and `--profile work` for independent sessions; logout
ends the selected CLI session. The browser has its own persistent session.

Credentials live in `<home>/accounts/<server-hash>/<profile>/auth.json` (0600 in a 0700 directory,
written atomically under an OS lock; owner-only DACLs on Windows). The profile defaults to
`default`; `SPACE_STATION_PROFILE` also selects one. Backend URLs have separate stores. The base
home is `$SILICON_HOME/.space-station`, else `$SPACE_STATION_HOME`, else `~/.space-station`.
Legacy credentials require a fresh Silicon Accounts sign-in.

Account UUIDs are immutable; `c:<handle>` and `si:<handle>` are changeable display IDs.
Notification recipients are your account or its webhooks. An `apikey-` or `spacewindow-` credential
can replace the stored session through `--api-key`, `--access-token`, or their environment
variables. Those credentials already identify their owning account.

## Commands

```sh
spacestation tables ls
spacestation tables get orders
spacestation tables create orders    # prints the table key once
spacestation tables rotate orders                          # prints the new key once
spacestation tables rm orders                              # the records go too
spacestation tables overview --window 5h

spacestation record --table-key "$TABLE_KEY" '{"order_id":"o_9","amount":12.5}'
printf '%s\n' '{"order_id":"o_10","amount":7}' | SPACE_STATION_TABLE_KEY="$TABLE_KEY" spacestation record

spacestation windows ls
spacestation windows get w_01
spacestation windows code w_01                             # processor and renderer of the current version
spacestation windows create "Orders"
spacestation windows edit w_01 --name "Orders live"
spacestation windows rm w_01
spacestation windows versions w_01
spacestation windows publish w_01 --name v1 --processor processor.js --renderer renderer.html
spacestation windows run w_01                              # runs the processor locally until idle 10 min
spacestation windows json w_01                             # the cached SiliconJSON + metadata
spacestation windows tool w_01 order_detail '{"order_id": "o_9"}'
spacestation windows open w_01                             # prints the page, and opens it

spacestation notifications ls
spacestation notifications create new-orders.json          # {"def": {...}, "recipients": [...]}
spacestation notifications get n_01
spacestation notifications edit n_01 new-orders.json
spacestation notifications rm n_01
spacestation notifications events n_01
spacestation notifications subscribe n_01
spacestation notifications unsubscribe n_01
spacestation notifications test n_01                       # runs its SQL now, cursors untouched

spacestation webhooks ls
spacestation webhooks create https://x.example/hooks       # prints the signing secret once
spacestation webhooks rm wh_1
spacestation webhook set https://bot.example/hooks         # this silicon's own delivery webhook
spacestation webhook rm                                    # remove this silicon's delivery webhook

spacestation keys ls
spacestation keys create --scopes tables,notifications     # prints the api key once
spacestation keys rm k_1

spacestation token show                                    # the spacewindow- access token + last use
spacestation token rotate

spacestation query "select record.price::Float64 as price from orders order by cursor desc limit 5"
spacestation errors                                        # notification and delivery dev errors

spacestation daemon                                        # the ingest daemon, in the foreground
spacestation daemon status                                 # is one running, and what does it still owe
```

`record` needs only a table key, without signing in. It uses the Rust client's sanitization,
disk spool and shared daemon. It waits up to five seconds for delivery when it owns the daemon;
with a daemon already running, it waits until that daemon has spooled the record.

The notification file holds the two arguments a definition takes: `{"def": {name, description?,
triggers, sql, delay?, cooldown?}, "recipients": [...]}`. On `edit`, absent recipients
leaves the subscribers alone.

`windows publish` refuses a processor or renderer containing anything shaped like a Space Station
secret (`spacewindow-…`, `apikey-…`, `whsec-…`, `table-…`) or a Silicon Accounts credential (`stk-`, refresh tokens, JWTs), even in a comment; the backend runs the same check. `windows run` and `windows tool` fetch the access token and start
the bundled `mission-control.js` runtime in Node with only `PATH` and `SPACE_STATION_ACCESS_TOKEN`
in its environment; the runtime then spawns the credential-less processor.

## Output

`ls` prints plain aligned columns, with `-` where a value is not there; `--json` prints the same
list as JSON instead, for scripting. Everything else prints JSON: indented on a terminal, one line
in a pipe.

Anything that needs pixels — graphs, live views, video — stays in the web app, so every command
with a page worth seeing prints its link on stderr and `windows open` opens it. Secrets go to
stdout alone with a one-line note on stderr, so `spacestation tables create orders > key.txt`
captures just the key.

Failures are `error: <code>: <message>` on stderr with exit code 1, where `code` is the backend's
own snake_case code (or `local`, `transport`, `io` for what never reached it) — branch on that,
never on the message. Confirmed expiry or revocation requires signing in again; temporary
failures leave the stored session intact.

## Environment

| variable | default | meaning |
|---|---|---|
| `SPACE_STATION_URL` | `https://backend.spacestation.teamofsilicons.com` | the Space Station origin (`/api` is appended) |
| `SILICON_HOME` | — | when set, the base home is `$SILICON_HOME/.space-station` |
| `SPACE_STATION_HOME` | — | compatibility alias, used when `SILICON_HOME` is unset |
| `SPACE_STATION_PROFILE` | `default` | independent saved account context |
| `SPACE_STATION_TOKEN` | — | the short-lived token `auth` exchanges, when it is not an argument |
| `SPACE_STATION_API_KEY` | — | act as this `apikey-` key |
| `SPACE_STATION_ACCESS_TOKEN` | — | act as this `spacewindow-` token |
| `SPACE_STATION_TABLE_KEY` | — | the ingest key used by `record` |

An empty variable is the same as an unset one. Nothing here reads a `.env` file or talks directly to Silicon Accounts,
so a local stack is one variable:

```sh
SPACE_STATION_URL=http://localhost:8080 spacestation auth <slt>
```
