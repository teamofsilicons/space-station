# External bugs and contract gaps

Things found while building Space Station that belong to someone else — today only Silicon IAM.
Sanitized: no tokens, keys, OTPs or contact details; request ids are kept because they are what
gets a specific failure investigated upstream. Each entry says how it was observed, what the
contract or docs say, and its status on the date last checked. Re-verify before reporting
upstream; IAM ships fast (its CLI went 1.1.1 → 1.2.0 → 1.2.1 during 2026-09-05, fixing several of
these).

Observed against IAM's testing environment (id ends `…6ce8`) with `iam` 1.1.1, 1.2.0 and 1.2.1,
the `silicon-iam-client` crate 1.2.1, and raw HTTP. "Contract" = `openapi.yaml` served by the
service that day; "docs" = /docs/api/ and the manual bundled in `iam docs`.

## Production recheck — 2026-09-06

The deployed IAM browser login works. Space Station completed the real browser round trip at
`https://spacestation.teamofsilicons.com`; the earlier hosted-login failure in row 12 is historical.
The CLI uses the public application callback, followed by its own local completion redirect, so
IAM receives a public HTTPS redirect URI. Other historical rows below have not all been rechecked.

## Open

| # | What | Evidence | Expected | Status 2026-09-05 |
|---|---|---|---|---|
| 1 | The crate named `silicon-iam` ("Official Rust SDK") implemented a PKCE flow the service no longer has and rejected live canonical `{org}>{handle}` ids. The maintained client is `silicon-iam-client` (1.2.1). | crates.io + source read; `/api/v1/oauth/authorize` and `/oauth/token` absent from the live contract. `cargo search silicon-iam` now lists `silicon-iam = "0.0.0"` with the old description | The placeholder's description to point at `silicon-iam-client` | Open: the name is now a `0.0.0` placeholder, still described as the official SDK |
| 2 | `POST /api/v1/app-auth/tokens` answers `415 request_rejected` ("The HTTP request was rejected") to a JSON body; only the contract says `application/x-www-form-urlencoded`, the prose does not | curl, JSON body → 415; form body → 200 | A 415 naming the accepted type, or one sentence in the applications doc | Open (still 415, docs still silent) |
| 3 | `iam signup` has no `--code` flag and prompts on `/dev/tty`, so it cannot be scripted in a testing environment where every code is `000000`; `login` and `step-up` do take `--code` | `iam signup --help` (1.2.1) has no `--code`, while its own text says "In non-interactive use, supply credential/code flags explicitly; no hidden prompts" | `--email-code`/`--phone-code`, or `--code` applied to both | Open in 1.2.1; the help text now contradicts the flags |
| 5 | `app create` with a loopback `base_url` is rejected at the AWS edge with an HTML 403 (no request_id); the docs show loopback examples | CLI 1.1.1 "Forbidden (unrecognized_error)"; 1.2.x explains it is an intermediary | Docs to say loopback needs a local IAM (`--url`); the CLI's hint is good | Open (behaviour), docs partly fixed. See 12 for the general rule behind it |
| 6 | `silicon-iam-client` edits the *dependent project's* `Cargo.toml` at runtime ("Automatic crate updates … on by default … runs the equivalent of `cargo update`") when it finds a newer release | crate README, `iam docs client/updates` (1.2.1) | A library must never modify its host's manifest; at minimum default-off with an explicit opt-in | Open: still default-on in 1.2.1; the opt-out exists (`Client::builder(..).auto_update(false)` or `SILICON_IAM_CLIENT_AUTO_UPDATE=false`) and Space Station sets it |
| 7 | `silicon-iam-client` requires Rust 1.98 four days after 1.98.0 shipped; any consumer on an older stable cannot compile it | crate `rust-version` (1.2.0 and 1.2.1) | State the MSRV policy prominently | Noted (this machine is on 1.98.1; the backend's `rust-version` is 1.98, the published crates stay at 1.88 by not depending on it) |
| 12 | **MAJOR — a carbon cannot complete the browser login against the real service.** (a) The hosted UI `https://auth.iam.teamofsilicons.com` answers Vercel `404 DEPLOYMENT_NOT_FOUND` for every `GET /api/v1/login` redirect, production included. (b) The AWS edge answers an HTML 403 (no request_id) to `GET /api/v1/login` whose `redirect_uri` is loopback — `http://localhost:3201/…`, `127.0.0.1`, private IPv4, any `*localhost*` hostname — while `http://[::1]:…` and `*.local` pass: a generic URL-in-query-argument rule (it also hits `GET /api/version?x=http://localhost:3201`) that the application layer contradicts (`422 "HTTP is limited to loopback development"`, rid `01a070fe-a579-7231-a0c4-0370a1a7…`). (c) A browser has no way to select the testing plane, so even a working UI could not sign a test carbon in | Browser + curl against production and the testing environment | A reachable login UI; the edge rule scoped so a documented loopback development redirect passes; a way for the hosted login to carry `--test` | Open. Space Station's browser login is built to the contract and works against the stub; against the real service carbons sign in through `iam login --app-id` + `spacestation auth` |
| 13 | The edge answers an HTML 403 (no request_id) to **any** request without a `User-Agent` — `GET /api/version` with the version header, `POST /oauth/introspect` with correct Basic credentials | curl `-H 'User-Agent:'` → 403 HTML; the same request with any UA → 200 | Documented, or a JSON 4xx with a code. `reqwest` sends no UA by default, so every Rust integrator hits this | Open; Space Station sets one explicitly |
| 14 | Revoking one refresh family's `ort_` flips the **sibling** families' access tokens of the same Application login session to `active: false`, while their refresh still answers 200 and mints an active token again | 9/9 runs; rids `01a07100-5426-7000-bbd8-6b685beefb02`, `01a07100-57c7-72b3-856a-537ee579bb7d`, `01a07100-6fc0-7492-b430-d83d062d3e76` | The revoke doc says "access authority issued for the same Application session" — if intended, introspection and refresh disagree about whether the session is over; if not, a bug | Open; Space Station refreshes before it ends a session on `active: false` |
| 15 | A **new IAM login by the same actor** (`silicon-auth/token` again; a carbon on a second device) makes the **older** families' refresh answer `400 invalid_grant` while their access tokens stay active until expiry. Two families minted from the *same* login coexist and both refresh | Probe 2026-09-05 16:05; reproduced in use 2026-09-05 17:48, rid `01a07181-ff54-7363-9d8e-7c7019e3ef92`: a second `iam silicon-login` for `bot:tos`, then the older Space Station session's refresh at its next 60 s re-check → `invalid_grant`, session ended about a minute after its next command | Documented as a consumer consequence: it means one live Application session per actor per device generation, and an Application that refreshes eagerly (as Space Station does at every re-check) ends the older session within about a minute of its next use — not at the access token's expiry | Open (undocumented). Recorded in ARCHITECTURE "Identity" and the Credentials doc |
| 16 | A tag or role change bumps `authorization_epoch` and the held access token introspects `active: false` although the member is active; a refresh yields a live token | Tag added to `bot:tos` between two introspections | Documented nowhere as a consumer consequence: benign changes force a refresh, and a consumer that ends sessions on `active: false` logs its users out on every tag edit | Open (undocumented) |
| 17 | The removal-webhook member row is a **tombstone** `{"authorization": "removed", "resource": {id, principal_id, principal_type, status: "removed", type: "organization_membership", version}}` — no `principal`, `organization`, `membership` object, no `removed_at` — documented only in the openapi `WebhookEvent.data` prose ("stable resource/version authorization tombstones"); the webhooks page implies full member rows | Captured live twice on `organization.silicon.removed.v1` and `organization.carbon.removed.v1` | The webhooks page to show the tombstone shape and say a consumer must resolve it by membership or principal id | Open (docs) |
| 18 | `GET /applications/{app}/webhook/dead-letters` lists `event_type` without the `.vN` suffix for some rows (`application.created`, `session.logout`) while deliveries carry `.v1` and the contract's pattern requires it | CLI/curl listing | One vocabulary | Open (polish) |
| 19 | A duplicated, identical `X-Testing-Environment-Key` header answers `401 unauthenticated` with `WWW-Authenticate: Bearer` on every plane-selectable route, instead of `400 invalid_request` like a duplicated `X-Org-ID` | rids `01a07105-5480-7240-a1b6-45e9c24d908c`, `01a07105-580f-7981-b98c-5befb30e163b`, `01a07105-8379-7ad2-9a76-faeb436d1b05`, `01a07106-e5ad-7a00-b1b2-787115f3b381` | `400 invalid_request` (the header is malformed, the caller is not unauthenticated) | Open (polish) |
| 20 | `POST /app-auth/short-lived-tokens` replays with the same `Idempotency-Key` omit `Idempotency-Replayed`, while `/app-auth/tokens` sends it | rids `01a07109-b5da-7e92-a086-7fc563ee4e6f`, `01a07109-bbd3-74d1-a780-47d50e45cafd` (omitted) vs `01a07109-d2df-78d0-a15a-bf3bf27fa47c` (sent) | The conventions page allows omission, so this is consistency, not a defect | Open (polish) |
| 21 | Observation: login challenges are rate limited to roughly 3–6 per carbon per 10 minutes (`429`), which throttles test automation that relies on the fixed `000000` code | rids `01a070af-2ed7-7b20-be48-61673a5ce195`, `01a070af-3411-7660-8a97-bfaf81b40a9a` | A higher or configurable limit inside a testing environment | Observation |
| 22 | Observation, not a defect: a superseded access token stays active after its refresh token is rotated, until its own expiry | Introspection after refresh | Matches the contract (the access token has its own lifetime) | For the record |

Also for the record, from the operator's runs of 2026-09-05 (a backend killed mid-flush and
restarted; Redis frozen with `SIGSTOP` for a minute and resumed): nothing IAM-side misbehaved.
Sessions kept refreshing, introspection kept answering, webhooks kept verifying — every failure
seen was Space Station's own and is fixed on our side, so no row.

