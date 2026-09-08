#!/usr/bin/env bash
# Runs every test group, says what each covers, and ends with a results table.
set -u
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target/main}"
declare -a NAMES RESULTS
run() { # name, description, command...
  local name="$1" desc="$2"; shift 2
  printf '\n## %s — %s\n\n' "$name" "$desc"
  if "$@"; then RESULTS+=("pass"); else RESULTS+=("FAIL"); fi
  NAMES+=("$name")
}
run shared   "limits, sanitizer (truncation, file markers, record size), secret scanner, table keys" cargo test -q -p space-station-shared
if ! scripts/sync-runtime.sh; then
  printf '%s\n' 'runtime sync failed; refusing to run tests against stale code.' >&2
  exit 1
fi
run client   "spool and seq, daemon election, batch building, acks; Auth as a value, loopback login with org, slt exchange, a final 401 against a responder; publish pre-flight; embedded runtime" cargo test -q -p space-station
run backend  "sql guard (one test runs the rendered SQL on ClickHouse), access lists, cron, crypto, config, ingest and flusher units, IAM stub + client + session, webhook receiver and outgoing webhook vetting" cargo test -q -p space-station-backend --lib
run backend-integration "database services + IAM stub: every login door, ingest → flush → query → trigger → notification → webhook, session refresh, IAM webhook → mirror + revoke, Origin rule" cargo test -q -p space-station-backend --test '*'
run cli      "auth.json lifecycle and lock, login through a fake browser binds the org, auth exchanges an slt and never prompts, a 401 prints the way in, org resolution, column rendering, secrets on stdout only, full command tree"  cargo test -q -p space-station-cli
run runtime  "queue semantics, delta/snapshot, reconnect catch-up, tool type table, sandbox constant and child isolation, 64 KB bound, dev server + publish pre-flight"        npm --prefix packages/space-station test
run web      "typecheck + build, org picker and switching, dev panel, one-time secrets, notification definitions, agent prompt"   npm --prefix apps/web run check-all
printf '\n## results\n\n| group | result |\n|---|---|\n'
for i in "${!NAMES[@]}"; do printf '| %s | %s |\n' "${NAMES[$i]}" "${RESULTS[$i]}"; done
for r in "${RESULTS[@]}"; do [ "$r" = pass ] || exit 1; done
