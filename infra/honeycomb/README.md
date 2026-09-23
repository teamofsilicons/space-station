# Honeycomb distribution

Space Station uses app ID `spacestation` with owning `org_id: tos`. Honeycomb distributes
the native CLI; the existing AWS backend remains a Rust executable supervised by systemd.
Neither the Honeycomb package nor production deployment uses Docker.

The native package supports Windows and packages six executables for the Rust client/CLI.
Rust 1.89 supplies portable file locking. Space Windows processor commands need Node.js 22.13+
on PATH; Honeycomb does not run the website installer's Node setup.

## Registration

`application.json` is the complete nonsecret application configuration, using separate `app_id`
and `org_id` fields as documented in [Honeycomb configuration](https://docs.honeycomb.teamofsilicons.com/application-config/). Creation additionally
requires `webhook_secret`, read securely from the existing backend's AWS Secrets Manager record
`space-station/production/ApiRuntime`. Never commit that private input or the returned app secret.
Save the returned app secret as `SILICON_IAM_APP_SECRET` in the same AWS secret, refresh the
root-only runtime environment, and restart `space-station.service`.

```sh
iam login --app-id 'honeycomb' --grant-org tos --approve-scopes
honeycomb login '<returned one-use token>'
honeycomb apps get 'spacestation' --json
```

The application requests only identity, membership and tag disclosure. It exposes no delegated
endpoints and requests no external application scopes. Its existing signed IAM webhook receiver
is `https://spacestation.teamofsilicons.com/webhooks/api/`. Activating a newly registered receiver
requires IAM's fresh `application.webhook.approve` verification, bound to the internal application
UUID returned by `iam app webhook 'spacestation'`.

## Release

Run `scripts/build-cli-release.sh` from macOS with the six Rust targets, Zig, LLVM/lld,
`cargo-zigbuild`, and `cargo-xwin` installed; the release workflow does this on native runners.
Both managed (`--features honeycomb-managed`) and standalone variants are built before
`python3 scripts/package-cli-release.py` packages them. The output is `dist/spacestation-honeycomb-<version>.tar.gz` with a
root `honeycomb.yaml` and per-platform executable mappings.
New manifests use `app_id: spacestation`; organization ownership belongs in application
configuration, not the manifest. Use Honeycomb 0.4.0 or newer for the new identifiers.
The [package contract](https://docs.honeycomb.teamofsilicons.com/package-format/) rejects unknown manifest fields.

```sh
# Choose a new, unpublished version in crates/cli/Cargo.toml before building.
version='<new-version>'
honeycomb validate "dist/spacestation-honeycomb-$version.tar.gz"
honeycomb apps get 'spacestation' --json
# Use the current revision from that response, and preserve the key on uncertain retries.
honeycomb --idempotency-key "spacestation-release-$version-0001" releases upload \
  'spacestation' "dist/spacestation-honeycomb-$version.tar.gz" --channel prod --revision <revision>
honeycomb install "spacestation@$version"
spacestation --help
```

Use Honeycomb's `--alias spacestation=<name>` if another installation already owns the command.
Honeycomb owns updates of its installed package; the CLI's standalone updater must not overwrite
those versioned files. New bytes require a new release version.
Release selectors retain their channel meaning: `spacestation>test@1.2.3` selects a development
release, while `spacestation@1.2.3` selects production. A bundle still uses `org>bundle`.
Do not rebuild or overwrite the published `0.1.3`/`0.1.4` archives below to rename their manifest;
Honeycomb's catalog migration preserves their exact bytes and verified legacy identity.
Migrate installed registries through Honeycomb's approved mapping tool before updating packages;
see the [Space Station cutover](../../docs/PUBLIC-ID-MIGRATION.md).

Public distribution is a separate Honeycomb publication request and validator review, after
upload and private installation verification. A registered application or an uploaded archive
alone does not mean public publication has completed.

## Verified on 2026-09-16

This is pre-migration evidence for the former `tos>spacestation` identity. It does not claim
that the new identifier deployment or a new release has been published.

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
- Public review request `604daa17-ed26-4dad-97ad-058367d5a684` was approved and is `published`,
  revision 1. The application is public and release `0.1.3` is ready.
- Released `0.1.4` to let fresh CLI homes save the organization selected by IAM for token or
  browser login. All six native tests/builds and archive validation passed in
  [CI run 35131407843](https://github.com/teamofsilicons/space-station/actions/runs/35131407843),
  source revision `b1bb592`. Archive SHA-256:
  `8fee12ad9f47e35868b30b85353ac8b64c5ac82642a5fc64a98747387aa6f4b8` (12,052,174 bytes).
- Installed `0.1.4` from public Honeycomb into a clean verification home without signing into
  Honeycomb. That installed CLI passed real IAM `login <slt>` without an org flag or environment
  setting, saved `tos`, and passed identity, authenticated status, logout, and unauthenticated
  status checks. Browser login without an org also passed. This CLI fix needed no backend change.

Check publication status with:

```sh
honeycomb publication get 'spacestation' --json
```
