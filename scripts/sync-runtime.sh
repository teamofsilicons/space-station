#!/usr/bin/env bash
# The client crate embeds the runtime with include_str!, and cargo package cannot reach outside it,
# so the published file is a vendored copy. Run before testing or publishing the CLI.
set -eu
cd "$(dirname "$0")/.."
cp packages/space-station/mission-control.js crates/client/runtime/mission-control.js
