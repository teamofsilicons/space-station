#!/bin/bash
set -euo pipefail
# Makes the backend executable shipped in this release the running API: the one it replaces is kept
# as .previous, the service restarts, and the API must answer /api/health within a minute or the
# previous executable goes back in. `install-backend.sh --rollback` puts .previous back by hand.
# Run as root on the API host from an unpacked release. Nothing is compiled here.
bin=/opt/space-station/bin
release=$(cd "$(dirname "$0")/../.." && pwd)
# The git revision of what is running, as the manual cutover recorded it; it follows the executable.
revision=/opt/space-station/deployed-revision

healthy() {
  for _ in $(seq 1 60); do
    if curl -fs --max-time 2 http://127.0.0.1:8080/api/health >/dev/null; then return 0; fi
    sleep 1
  done
  return 1
}
# Replace atomically: a half-copied executable is never the one systemd starts.
put() {
  install -m 0755 "$1" "$bin/space-station-backend.new"
  mv "$bin/space-station-backend.new" "$bin/space-station-backend"
}
restore() {
  [ -x "$bin/space-station-backend.previous" ] || return 1
  put "$bin/space-station-backend.previous"
  if [ -f "$revision.previous" ]; then cp -p "$revision.previous" "$revision"; fi
  systemctl restart space-station
  healthy
}

if [ "${1:-}" = --rollback ]; then
  if restore; then printf 'Rolled back to the previous executable\n'; sha256sum "$bin/space-station-backend"; exit 0; fi
  printf 'No healthy previous executable to roll back to\n' >&2
  exit 1
fi

mkdir -p "$bin"
if [ -x "$bin/space-station-backend" ]; then cp -p "$bin/space-station-backend" "$bin/space-station-backend.previous"; fi
if [ -f "$revision" ]; then cp -p "$revision" "$revision.previous"; fi
put "$release/space-station-backend"
systemctl daemon-reload
systemctl enable space-station
systemctl restart space-station
if healthy; then
  cp "$release/REVISION" "$revision"
  printf 'Running %s\n' "$(cat "$revision")"
  sha256sum "$bin/space-station-backend"
  exit 0
fi
tail -n 80 /var/log/space-station/api.log 2>/dev/null || journalctl --no-pager -u space-station -n 80 || true
if restore; then
  printf 'The new executable was not healthy; the previous one is running again\n' >&2
else
  printf 'The new executable was not healthy and there is no healthy previous one\n' >&2
fi
exit 1