## Fixed or answered since first observed

| # | What | Then | Now (1.2.1 / contract of 2026-09-05 afternoon) |
|---|---|---|---|
| 4 | Accepting a carbon-id invitation through the CLI answered `404 not_found` for the invitee (`iam --test … --org tos invite accept <id> --code 000000` as bob) | 404 on 1.1.1 and 1.2.0 | **Answered on 1.2.1**, re-tested by hand: the invitee must first request the code with `iam --test … --org tos invite code <their email>` (`{accepted: true, expires_in: 599}`), after which the same `invite accept <id> --code 000000` joins the org (`status: active`, `org_role: member`). Without that step the accept still answers `404 not_found` (rid `01a07138-4782-7930-9063-6cfc645165f9`) although `invite show` displays the invitation to the same caller — a misleading status; `409`/`422` naming the missing step would do. Side effect of the re-test: bob is now a member of `tos` in the testing environment. **Still open inside this row:** `iam invite code` takes `<EMAIL>` as its only argument (`iam invite code --help`, 1.2.1: "Send yourself the verification code for an email invitation"), so an invitation issued by **carbon id** has no CLI way to request the code without an email — the invitee must know and type the address IAM holds for them (rid `01a07139-06a4-74b0-8e04-aba32b7860c2`, 2026-09-05). A `--carbon-id` alternative, or no code step for a carbon-id invitation, would close it |
| 8 | Application access tokens get `403 forbidden` on every directory/org/me route despite carrying `memberships.read organizations.read roles.read profile` | 403 everywhere; no way to learn tags except webhooks | Still 403 **by design**, but introspection of an org-bound token now returns an `authorization` snapshot with `public_id`, `principal_id`, `org_role`, `tags`, `membership_version`, `scopes` — "a synchronous bootstrap/resynchronization snapshot… webhook snapshots are asynchronous updates, not prerequisites for initial access". Answered |
| 9 | Nothing delivered on webhook activation, so an Application could not bootstrap its view of a member | confirmed 0 deliveries on `set-webhook` | Answered by the introspection snapshot above |
| 10 | `POST /oauth/introspect` with a *matching* `X-Org-ID` answered `401 unauthenticated` | reproduced | Fixed: answers `active: true` with the snapshot |
| 11 | `iam` CLI credential store not locked: 32 parallel local logouts lost writes and corrupted `credentials.json` (from the owner's Browser report) | 2 parse failures, 18/32 sessions still stored | Fixed: 0 failures, 0 remaining (reproducer re-run on 1.2.0); `iam docs storage` now documents the locking. 1.2.x also refuses an IAM home that is not mode `0700` |

## Working as documented (for the record)

Short-lived-token minting by browser redirect (against the stub), `iam login --app-id`, `iam
silicon-login --app-id` and `POST /app-auth/short-lived-tokens`; exchange with `Cache-Control:
no-store`; refresh rotation; reuse detection revoking the family (`400 invalid_grant`); single-use
slt; revoke (unknown tokens 200); org-bound logins into any org the actor belongs to, and
`organization_context_forbidden` — a legitimate, non-enumerated 403 code — for a short-lived token
into an org the actor is not in; one Application receiving directory events from several
organizations; HMAC signatures with a caller-chosen webhook secret and a key-version header; the
testing-environment header and fixed `000000` codes; the `{"test": {testing_key, metadata, data}}`
envelope; introspection with a well-formed but mismatched `X-Org-ID` → `{active: false}`, and a
malformed or duplicated one → `400 invalid_request`; silicon logout is local (`POST /logout` with a
`sat_` → 403, and `iam docs cli` says so: rotate or remove the Silicon to revoke its credential);
the 1.2.1 credential-store shape (`sessions`, `test_sessions`, `testing_environment_keys`) written
by hand into a private `SILICON_IAM_HOME` is accepted as-is.
