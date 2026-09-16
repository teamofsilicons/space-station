# Honeycomb distribution

Space Station is registered as `tos>spacestation` in the `tos` organization. Honeycomb distributes
the native CLI; the existing AWS backend remains a Rust executable supervised by systemd.
Neither the Honeycomb package nor production deployment uses Docker.

The integration adds Windows support to the existing Rust client/CLI and packages six native
executables, without changing the backend API, frontend, databases, or application identity.
Rust 1.89 supplies portable file locking. Space Windows processor commands need Node.js 22.13+
on PATH; Honeycomb does not run the website installer's Node setup.

## Registration

`application.json` is the complete nonsecret application configuration. Creation additionally
requires `webhook_secret`, read securely from the existing backend's AWS Secrets Manager record
`space-station/production/ApiRuntime`. Never commit that private input or the returned app secret.
Save the returned app secret as `SILICON_IAM_APP_SECRET` in the same AWS secret, refresh the
root-only runtime environment, and restart `space-station.service`.

```sh
iam login --app-id 'tos>honeycomb' --grant-org tos --approve-scopes
honeycomb login '<returned one-use token>'
honeycomb apps get 'tos>spacestation' --json
```

The application requests only identity, membership and tag disclosure. It exposes no delegated
endpoints and requests no external application scopes. Its existing signed IAM webhook receiver
is `https://spacestation.teamofsilicons.com/webhooks/api/`. Activating a newly registered receiver
requires IAM's fresh `application.webhook.approve` verification, bound to the internal application
UUID returned by `iam app webhook 'tos>spacestation'`.

## Release

Run `scripts/build-cli-release.sh` from macOS with the six Rust targets, Zig, LLVM/lld,
`cargo-zigbuild`, and `cargo-xwin` installed; the release workflow does this on native runners.
Both managed (`--features honeycomb-managed`) and standalone variants are built before
`python3 scripts/package-cli-release.py` packages them. The output is `dist/spacestation-honeycomb-<version>.tar.gz` with a
root `honeycomb.yaml` and per-platform executable mappings.

```sh
honeycomb validate dist/spacestation-honeycomb-0.1.3.tar.gz
honeycomb apps get 'tos>spacestation' --json
# Use the current revision from that response, and preserve the key on uncertain retries.
honeycomb --idempotency-key spacestation-release-0.1.3-0001 releases upload \
  'tos>spacestation' dist/spacestation-honeycomb-0.1.3.tar.gz --revision <revision>
honeycomb install 'tos>spacestation' --version 0.1.3
spacestation --help
```

Use Honeycomb's `--alias spacestation=<name>` if another installation already owns the command.
Honeycomb owns updates of its installed package; the CLI's standalone updater must not overwrite
those versioned files. New bytes require a new release version.

Public distribution is a separate Honeycomb publication request and validator review, after
upload and private installation verification. A registered application or an uploaded archive
alone does not mean public publication has completed.

## Verified on 2026-09-16

- Signed into Honeycomb through the existing IAM CLI session, sharing `tos`.
- Created the application through Honeycomb; IAM accepted it and the application is active.
- Updated only the AWS application's IAM credential, preserving database and webhook secrets.
- Restarted the native backend and verified health, real IAM token exchange, current identity,
  and authenticated access to the organization's 13 tables.
- Activated the signed IAM webhook through Honeycomb after IAM email verification. The receiver
  rejects unsigned requests with HTTP 401.
- Built and atomically deployed the native ARM64 backend on AWS; systemd and the public health
  endpoint passed after replacement. The previous executable is retained for rollback.
- The Honeycomb-managed macOS CLI authenticated and listed all 13 tables.

- All six native client/CLI test and build jobs passed in [CI run 35080807100](https://github.com/teamofsilicons/space-station/actions/runs/35080807100), source revision `3045cf8`.
  Native Windows tests caught and now cover canonical drive paths in disk-space telemetry.
- Uploaded that CI run's archive as immutable release `0.1.3` (12,033,680 bytes), SHA-256
  `4798af7601153e6ced3d02c9e7a918a8c0ab37c3e5cbe515c0aa470e31960859`. Honeycomb accepted it.
- Downloaded and installed `0.1.3` through Honeycomb into an isolated verification home. The
  installed command passed help/version checks, real IAM identity and authenticated table listing.
- Completed the real browser IAM round trip into the live `tos` tables page.
- Public review request `604daa17-ed26-4dad-97ad-058367d5a684` is `awaiting_validator`, revision 1.
  The only remaining gate is Honeycomb validation; the app is still private. The current
  organization-admin session has no global validator authority.

Check the remaining external review with:

```sh
honeycomb publication get 'tos>spacestation' --json
```
