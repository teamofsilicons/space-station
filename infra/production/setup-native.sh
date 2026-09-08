#!/bin/bash
set -euo pipefail
# Run as root through SSM after uploading the release archive. No secret values in commands.
ROLE=${1:?api or clickhouse}
case "$ROLE" in
  api|clickhouse) ;;
  *) printf 'ROLE must be api or clickhouse\n' >&2; exit 2 ;;
esac
export DEBIAN_FRONTEND=noninteractive
cloud-init status --wait
apt-get update
apt-get install -y python3-boto3 curl ca-certificates gnupg
if [ "$ROLE" = api ]; then
  apt-get install -y postgresql redis-server build-essential pkg-config libssl-dev
  id spacestation >/dev/null 2>&1 || useradd --system --home /var/lib/space-station --create-home --shell /usr/sbin/nologin spacestation
  systemctl enable --now postgresql redis-server
  python3 /opt/space-station/source/infra/production/configure-native.py api
  if [ ! -x /root/.cargo/bin/cargo ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh
    sh /tmp/rustup.sh -y --profile minimal --default-toolchain 1.98.0
  fi
  cd /opt/space-station/source
  /root/.cargo/bin/cargo build --locked --release -p space-station-backend -p space-station-cli -j 2
  install -m 0755 target/release/space-station-backend /opt/space-station/bin/space-station-backend.new
  mv /opt/space-station/bin/space-station-backend.new /opt/space-station/bin/space-station-backend
  systemctl daemon-reload
  systemctl enable space-station space-station-backup.timer
  systemctl restart space-station
  systemctl start space-station-backup.timer
else
  curl -fsSL https://packages.clickhouse.com/rpm/lts/repodata/repomd.xml.key | gpg --dearmor --yes -o /usr/share/keyrings/clickhouse-keyring.gpg
  echo 'deb [signed-by=/usr/share/keyrings/clickhouse-keyring.gpg arch=arm64] https://packages.clickhouse.com/deb lts main' > /etc/apt/sources.list.d/clickhouse.list
  apt-get update
  apt-get install -y clickhouse-server clickhouse-client
  python3 /opt/space-station/source/infra/production/configure-native.py clickhouse
fi
python3 /opt/space-station/source/infra/production/operations.py "$ROLE"
