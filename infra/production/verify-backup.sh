#!/bin/bash
set -euo pipefail
umask 077
role=${1:?api or clickhouse}
case "$role" in
  api|clickhouse) ;;
  *) printf 'role must be api or clickhouse\n' >&2; exit 2 ;;
esac
bucket=space-station-production-234951665042-us-east-1
probe=ss_restore_$(date +%s)
systemctl start space-station-backup.service
if [ "$role" = api ]; then
  key=$(aws s3api list-objects-v2 --bucket "$bucket" --prefix backups/postgres/ --query 'sort_by(Contents,&LastModified)[-1].Key' --output text)
  file=/tmp/${probe}.dump
  aws s3 cp "s3://${bucket}/${key}" "$file" --only-show-errors
  # Dumps can contain credentials and customer data; retain private mode while
  # the temporary restore probe is running.
  chmod 600 "$file"
  runuser -u postgres -- createdb "$probe"
  trap 'runuser -u postgres -- dropdb "$probe"; rm -f "$file"' EXIT
  runuser -u postgres -- pg_restore --exit-on-error -d "$probe" "$file"
  runuser -u postgres -- psql -d "$probe" -tAc 'SELECT count(*) AS restored_tables FROM tables'
else
  key=$(aws s3api list-objects-v2 --bucket "$bucket" --prefix backups/clickhouse/ --query 'sort_by(Contents,&LastModified)[-1].Key' --output text)
  file=/var/lib/clickhouse/backups/${probe}.zip
  aws s3 cp "s3://${bucket}/${key}" "$file" --only-show-errors
  chmod 600 "$file"
  chown clickhouse:clickhouse "$file"
  trap 'clickhouse-client --query "DROP DATABASE IF EXISTS ${probe}"; rm -f "$file"' EXIT
  clickhouse-client --query "RESTORE DATABASE space_station AS ${probe} FROM File('${file}')"
  clickhouse-client --query "SELECT count() AS restored_records FROM ${probe}.records"
fi
printf 'Restored successfully from s3://%s/%s\n' "$bucket" "$key"
