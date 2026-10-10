# Silicon Accounts cutover

The app now uses Silicon Accounts sign-in and Silicon Apps publishing. Organization selectors and organization API routes are removed. Every operation uses the current Carbon or Silicon account, identified by its immutable Accounts UUID.

## Existing data

PostgreSQL and ClickHouse retain their physical namespace columns for data compatibility. These are storage partitions, not organization authority. `account_owners` maps account UUIDs to those partitions. New accounts receive their own UUID partition. The legacy `tos` partition is explicitly assigned to the verified `si:tos` account, UUID `0080c488-a9e9-4c22-aa91-1c7a639f1d7b`; records and configuration remain in place. Other legacy partitions must be assigned explicitly after verifying their intended account. Never infer ownership from a mutable handle.

Historical numbered SQL migrations remain unchanged so installed databases can verify their checksums. Migration 0011 introduces Accounts session and ownership state. Old IAM sessions require a new login; they cannot be interpreted as Accounts tokens.

## Deployment order

1. Register `spacestation` in Silicon Apps under `si:tos`; save the app secret privately.
2. Configure the HTTPS callback and signed Accounts webhook, and save its signing secret.
3. Back up PostgreSQL. Retain `SS_KEY`, database credentials, table keys, and existing record partitions.
4. Store the new `SILICON_ACCOUNTS_*` configuration in the existing AWS runtime secret.
5. Refresh the host’s root-readable runtime environment, build and deploy the backend, then deploy the frontend.
6. Sign in through browser and CLI, verify the expected account and existing tables, exercise recording/query and logout, then publish the native packages.

The backend installer refuses to roll back from Accounts to an IAM executable. Subsequent Accounts builds can roll back to a compatible prior Accounts binary.

## Session behavior

The browser keeps an HttpOnly cookie until the Accounts refresh-token expiry. The CLI keeps a protected session file per server and profile. The backend holds encrypted upstream tokens, refreshes them under a database lock, and preserves sessions on temporary network errors. Logout revokes the upstream session and removes the local credential; signed account events also revoke removed or signed-out sessions.
