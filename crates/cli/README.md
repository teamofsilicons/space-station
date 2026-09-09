# space-station-cli

`spacestation` is Space Station from the terminal — orgs, tables, space windows, notifications,
webhooks, keys, queries and the ingest daemon, under one binary.

It is a shell over the [`space-station`](https://crates.io/crates/space-station) crate and has no
capability of its own: **anything the command does, the crate does**, and nothing can appear here
that is not first a method there. What this crate adds is the tree, the flags, the columns, and
the links back to the web app.

```sh
curl -fsSL https://spacestation.teamofsilicons.com/install.sh | sh && export PATH="$HOME/.local/bin:$PATH"
```

`windows run` and `windows tool` also need Node >= 22.13 on `PATH`; nothing else does.

## Signing in

```sh
spacestation login --org tos                 # opens the browser, exchanges the returned short-lived token
spacestation auth <slt> --org tos            # a short-lived token from the iam CLI, exchanged for a session
spacestation whoami                          # id, kind, org, tags
spacestation orgs                            # the orgs you belong to
spacestation logout
```

Space Station never sees an IAM credential. What it takes is a **short-lived token** that IAM
minted for it — good for two minutes and for one exchange — which the backend trades for a session
it alone holds and refreshes:

```sh
iam login --app-id 'tos>spacestation' --org tos        # a carbon in a terminal: prints the token
iam silicon-login --app-id 'tos>spacestation'          # a silicon: the only way a silicon signs in
spacestation auth <slt> --org tos
```

The token is the argument, `-` to read it from stdin, or `$SPACE_STATION_TOKEN`. The command never
prompts, and `auth` refuses anything that is not a short-lived token before it leaves the machine:
a silicon's own `stk-` token, the Application's `ask_` secret, an IAM bearer (`sat_`, `cat_`,
`oat_`) or refresh token (`rft_`, `ort_`) is answered with `error: local: not a short-lived token`,
exit 1, nothing sent and nothing echoed. Those never leave the machine; only the `oac_` token IAM
minted to be handed over does. `login` is the browser flow instead, brokered by Space Station (only
the backend holds the Application secret): the command listens on `127.0.0.1`, opens the sign-in
page, catches the short-lived token redirected back, then exchanges it for an `sscli-` session;
`--no-browser` prints the link.

A session lasts as long as IAM lets the backend refresh it, and IAM ties that to the actor's own
IAM login: a **new** IAM login by the same actor — a second device, `iam login` with an email code
again, `iam silicon-login --sid … --stk …` again — makes IAM refuse the refresh of the older Space
Station sessions, so within about 30 minutes they answer `error: <code>: …` on a 401 with the way
back in, and `auth` (or `login`) again is the remedy. Minting another short-lived token from the
IAM session you already have (`iam login --app-id …`, `iam silicon-login --app-id …` without
`--stk`) is not a new login and ends nothing.

A session is bound to exactly one org — `--org` (or `$SPACE_STATION_ORG`), else the org already
stored — and that org is stored beside it, so later commands need no flag. Another org is another
`login` or `auth`; IAM completes it without a prompt while its own session is good. `logout` ends
the terminal's session at the server and does not sign the browser out.

Whatever is stored lives in `~/.space-station/auth.json` (0600, in a 0700 directory, written under
a lock through a tmp file and a rename). `whoami` names what is stored on stderr, so it says
something even offline.

## The org

Every org command needs one, and takes the first of:

```sh
spacestation --org tos tables ls     # the flag
SPACE_STATION_ORG=tos spacestation tables ls
spacestation use tos                 # remembered for later commands
```

A session already carries the org it was signed in to; `use` is for naming it again. When no org
is known the command says so and names every way to set one, without asking the server anything.

An `apikey-` or `spacewindow-` credential can act instead of the stored one — `--api-key`,
`--access-token`, or their `SPACE_STATION_*` variables — and, not being stored, it carries no
stored org, so name one with `--org`. Left empty (`SPACE_STATION_ACCESS_TOKEN=` as the repo's
`.env` exports it, `--api-key ""`), a credential or an org is absent, not blank: the stored session
and its org act.

## Commands

```sh
spacestation tables ls
spacestation tables get orders
spacestation tables create orders --access @alice,tech    # prints the table key once
spacestation tables access orders --access @alice,@bot:tos
spacestation tables rotate orders                          # prints the new key once
spacestation tables rm orders                              # the records go too
spacestation tables overview --window 5h

spacestation record --table-key "$TABLE_KEY" '{"order_id":"o_9","amount":12.5}'
printf '%s\n' '{"order_id":"o_10","amount":7}' | SPACE_STATION_TABLE_KEY="$TABLE_KEY" spacestation record

spacestation windows ls
spacestation windows get w_01
spacestation windows code w_01                             # processor and renderer of the current version
spacestation windows create "Orders" --access @alice
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
triggers, sql, delay?, cooldown?, access}, "recipients": [...]}`. On `edit`, absent recipients
leaves the subscribers alone.

`windows publish` refuses a processor or renderer containing anything shaped like a Space Station
secret (`spacewindow-…`, `apikey-…`, `whsec-…`, `table-…`) or an IAM credential (`stk-`, `ask_`,
`sat_`, `cat_`, `oat_`, `ort_`, `rft_`), even in a comment; the backend runs the same check. `windows run` and `windows tool` fetch the access token and start
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
never on the message. A 401 means the session is over — the backend holds and refreshes it, so
there is nothing to retry with here — and the message ends with how to sign in again.

## Environment

| variable | default | meaning |
|---|---|---|
| `SPACE_STATION_URL` | `https://backend.spacestation.teamofsilicons.com` | the Space Station origin (`/api` is appended) |
| `SPACE_STATION_HOME` | — | explicit home override |
| `SILICON_HOME` | `$SILICON_HOME/.space-station` | base for this app's `auth.json`, runtime, spool and daemon socket |
| `SPACE_STATION_ORG` | — | the org to work in, under `--org` and over the one stored |
| `SPACE_STATION_TOKEN` | — | the short-lived token `auth` exchanges, when it is not an argument |
| `SPACE_STATION_API_KEY` | — | act as this `apikey-` key |
| `SPACE_STATION_ACCESS_TOKEN` | — | act as this `spacewindow-` token |
| `SPACE_STATION_TABLE_KEY` | — | the ingest key used by `record` |

An empty variable is the same as an unset one. Nothing here reads a `.env` file or talks to IAM,
so a local stack is one variable:

```sh
SPACE_STATION_URL=http://localhost:8080 spacestation auth <slt> --org tos
```
