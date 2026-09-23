# Space Station public identifier cutover

Space Station uses `c:alice`, `si:bot`, and the bare application ID `spacestation`. Organizations remain explicit authorization context. Bundle IDs such as `tos>interface` retain their existing grammar. The contract is described in the live [IAM OpenAPI](https://docs.iam.teamofsilicons.com/openapi.yaml) and [Honeycomb application configuration](https://docs.honeycomb.teamofsilicons.com/application-config/). The backend uses a vendored published IAM SDK 4.0.0 snapshot with a narrow canonical webhook aggregate-ID verification fix; its provenance records the patch.

The Ting handoff is coordination guidance, not a description of Space Station storage. This repository has no Ting registration, Ting outbox, application catalog, or durable notification delivery outbox. Its identifier-bearing state is in PostgreSQL; ClickHouse records and Redis staged payloads are user data and must not be rewritten.

## Authoritative inventory

Export IAM's collision-checked `iam_private.public_id_schema_map` using authenticated operator tooling, separately for production and each testing world, including removed identities and historical memberships. Keep the export with database snapshots and migration evidence. Empty `scope_key` means production; test scope keys must be the exact IAM world identifiers, not an invented label or an organization ID.

Create the following JSON from that export and the matching IAM organization/membership inventory. Normalize `actor_type` to `carbon`, `silicon`, or `application`. Carbon identity `org_id` is `null`; its organizations come exclusively from membership rows. Silicon/application identity `org_id` is the authoritative owner. Each membership must match one identity row and carry the verified organization UUID.

```json
{
  "scope_key": "",
  "identities": [
    {"scope_key":"", "actor_type":"carbon", "org_id":null, "old_id":"alice", "new_id":"c:alice"},
    {"scope_key":"", "actor_type":"silicon", "org_id":"tos", "old_id":"bot:tos", "new_id":"si:bot"},
    {"scope_key":"", "actor_type":"application", "org_id":"tos", "old_id":"tos>spacestation", "new_id":"spacestation"}
  ],
  "memberships": [
    {"scope_key":"", "actor_type":"carbon", "org_id":"tos", "old_id":"alice", "new_id":"c:alice",
     "org_uuid":"10000000-0000-0000-0000-000000000001", "principal_id":null, "membership_id":null},
    {"scope_key":"", "actor_type":"silicon", "org_id":"tos", "old_id":"bot:tos", "new_id":"si:bot",
     "org_uuid":"10000000-0000-0000-0000-000000000001", "principal_id":null, "membership_id":null}
  ]
}
```

The example UUID is a placeholder. Never fabricate missing authority. If a local projection contains a legacy IAM principal UUID or membership UUID, set `principal_id` or `membership_id` in the membership inventory to that exact, independently verified UUID. Leave the field `null` when the stored reference is already a public actor/membership ID. The migration preserves principal UUIDs and converts verified legacy membership UUID **references** to the canonical public membership ID, such as `c:alice[tos]`; this keeps SDK 4 membership-removal tombstones able to revoke existing access tokens. These fields are IAM projections, not Space Station resource primary keys.

Include all referenced Carbon memberships, including additional organizations and removed accounts. Include actors mentioned only by historical version authors, access lists, API-key creators, or notification recipients. An unknown recipient remains a blocker even if its old spelling looks obvious. A Silicon membership must agree with its IAM owner. Application mappings are collision-checked but applications are not stored as resource owners here; update deployment configuration separately.

The converter accepts the verified schema transformation or an already canonical source. It rejects guessed handle renames, duplicate mappings, Silicon/application collisions in their respective world namespaces, ambiguous actor aliases, mismatched kinds, conflicting private bindings, foreign organization UUIDs, missing actors, and colliding local old/new rows. Carbon and application handles can legitimately coincide. Resolve real handle collisions in IAM, coordinate existing data references, and rebuild the export before cutover.

## Maintenance and rehearsal

1. Rehearse on restored production and every test-world database with matching `SS_KEY` and deployment configuration. Preserve the original PostgreSQL, ClickHouse and Redis snapshots, IAM export, binaries, deployment configuration, and cryptographic keys together. The script's PostgreSQL archive is an additional backup, not a substitute for the coordinated snapshot.
2. Stop external writes, sign-ins and incoming IAM webhooks. Drain Redis `staging` and `flushing` normally. Pause new notification triggers and let existing delivery attempts finish while the old service is still running; retries start at 0, 10 and 60 seconds and each request can take additional time. Verify receiver receipts/logs for uncertain outcomes. Notifications use detached in-memory tasks with no durable pending-work registry, so process shutdown alone cannot establish delivery completion. Do not replay uncertain events under new identities or new keys.
3. Reconcile every `sessions.refresh_key` against IAM using its original idempotency key before the IAM cutover. Do not clear the column or delete the session to bypass this guard. Then stop **all** Space Station processes, workers, dev servers and other database clients. Confirm Redis `lease:engine` expires. Take the coordinated snapshots with writers stopped.
4. Apply IAM/Honeycomb's coordinated migration and export the authoritative inventory. Verify that the selected PostgreSQL/Redis deployment, IAM export scope, and `SILICON_IAM_TEST_KEY` identify the same world. Space Station's legacy schema contains no world identifier, so the first conversion requires this operator verification; the converter cannot infer that association from an organization or a public actor ID. Subsequent startup is fenced by the stored scope and testing-key SHA-256. Never use a production export for a testing database.
5. Run the offline preview, then apply with the same export. The tool uses Python standard library, PostgreSQL `psql`/`pg_dump`/`pg_restore`, and `redis-cli`; it makes no IAM requests and starts no service.

Load `DATABASE_URL`, `REDIS_URL`, and the selected deployment configuration through your normal private environment mechanism. Production must have `SILICON_IAM_TEST_KEY` unset; a test scope requires that deployment's existing key. Do not put secrets in shell history.

```sh
python3 scripts/migrate-public-identifiers.py /private/iam-world-map.json \
  --scope-key '' --offline

python3 scripts/migrate-public-identifiers.py /private/iam-world-map.json \
  --scope-key '' --offline --apply --backup /private/space-station-before-public-ids.dump
```

Use the exact nonempty IAM scope key for each testing database. `--offline` attests that all writers have stopped and external notification sends have been reconciled; it does not pause services. Both modes refuse a live Redis engine lease, staged/in-progress flush work, other PostgreSQL clients, and any pending IAM refresh. The script never deletes queue entries to satisfy a guard.

The tool verifies schema migration checksums through `0008`, locks the identity/resource tables, and performs the conversion in one transaction. It can install and register `0008` itself when the store is at `0007`. Preview executes the real conversion and rolls back, including any installation of `0008`. Apply first creates a new mode-0600 PostgreSQL custom archive without overwriting any file and checks that `pg_restore` can read its catalog. Failures roll back the conversion. Record the resulting counts, scope and mapping SHA-256; the database marker retains that digest. A second conversion is refused to protect sessions created after cutover.

Migration `0008` marks populated stores unready. The upgraded backend refuses to connect to IAM or start workers until the offline conversion completes. Empty stores can bootstrap normally and bind their configured IAM testing key (or production mode) on first startup; subsequent starts reject a different world. Do not run old binaries against converted data or bypass the marker.

## Data and credentials

The converter changes only these typed references:

- `iam_members.actor`, public `principal_id`, and `membership_id` projections; `access_tokens` actor/principal/membership references.
- Actor `created_by` fields on tables, windows and every window version, notifications and every notification version, webhooks and API keys.
- `@actor` entries in table/window access lists, notification-version `def.access`, and notification recipients; Silicon webhook actor indexes.

All table IDs, window/version/notification/webhook/API-key UUIDs, event IDs, timestamps, directory versions, organization IDs, roles, tags, hashes, resource ownership and notification cursors remain unchanged. Notification definitions retain their SQL, descriptions, triggers and other payload fields. Window processor/renderer code and saved state remain untouched. Notification events, developer-error history and IAM webhook deduplication IDs remain historical evidence. Provider URLs, arbitrary JSON/user text, ClickHouse records and Redis payloads are not globally replaced.

Encryption audit: `crates/backend/src/crypto.rs` uses AES-256-GCM with empty associated data. Stored access tokens and webhook secrets have no actor/app/organization binding in their encryption context, so their nonce/ciphertext bytes and hashes are retained exactly; keep the same `SS_KEY`. No credential rotation is needed. Verify existing token retrieval and webhook signing against the restored store before reopening traffic. Changing public IDs does not authorize changing webhook URLs: update an externally changed endpoint through the normal explicit configuration workflow.

Sessions have an explicit **invalidate and log in again** policy: after all refreshes are reconciled, the transaction deletes PostgreSQL `sessions` rows. Browser cookies and CLI `sscli-` bearers then fail with the normal login instruction. IAM access/refresh token ciphertext remains in the private pre-cutover backup only; the offline command cannot revoke IAM-side families, which must follow the coordinated IAM credential policy. It does not edit opaque tokens. Existing `spacewindow-` access tokens, table ingest keys, API keys, app secrets and webhook secrets retain their values. Restart discards all in-memory identity/visibility caches, and re-login obtains current IAM authorization.

Set `SILICON_IAM_APP_ID=spacestation` and update explicit actor selectors to `@c:alice` / `@si:bot`. Preserve organization selectors and Honeycomb bundle IDs. Deploy SDK-compatible backend/CLI/frontend and Honeycomb packaging together. Start behind the maintenance boundary and verify Carbon/Silicon login, old access-token continuity, table/window/history access, notification recipients and webhook signing, canonical membership-removal revocation, cross-org denial, and wrong-test-world rejection before reopening traffic.

## Verification and rollback

```sh
python3 scripts/test-migrate-public-identifiers.py
```

This creates and removes an isolated local PostgreSQL database using libpq settings (`PGHOST`, `PGPORT`, `PGUSER` as needed), never `DATABASE_URL`. It also uses Node's built-in crypto to authenticate preserved AES-GCM ciphertext. The check covers production and two testing scopes, actual preview rollback, installation/checksums, retained resource keys/history/secret bytes, readable credentials, canonical membership projections, session invalidation, pending refreshes, late unmapped recipients, kinds, ownership/private-binding collisions, repeat-apply refusal and wrong-world rollback. Synthetic local fixtures are not evidence that the production restore or cross-service cutover has succeeded.

To also exercise the command-line wrapper, set `SS_IDENTIFIER_TEST_REDIS_URL` to a **dedicated empty** testing Redis database when running the same check. It refuses a nonempty database, temporarily creates and removes the lease/staging/flushing guard keys, and verifies real CLI preview/apply, readable mode-0600 `pg_dump` backups, and refusal to overwrite an existing backup. Its PostgreSQL database and backup directory remain isolated and are removed afterward.

Before reopening writes, rollback means stopping upgraded services and restoring the coordinated PostgreSQL/ClickHouse/Redis, configuration, binaries and keys together with IAM/Honeycomb. Never roll back only the Space Station binary or one upstream service. After accepting new writes, stop traffic and reconcile them before a restore or fix forward; restoring the old snapshot would lose those writes. Keep the mapping and matching snapshots for the recovery window.
