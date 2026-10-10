# Silicon Apps releases

App ID: `spacestation`. Publishing account: `si:tos`. The nonsecret catalog settings are in `application.json`. Configure the app and sign-in in [Silicon Developer](https://developers.teamofsilicons.com), or use the `silicon-apps` and `silicon-accounts app` CLIs.

```sh
silicon-apps login status --json
silicon-apps setup spacestation show
scripts/build-cli-release.sh
silicon-apps validate dist/spacestation-apps-0.4.0.tar.gz
silicon-apps upload spacestation --target linux-x86_64 dist/spacestation-apps-0.4.0.tar.gz
silicon-apps upload spacestation --target linux-aarch64 dist/spacestation-apps-0.4.0.tar.gz
silicon-apps release spacestation --version 0.4.0 --package PACKAGE_X64 --package PACKAGE_ARM64
silicon-apps promote spacestation DEVELOPMENT_RELEASE_ID --version 0.4.0
silicon-apps readiness spacestation
silicon-apps publish spacestation
```

Use a stable `--idempotency-key` for recoverable publishing mutations. New bytes require a new version. Packages contain `apps.yaml` and six native executables; each implements `--help`, `accounts --json`, and `login status --json`. Silicon Apps is the sole updater of installed packages.

The current public platform has Linux validation workers only. macOS and Windows binaries are also distributed through the GitHub release and website downloads, and can be added to Apps when its workers become available. Check `silicon-apps capabilities --json` before uploading.

The GitHub `release` workflow builds on native macOS, Linux, and Windows runners, checks discovery, packages archives, and publishes on a `v*` tag. Rust crates and JavaScript packages are published separately. Backend deployment is described in [production operations](../production/README.md).
