# IAM 5 account and organization contexts

Space Station uses a vendored copy of the published IAM SDK 5.2.1 and requires a canonical Carbon or Silicon identity plus exactly one organization. Ordinary credentials cannot include OBO scopes. Active introspection must agree on actor, organization, canonical membership, audience, expiry, authorization epoch and testing environment. The [local patch](../vendor/silicon-iam-client/SPACE-STATION-PATCH.md) accepts canonical webhook membership strings while retaining signature and envelope verification.

## Browser

Each login adds an independent encrypted session. The organization menu lists saved account/organization contexts and offers an explicit new login. Selecting a context changes the selected cookie and navigates to a fresh page. Tabs, forms, sockets and caches therefore belong to the rendered context; tab persistence includes that context's public ID. Requests require `X-Spacestation-Context`, the public ID returned by `/api/me`; mission-control sockets carry the same ID as `account_context`. These markers are not credentials. An old tab receives `409 context_changed` when its cookie changes underneath it, including another account in the same organization. Refresh and delayed response parsing cannot retarget a page.

`GET /api/auth/contexts` lists this browser's saved contexts. `POST /api/auth/context` takes `{context_id}` with the current marker and same-origin request headers. Context references are server-side, scoped to a random HttpOnly browser-group cookie; IAM credentials never reach JavaScript. Logout removes only the selected ordinary session and selects a remaining context when present. OBO grants are not involved in logout.

The login page offers Carbon and Silicon choices. The backend seals the chosen kind with an expiring callback nonce and verifies it after exchanging the token. Popups report only completion status and a one-use attempt ID to the exact app origin; the opener checks origin, window and attempt before reloading. Blocked popups fall back to full-page login, and closed popups cancel without discarding the page. A login carries its sealed, expiring nonce embedded in the IAM callback URL. Callback state is checked before exchange. Login receipts and server-keyed session IDs recover a lost callback/exchange response without issuing a second local session; receipts remain after logout so replay cannot recreate it. A browser code cannot be replayed into a CLI session or another browser group.

## CLI

Use `spacestation --profile work login` and `spacestation --profile personal login` to keep independent account/organization credentials. `SPACE_STATION_PROFILE` selects the same profile; default is `default`. Profile names allow 1–64 lowercase letters, digits, hyphens and underscores. Files are owner-only and grouped under `iam5/<server-hash>/<profile>/auth.json`. Different server URLs have separate stores; the recording daemon retains its existing shared home.

`--org` and `use` verify the profile's immutable organization. Sign in under another profile to use another organization. Legacy root `auth.json` is retained but not adopted; sign in again after migration. Delayed logout and 401 cleanup compare the original saved credentials under the file lock before removing them.

## Backend migration and recovery

Migrations `0009_iam5_contexts.sql` and `0010_iam_login_attempts.sql` are additive. Existing rows have contract version 0 and cannot authenticate; new sessions use version 5. No tables, windows, records or organization resources are deleted. Old binaries must not be restored after cutover because they do not enforce the new version fence.

Rows bind IAM URL, application and testing metadata. Test key/generation/version or cleaning changes invalidate the cached world and require restart and reauthentication; there is no production fallback. The existing database testing-key boundary remains in force.

Mission-control sockets revalidate every 60 seconds even while the client only listens. Logout or revoked membership closes the original connection. Each recheck also drops subscriptions whose tables or predicate dependencies are no longer visible and refreshes the identity used for notification delivery.

Refresh is serialized by a PostgreSQL row lock. Its receipt key derives deterministically from the original session and refresh token, so a process crash before commit reconstructs the same IAM mutation. New pairs are swapped atomically. An uncertain introspection preserves the rotated pair with an unusable expiry so the next request can recover; expiry is taken from active introspection rather than extending an idempotent replay's old TTL.

## Verification

Unit, CLI and frontend context tests exercise strict ordinary tokens, stale responses, profile isolation and conditional credential cleanup. The database-backed `saved_contexts_receipts_refresh_and_logout_remain_independent` test uses isolated PostgreSQL/Redis URLs (`SS_CONTEXT_TEST_DATABASE`, `SS_CONTEXT_TEST_REDIS`) and the local IAM fixture to cover separate organizations, saved browser selection, login replay, simulated interrupted refresh, concurrent refresh, legacy/world rejection and logout. Run it explicitly with `cargo test -p space-station-backend --lib saved_contexts_receipts -- --ignored`.

The full local ClickHouse-backed core and notification integration tests now pass with an isolated official ClickHouse 25.8.32.4 native ARM64 build, PostgreSQL 16 and Redis. The rendered SQL/row-policy and stale-watermark checks also execute against that server; configured-but-unavailable ClickHouse fails those checks instead of silently skipping. The IAM fixture requires one explicit `org_ids` selection, exposes authenticated testing-context metadata, and uses the real callback-state/browser-context handshake. Fifteen IAM fixture tests and strict backend all-target Clippy pass. Live IAM 5 verification remains a separate release gate; local fixture success does not prove deployed provider compatibility.

The additional `idle_browser_socket_closes_after_its_session_ends` regression opens a real cookie-authenticated WebSocket, ends its original session, and checks that the server closes it without receiving another application frame. It uses the same isolated database URLs and runs explicitly with `cargo test -p space-station-backend --lib idle_browser_socket -- --ignored`.
