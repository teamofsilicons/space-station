# Credentials

Three kinds of credential, one job each. Everything else is a table key.

| | auth token | access token | API key |
|---|---|---|---|
| shape | `ss_session` cookie · `sscli-…` bearer | `spacewindow-{32 hex}` | `apikey-{32 hex}` |
| acts as | you, inside the one org the session is bound to | you, inside one org | the org, within its scopes, as nobody |
| used by | the app, the CLI, the Rust package | `space-station-dev`, `windows run` | your own programs |
| where | minted by Space Station when it exchanges the short-lived token IAM gave you | window page · `spacestation token show` | Settings → API keys · `spacestation keys create` |
| lifetime | as long as it is used; Space Station refreshes it | one live per actor per org; rotate any time | until deleted |
| stored server-side as | a session row holding the Application tokens, encrypted | encrypted, viewable | hashed, shown once |
| stored on your machine | `~/.space-station/auth.json`, by the CLI only | never, by anything of ours | never, by anything of ours |

Only three bearer shapes are accepted on `Authorization`: `sscli-`, `spacewindow-` and `apikey-`.
An IAM token of any kind is refused with `unsupported_bearer`: Space Station never receives one.

## Signing in: the short-lived token

Space Station is an *Application* registered with Silicon IAM, `tos>spacestation`. IAM never hands
an application a person's or a machine's credential; what it hands over is a **short-lived token**:
minted for this Application, good for two minutes and for exactly one exchange, bound to who you
are and to one organization. Space Station exchanges it with IAM — the only party that can, since
only it holds the Application secret — and what comes out is a **session** that Space Station alone
holds and refreshes. Nothing of yours crosses: not a password, not a verification code, not an IAM
bearer, not a silicon's `stk-` token, not an Application secret. What can expire in two minutes is
all that ever does.

Three ways to obtain one, one way to spend it:

| who | how the short-lived token is minted | how it is spent |
|---|---|---|
| a carbon in a browser | the app sends you to IAM's login for `tos>spacestation`, bound to the org you chose; IAM signs you in — or recognises you — and comes straight back | the callback exchanges it and sets the `ss_session` cookie |
| a carbon in a terminal | `spacestation login --org o` opens that same page and catches the short-lived token on loopback (`?slt=…&state=…`); without a browser, `iam login --app-id 'tos>spacestation' --org o` prints one | either way `POST /api/auth/session {slt, org}` — `login` does it for you, `spacestation auth <slt>` does it with the one you pasted — and the `sscli-` session comes back over that POST, never in a URL |
| a silicon | `iam silicon-login --app-id 'tos>spacestation'` prints one; the `iam` CLI is what holds the `stk-`, and it never leaves | `spacestation auth <slt> --org o` |

**One session, one org.** A login is bound to an organization, and so is the session it becomes:
`GET /me` answers `{id, kind, org, app}`, and every `/orgs/{org}/…` route of another org answers
`403 not_a_member`. To work in another org, sign in again for it — another org in the app's
sidebar is exactly that link, and `spacestation login --org other` is the terminal's — and IAM
completes the login without a prompt while its own session is good. A login that names no org is
refused with `org_required`; a login for an org you are not a member of never reaches Space
Station, because IAM refuses it.

**A carbon in a browser** holds an `ss_session` cookie. Because a browser attaches a cookie on its
own, every mutating request and every WebSocket upgrade must also carry an `Origin` matching the
app's. A browser holds one session at a time: signing in again — another org, another account —
ends the session the cookie you presented belonged to before the new one is set, so switching
never leaves a stale session alive behind you. (Cookies are per host, not per port: two Space
Stations on `localhost` with different ports share the cookie and sign each other out; a second
local stack belongs on `127.0.0.1` or `[::1]`.)

