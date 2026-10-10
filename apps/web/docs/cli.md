# CLI

`spacestation` is Space Station from a terminal, for carbons and silicons alike. It creates
tables and rotates their keys, writes and publishes space windows, manages notifications,
webhooks, access tokens and API keys, sends records, runs queries, and runs the ingest daemon. Everything the
app can do is here, and a few things only here.

Install with Silicon Apps:

```sh
silicon-apps install spacestation
```

Silicon Apps chooses the package for your platform and keeps it updated. The native CLI runs on
macOS, Linux and Windows. `windows run` and `windows tool` also require Node >=22.13.

Build from a checkout with `cargo install --path crates/cli --locked`.

For the local app, export `SPACE_STATION_URL=http://localhost:8080` before using the CLI.
`python3 scripts/dev.py start` also prints the path to an already-built CLI.

## How it is layered

The [Rust package](/docs/rust) `space-station` is the interface: every capability is a method on
it. The CLI is a `clap` tree over that package plus output formatting, and holds **no capability
the package lacks** — a command exists only because a method exists. This page and
[the package's](/docs/rust) are the same surface, once as commands and once as functions. The
app is a subset of it.

The split is also one of state. The package is stateless: `Auth` is a value, `Space` is a
function of `(url, Auth)`, and neither reads a file or the environment. The CLI is the stateful
shell around it: it owns `<home>/accounts/<server-hash>/<profile>/auth.json` — the signed-in credential and its account identity — it reads the `SPACE_STATION_*` variables, and it opens the browser. Nothing on your
machine ever rotates a token: the session and its refresh live in the backend.

Where something genuinely needs pixels — a live renderer, a chart, a video — it stays in the app,
and the CLI prints the link instead of inventing a terminal renderer: `spacestation windows open
<id>` prints the window's URL and opens it if a browser is there, and commands with a useful page
print that page's link on stderr, so a terminal session can always hand off.

**Output.** `ls` prints aligned columns, `--json` the same list as JSON; everything else prints
JSON, pretty on a terminal and compact in a pipe, so `| jq` works. A secret goes to stdout alone —
nothing else on the line — with its note on stderr, so `spacestation tables create orders >
key.txt` captures the key and nothing more. A failure is `error: <code>: <message>` on stderr and
exit code 1; `code` is the backend's own snake_case code, or `local`, `transport`, `io` for what
never reached it.

## Sign in

```sh
spacestation login                           # Carbon: opens Silicon Accounts in a browser
silicon-accounts login --app spacestation -q | spacestation auth -  # Silicon: exchange a fresh SLT
spacestation whoami                          # id, uuid, kind and session expiry
spacestation logout                          # end the selected terminal session
```

`login --no-browser` prints the sign-in URL and waits for the same loopback handoff. Carbon
sign-in uses OAuth with PKCE; Silicon sign-in uses an app-bound `slt_` token that expires after
two minutes and can be exchanged once. Space Station never receives an STK.

Every command acts as the account in its credential. The session stays saved until it expires,
is revoked or you log out; network and service failures do not delete it. Browser and CLI
sessions are independent. Use `--profile personal` or `SPACE_STATION_PROFILE` to keep another
account without replacing the current one. Credentials are stored with owner-only permissions
under `<home>/accounts/<server-hash>/<profile>/auth.json` and replaced atomically.

An `apikey-` or `spacewindow-` credential can act instead of the saved session with `--api-key`
or `--access-token` (or `SPACE_STATION_API_KEY` / `SPACE_STATION_ACCESS_TOKEN`). These credentials
already identify the account that owns them.

## Commands

Global options, valid before or after the subcommand: `--json`, `--api-key <key>`,
`--access-token <token>`, `--profile <name>`.

```
login [--no-browser]                            a carbon: opens Space Station in a browser and keeps the session, for the selected account
auth [<slt>]                                    a short-lived token from the silicon-accounts CLI; `-` reads stdin; for the selected account
logout                                          forget the stored credential, and end the terminal session at the server
whoami                                          who this credential is: id, uuid, kind and expiry
record [<JSON>|-] --table-key <key>             send one JSON object; key also from SPACE_STATION_TABLE_KEY, no login needed

tables ls                                       every table you may see, with its records and its watermark
       get <id>                                 one table
       create <id>               prints the table key once
       rotate <id>                              prints the new key once; the old one dies
       rm <id>                                  the table and its records
       overview [--window 5h]                   1m 5m 15m 1h 5h 1d 7d 30d
```

The `si:tos` account owns Space Station's own telemetry table like any other table. Provision
it once with the normal command, store the one-time key in the deployment secret store, and send
events through the same `record`/daemon path:

```sh
spacestation tables create spacestation > spacestation.table-key
chmod 600 spacestation.table-key
```

Telemetry records should be self-contained and include at least `source`, `step`, `progress`,
`event`, and a context object. The daemon adds the normal record and system metadata; no separate
telemetry transport or privileged table path exists.

The Space Station frontend uses two additional `si:tos` tables: `spacestationfrontendanalytics` for
sampled automatic browser analytics and `spacestationfrontendevents` for explicit product events.
The browser never receives their table keys; its authenticated session posts batches to the
frontend collector. Install the reusable package with `npm i @teamofsilicons/space-station-web`.

```
windows ls | get <id>                           a summary: id, name, current version and its author
        create <name>            name under 20 characters
        edit <id> [--name n]     rename; prints the summary
        code <id>                               the current version's processor and renderer
        rm <id>                                 the window, its versions and its state
        versions <id>                           every published version: name, author, date
        publish <id> --name v --processor <file> --renderer <file>
        run <id>                                run the processor locally in Node until the window goes idle
        json <id>                               the cached SiliconJSON and its metadata
        tool <id> <name> '<json args>'          run one tool on the cached SiliconJSON
        open <id>                               print the window's page, and open it

notifications ls | get <id>
        create <file.json>                      the definition itself, or {def, recipients}
        edit <id> <file.json>                   a new version; absent recipients keeps the current ones
        rm <id>                                 the notification, its versions and its events
        events <id>                             what it has fired, newest first
        subscribe <id> | unsubscribe <id>       adds or removes you from recipients
        test <id>                               rows since the cursors, without advancing them

webhooks ls | create <url> | rm <id>            your account's webhooks; create prints the secret once; rm also
                                                drops webhook:<id> from every notification's recipients
webhook set <url> | rm                          this silicon's own delivery webhook; set prints its secret once

keys ls
     create --scopes tables,notifications       prints the API key once
     rm <id>

token show | rotate                             your access token, for space-station-dev and windows run

query "<sql>"                                   {rows, watermarks}
errors                                          what went wrong server-side for this account
daemon [run | status]                           the ingest daemon: foreground (the default), or is one running
```

## Retiring tables

```sh
spacestation tables retire orders
spacestation tables ls --retired --json
spacestation tables ls --all --json
spacestation tables get orders --json
spacestation query "SELECT * FROM orders LIMIT 10"
spacestation tables restore orders
```

Retirement blocks ingestion through the table's key and hides the table from the default list.
It preserves history, account ownership and window references; the web app lists it under **Retired**.
Restoration re-enables writes. Already accepted records can finish flushing after retirement.

## Deleting

`tables rm`, `windows rm` and `notifications rm` need the same access as reading the thing, and
take what belongs to it with them: a window takes its versions and its state, a notification
takes its versions and its whole event history, a table takes its records.

Table records go asynchronously. The table row disappears at once — the key stops working, the id
is free to reuse immediately — and the records are removed by a ClickHouse mutation that runs in
the background, so a query issued in the same second may still see a few. If the mutation fails,
it shows up in `spacestation errors`.

## Windows from the terminal

For a silicon, a Space Window *is* the processor's output. `windows ls`, `get` and `edit` print
a summary and never the code — a listing of twenty windows would otherwise be twenty programs —
and `windows code <id>` prints the current version's processor and renderer, which is how a
silicon reads one of its windows. `windows run` fetches your access
token, unpacks the runtime bundled in the binary into `~/.space-station/runtime/` and starts its
host in Node with only `PATH` and `SPACE_STATION_ACCESS_TOKEN` in the environment; the host
spawns the processor in a credential-less child process (`node --permission`, empty environment)
— the same boundary as the browser. The host keeps the server's copy of the SiliconJSON current
for as long as it runs and stops after 10 idle minutes. `windows json` reads that copy without
starting anything; `windows tool` runs one tool against it.

`windows publish` refuses code carrying anything credential-shaped before uploading; the server
checks again and answers `secret_in_code`.

The renderer is the one thing a terminal cannot show, so `windows open <id>` prints its page —
`{app}/a/{uuid}/windows/{id}` — and opens it when a browser is available. `get`, `create`, `edit`,
`publish` and `run` print the same link on stderr.

## Recording from the CLI

The same binary carries the ingest daemon that `SpaceClient` talks to:

```
spacestation daemon            one per machine, any number of table keys, in the foreground
spacestation daemon status     whether one is listening, and how many records are unacked
```

You do not have to run it: the first `SpaceClient` on a machine elects itself and runs one
in-process. Run it explicitly when you would rather it outlive your app — under systemd, or in a
container's entrypoint. Then it is this process, not the app, that hears the server's per-record
rejections (`unauthorized` after a key rotation, `size_exceeded`, `invalid`): they are printed
here and counted in `daemon status`, and the app's `on_error` never sees them. `daemon status`
also prints the socket path: `<home>/daemon.sock`, or — when the configured home is so deep
that this would exceed the 104/108-byte cap on unix socket paths — a short
`space-station-<hash>.sock` in the system temp directory instead. See
[Getting started](/docs/getting-started).

## Environment

| variable | default | meaning |
|---|---|---|
| `SPACE_STATION_URL` | `https://backend.spacestation.teamofsilicons.com` | the Space Station origin (`/api` is appended) |
| `SILICON_HOME` | `~/.silicon/.space-station` | base directory for this app's `auth.json`, runtime, spool, daemon lock and socket |
| `SPACE_STATION_HOME` | — | compatibility override used only when `SILICON_HOME` is unset |
| `SPACE_STATION_PROFILE` | `default` | independent saved account context |
| `SPACE_STATION_TOKEN` | — | the short-lived token for `auth`, instead of the argument |
| `SPACE_STATION_API_KEY` | — | act as this `apikey-` key |
| `SPACE_STATION_ACCESS_TOKEN` | — | act as this `spacewindow-` token |

The CLI reads no `.env` file, never talks to Silicon Accounts itself, and defaults to the public host: point
it at a local stack explicitly.

## Dev errors

Processor and renderer errors belong to the run that produced them: `windows run` prints them as
they happen. Notification, delivery and record-deletion errors are recorded server-side, and
`spacestation errors` lists them — the same list the app shows with Option+Shift+D.
