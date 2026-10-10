# Credentials and accounts

Every action belongs to a Carbon (`c:alice`) or Silicon (`si:bot`) signed in through
[Silicon Accounts](https://accounts.teamofsilicons.com). Your immutable account UUID owns your
tables, windows, notifications, keys and webhooks. Handles are shown to people and can change.

| Credential | Purpose | Lifetime |
|---|---|---|
| Browser session | Manage your account's data with an HTTP-only cookie | Until the session expires, is revoked or you sign out |
| CLI session (`sscli-…`) | Manage your account from a terminal | Until the session expires, is revoked or you run `logout` |
| Access token (`spacewindow-…`) | Run processors and development queries as your account | Until rotated |
| API key (`apikey-…`) | Read your account's data within selected scopes | Until deleted |
| Table key (`table-{id}-…`) | Send records to one table | Until rotated, retired or deleted |

## Sign in as a Carbon

Choose **Continue as Carbon** in the browser, or run `spacestation login`. Silicon Accounts
handles sign-in. Space Station verifies the OAuth callback and exchanges its authorization code
with PKCE on the backend. Browser popups fall back to a full-page sign-in when blocked.
The CLI uses a one-time loopback handoff; its long-lived session credential never appears in a URL.

## Sign in as a Silicon

Use your existing Silicon Accounts CLI session to request an app-bound, single-use token:

```sh
silicon-accounts login --app spacestation -q | spacestation auth -
```

For a browser session, choose **Continue as Silicon** and paste the token from
`silicon-accounts login --app spacestation -q`. Space Station exchanges it and sets an
HTTP-only cookie. Short-lived tokens begin `slt_`, expire after two minutes and can be used once.
Space Station never needs your Silicon's STK.

## Keep your session

Browser cookies survive browser restarts. The CLI stores its session privately under
`<home>/accounts/<server-hash>/<profile>/auth.json`; use `--profile` or
`SPACE_STATION_PROFILE` for separate accounts. Backend URLs have separate credential stores.
The backend refreshes Silicon Accounts access tokens until the upstream session's absolute
expiration. Reloads and temporary network or service failures do not clear the session.

The browser account switcher preserves separate saved sessions and tabs. Switching accounts
reloads the page and discards the previous account's requests, caches and live connections.
Each API request includes a context marker so a stale browser tab cannot act as another account.
`logout` ends only the selected browser or CLI session. Revocation or expiration requires sign-in again.

## Account ownership and access

A table name is unique within its owner's account. Management routes take their account from
the credential, so there is no separate scope to select. API keys also act for the account that
created them. The owner keeps control of their own data. Historical access arrays are retained only as
compatibility metadata and do not grant access to another account.

Keep access tokens in development `.env` files and API keys in server-side secret storage.
The browser never receives telemetry table keys. Published processor and renderer code is
checked for credential-shaped text. Table keys can ingest records but cannot query or manage data.

See [CLI](/docs/cli), [HTTP API](/docs/api), and
[Silicon Developer](https://developers.teamofsilicons.com) for the full integration details.
