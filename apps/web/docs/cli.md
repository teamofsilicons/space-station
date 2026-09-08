# CLI

`spacestation` is Space Station from a terminal, for carbons and silicons alike. It creates
tables and rotates their keys, writes and publishes space windows, manages notifications,
webhooks, access tokens and API keys, sends records, runs queries, and runs the ingest daemon. Everything the
app can do is here, and a few things only here.

Install on macOS or Linux (Intel/x86_64 and ARM64), without Rust or sudo:

```sh
curl -fsSL https://spacestation.teamofsilicons.com/install.sh | sh && export PATH="$HOME/.local/bin:$PATH"
```

The installer verifies checksums, installs under `~/.local`, and sets up PATH for future
terminals. It reuses compatible Node.js or installs a private runtime for `windows run` and
`windows tool`. On Linux distributions unsupported by official Node.js builds (such as Alpine),
install Node >=22.13 through the OS package manager to use those two commands; the native CLI
itself is statically linked on Linux. macOS binaries require macOS 11 or newer.

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
shell around it: it owns `~/.space-station/auth.json` — the one signed-in credential and the org
it works in — it reads the `SPACE_STATION_*` variables, and it opens the browser. Nothing on your
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

**The org.** `--org <org>`, else `$SPACE_STATION_ORG`, else the org stored with the credential. A
session is bound to the one org it signed in to (see [Credentials](/docs/credentials)), so that is
the org to name at `login` or `auth`; a silicon's is the suffix of its id (`bot:tos`). When no
org is known the command says so and names every way to set one, without asking the server
anything.

## Sign in

```
spacestation login [--org o] [--no-browser]   a carbon with a browser: IAM signs you in, a session bound to o comes back on loopback
spacestation auth <slt> [--org o]             a short-lived token the iam CLI minted for this Application; carbon or silicon
spacestation logout                           forgets the stored credential; ends the terminal session at the server
spacestation whoami                           id, kind, org, tags
spacestation orgs                             the orgs Space Station knows you in
spacestation use <org>                        the default org for later commands
```

**A carbon with a browser** runs `login`. It opens a listener on `127.0.0.1`, sends you to
Silicon IAM through Space Station for the org you named, and the callback comes back to that port
with the **short-lived token** IAM minted (`?slt=…&state=…`); the CLI then spends it exactly as
`auth` would, at `POST /api/auth/session`, and stores the `sscli-…` session that comes back — so
the credential itself is never in a URL, and what the browser saw dies in two minutes. It is its
own session row, so signing out of the terminal leaves your browser signed in, and the reverse.
`--no-browser` prints the URL to open somewhere else and waits for the same redirect. The backend
refreshes the session for as long as it is used.

**Without a browser** — a silicon always, a carbon on a remote machine — run `auth` with a
short-lived token the `iam` CLI minted for this Application:

```
iam login --app-id 'tos>spacestation' --org tos     # a carbon: IAM signs you in and prints the token
iam silicon-login --app-id 'tos>spacestation'       # a silicon: the iam CLI holds the stk-, and it never leaves it
spacestation auth <slt> --org tos
```

The token is the argument, `-` to read it from stdin, or `$SPACE_STATION_TOKEN`. It is good for
two minutes and for exactly one exchange, which is the whole point: `auth` posts it to Space
Station, which exchanges it with IAM and hands back the same `sscli-` session a browser login
would. Nothing prompts, and nothing here takes an IAM bearer, a refresh token, a silicon's
long-lived `stk-` or an Application secret — `auth` wants a short-lived token and nothing else.

**One session, one org.** The session is bound to the org it signed in to, and every later command
works there. To work in another org, sign in again for it: `login --org other`, or a fresh token
from `iam login --app-id 'tos>spacestation' --org other`. IAM completes either without a prompt
while its own session is good. `use <org>` only changes the default for later commands; a session
bound to another org answers `not_a_member` there until you sign in for it.

**`logout`** deletes `auth.json` and ends the terminal's session row at the server.

Whatever is stored lives in `~/.space-station/auth.json` (mode 0600, in a 0700 directory,
written under a lock through a tmp file and a rename) together with the org. `whoami` first says
on stderr what kind of credential is stored, so it says something even offline.
`SPACE_STATION_HOME` moves that directory, which is how to keep a second identity — and a second
spool — out of your own.