**In a terminal**, carbon or silicon, the credential is an `sscli-…` token: a *separate session
row* from any browser's, which the backend refreshes for as long as it is used. `spacestation
logout` ends that row and does not sign out your browser; signing out of the browser does not stop
the terminal. Being a bearer that no browser attaches by itself, it is not subject to the `Origin`
rule.

**What is stored, and where.** The Rust package stores nothing: `Auth` is a value. The session and
its refresh token live in the backend, so nothing on your machine ever rotates anything. The CLI is
the one that keeps things, in `~/.space-station/auth.json` — mode 0600 inside a 0700 directory,
written under a lock through a tmp file and a rename — holding exactly one credential (the
`sscli-` session) and the org it works in. Access tokens and API keys passed with `--access-token`
/ `--api-key` are not written there. `spacestation logout` deletes the file. `SPACE_STATION_HOME`
moves the directory.

## Who you are, and tags

Everyone is known by their IAM public id — `alice`, or `bot:tos` for a silicon — and shown as
`@alice` and `@bot:tos`. There are no display names in Space Station: the handle is the name.

Access to tables, windows and notifications is decided by **access lists** — `@actor` ids and IAM
**tag** names — checked against who you are in the org. Whoever creates something is on its list.
Deleting something needs the same access as reading it.

An access list is **matched, never validated.** An Application may ask IAM nothing about its
directory, so Space Station cannot know whether the tag `tech` or the member `@bob` exists; it
stores what you wrote and compares it, exactly and case-sensitively, with what IAM reports about
each person who signs in. A mistyped entry — `Tech`, `@alcie`, a tag that was renamed — is
accepted without a word and grants nothing. When someone cannot see what you shared, compare the
list (`spacestation tables get <id>`) with their `spacestation whoami` before anything else.

Tags reach Space Station **with your login**: an Application may ask IAM nothing about its
directory, but when Space Station verifies the session it just opened, IAM's answer carries your
id, your membership and your tags in that org — so `whoami` is right from the first command, and
Space Station re-reads it about once a minute while the session lives. In between, IAM's
**webhooks** keep it current: a tag renamed or reassigned, a member removed. Two consequences:

- **You are known in the orgs you have signed in to, plus those IAM's events have mentioned you
  in.** `spacestation orgs` and the sidebar list those; any other org you belong to is one login
  away.
- Renaming a tag in IAM changes who has access, and removing a member from an org revokes their
  sessions, access token and delivery webhook here. Both by design.

Your own membership is never taken from that mirror: the login proved it, and Space Station keeps
re-checking it with IAM while the session lives — at the first request more than a minute after
the last check, refreshing the Application tokens as it goes. Signing in to IAM afresh — a second
device, a silicon running `iam silicon-login` again — makes IAM refuse to refresh the older login's
tokens, so the older Space Station session of the same actor ends at its **next re-check**: about
a minute after its next command, with a `401`, not "eventually" and not at some token's expiry.
That is how IAM runs token families, and the newest login wins. A silicon that logs in to IAM
once per job therefore ends the session of the job before it; keep one IAM session and mint
short-lived tokens from it (`iam silicon-login --app-id …` without `--stk` reuses the stored one).

## Access token

Lets code you write act *on your behalf* inside one org — running queries and keeping a window's
state through mission control: to get data locally, to develop a Space Window with
`space-station-dev`, or to run one with `spacestation windows run`. It is tied to you and the
org, not to a window; there is one live token at a time, and both the window page and
`spacestation token show` show it with its last use, with **rotate** next to it.

It carries your identity, so treat it like a password: inside that org it can do what you can.
It belongs in `.env` and nowhere else. The runtime is built so your processor and renderer never
see it, and the server refuses to publish any code containing something shaped like it.

## API key

Programmatic, actor-less read access for the org: scope `tables` (list tables, the overview, run
queries) and/or `notifications` (list notifications and their events). Keys bypass access lists
within their scope and can do nothing else — every other route answers `401 unauthorized`. Create
them under **Settings**, or with `spacestation keys create --scopes tables,notifications`; the
key is shown once.

```
curl -H "Authorization: Bearer apikey-…" https://space.example.com/api/orgs/<org>/tables
```

## Table key

`table-{table_id}-{32 hex}` — what an app's `SpaceClient` sends records with. It resolves to one
table in one org, is stored hashed, and is the only credential that can write records. One per
table, rotatable, and shown once by `spacestation tables create` / `tables rotate` or the app.

## Webhook secret

`whsec-{32 hex}`, shown once when a webhook is created (`spacestation webhooks create <url>`, or
Settings → Webhooks) and again only if you recreate it. It signs outgoing notification
deliveries; see [Notifications](/docs/notifications) for the header and how to verify it.

## Carbons and silicons

A silicon is a machine identity in IAM, shown like anyone else as `@handle:org`. It can do
everything a carbon can: create tables and windows, read the code of windows it has access to,
get its access token, create and subscribe to notifications, query past events, write a window
locally and publish it for carbons. A carbon can do everything a silicon can, from the same
[CLI](/docs/cli) and the same [Rust package](/docs/rust).

Two differences remain, and neither is about capability. A carbon may sign in from a browser and a
silicon cannot, so a silicon always brings a short-lived token from `iam silicon-login` to
`spacestation auth` (a carbon may do the same with `iam login`); and a notification addressed to
a silicon is POSTed to its own delivery webhook (`spacestation webhook set <url>`), signed like
any other webhook, while one addressed to a carbon shows up in the app. `spacestation webhook rm`
removes that delivery webhook again; a notification that still names the silicon then records a
dev error at delivery, as if none had ever been set.
