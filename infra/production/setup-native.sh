#!/bin/bash
set -euo pipefail
# Converges one host's native services — packages, configuration, systemd units, backups and
# CloudWatch — from an unpacked release. Run as root through SSM. No toolchain is installed and
# nothing is compiled: the backend arrives prebuilt and install-backend.sh puts it in place.
# No secret values in commands.
ROLE=${1:?api or clickhouse}
case "$ROLE" in
  api|clickhouse) ;;
  *) printf 'ROLE must be api or clickhouse\n' >&2; exit 2 ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
export DEBIAN_FRONTEND=noninteractive
cloud-init status --wait
apt-get update
apt-get install -y python3-boto3 curl ca-certificates gnupg
if [ "$ROLE" = api ]; then
  apt-get install -y postgresql redis-server
  id spacestation >/dev/null 2>&1 || useradd --system --home /var/lib/space-station --create-home --shell /usr/sbin/nologin spacestation
  systemctl enable --now postgresql redis-server
  python3 "$here/configure-native.py" api
  systemctl daemon-reload
else
  curl -fsSL https://packages.clickhouse.com/rpm/lts/repodata/repomd.xml.key | gpg --dearmor --yes -o /usr/share/keyrings/clickhouse-keyring.gpg
  echo 'deb [signed-by=/usr/share/keyrings/clickhouse-keyring.gpg arch=arm64] https://packages.clickhouse.com/deb lts main' > /etc/apt/sources.list.d/clickhouse.list
  apt-get update
  apt-get install -y clickhouse-server clickhouse-client
  python3 "$here/configure-native.py" clickhouse
fi
python3 "$here/operations.py" "$ROLE"