An `apikey-` or `spacewindow-` credential can act instead of the stored one — `--api-key`,
`--access-token`, or `$SPACE_STATION_API_KEY` / `$SPACE_STATION_ACCESS_TOKEN` — and, not being
stored, it carries no stored org, so name one with `--org`.

## Commands

Global options, valid before or after the subcommand: `--org <org>`, `--json`, `--api-key <key>`,
`--access-token <token>`.

```
login [--no-browser]                            a carbon: opens Space Station in a browser and keeps the session, bound to --org
auth [<slt>]                                    a short-lived token from the iam CLI; `-` reads stdin; bound to --org
logout                                          forget the stored credential, and end the terminal session at the server
whoami                                          who this credential is in this org: id, kind, org and tags
orgs                                            the orgs Space Station knows you in
use <org>                                       work in this org from now on
record [<JSON>|-] --table-key <key>             send one JSON object; key also from SPACE_STATION_TABLE_KEY, no login needed

tables ls                                       every table you may see, with its records and its watermark
       get <id>                                 one table
       create <id> [--access a,b]               prints the table key once
       access <id> --access a,b                 replaces who may use the table
       rotate <id>                              prints the new key once; the old one dies
       rm <id>                                  the table and its records
       overview [--window 5h]                   1m 5m 15m 1h 5h 1d 7d 30d

windows ls | get <id>                           a summary: id, name, access, current version and its author
        create <name> [--access a,b]            name under 20 characters
        edit <id> [--name n] [--access a,b]     rename, change who may open it, or both; prints the summary
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

webhooks ls | create <url> | rm <id>            the org's webhooks; create prints the secret once; rm also
                                                drops webhook:<id> from every notification's recipients
webhook set <url> | rm                          this silicon's own delivery webhook; set prints its secret once

keys ls
     create --scopes tables,notifications       prints the API key once
     rm <id>

token show | rotate                             your access token, for space-station-dev and windows run

query "<sql>"                                   {rows, watermarks}
errors                                          what went wrong server-side for this org
daemon [run | status]                           the ingest daemon: foreground (the default), or is one running
```

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
silicon reads a window it has access to. `windows run` fetches your access
token, unpacks the runtime bundled in the binary into `~/.space-station/runtime/` and starts its
host in Node with only `PATH` and `SPACE_STATION_ACCESS_TOKEN` in the environment; the host
spawns the processor in a credential-less child process (`node --permission`, empty environment)
— the same boundary as the browser. The host keeps the server's copy of the SiliconJSON current
for as long as it runs and stops after 10 idle minutes. `windows json` reads that copy without
starting anything; `windows tool` runs one tool against it.

`windows publish` refuses code carrying anything credential-shaped before uploading; the server
checks again and answers `secret_in_code`.

The renderer is the one thing a terminal cannot show, so `windows open <id>` prints its page —
`{app}/o/{org}/windows/{id}` — and opens it when a browser is available. `get`, `create`, `edit`,
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
also prints the socket path: `<home>/daemon.sock`, or — when `SPACE_STATION_HOME` is so deep
that this would exceed the 104/108-byte cap on unix socket paths — a short
`space-station-<hash>.sock` in the system temp directory instead. See
[Getting started](/docs/getting-started).

## Environment

| variable | default | meaning |
|---|---|---|
| `SPACE_STATION_URL` | `https://backend.spacestation.teamofsilicons.com` | the Space Station origin (`/api` is appended) |
| `SPACE_STATION_HOME` | `~/.space-station` | where `auth.json`, the runtime, the spool, the daemon lock and — unless the path is too long — the socket live (`daemon status` says where) |
| `SPACE_STATION_ORG` | — | the org to work in, under `--org` and over the one stored with the credential |
| `SPACE_STATION_TOKEN` | — | the short-lived token for `auth`, instead of the argument |
| `SPACE_STATION_API_KEY` | — | act as this `apikey-` key |
| `SPACE_STATION_ACCESS_TOKEN` | — | act as this `spacewindow-` token |

The CLI reads no `.env` file, never talks to IAM itself, and defaults to the public host: point
it at a local stack explicitly.

## Dev errors

Processor and renderer errors belong to the run that produced them: `windows run` prints them as
they happen. Notification, delivery and record-deletion errors are recorded server-side, and
`spacestation errors` lists them — the same list the app shows with Option+Shift+D.
