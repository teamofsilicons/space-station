#!/usr/bin/env python3
"""Local diagnostic logs and daily logical backups. Run on either role as root."""
import pathlib,subprocess,sys
if len(sys.argv) != 2 or sys.argv[1] not in {'api', 'clickhouse'}:
 raise SystemExit('usage: operations.py api|clickhouse')
role=sys.argv[1]
def run(*args):subprocess.run(args,check=True)
def write(path,text,mode=0o644):
 p=pathlib.Path(path);p.parent.mkdir(parents=True,exist_ok=True);p.write_text(text);p.chmod(mode)
# Stop the existing forwarder before removing it; leaving it installed would keep sending data.
if pathlib.Path('/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl').exists():
 run('systemctl','disable','--now','amazon-cloudwatch-agent')
 run('dpkg','--purge','amazon-cloudwatch-agent')
if role=='api':
 pathlib.Path('/var/log/space-station').mkdir(exist_ok=True)
 write('/etc/systemd/system/space-station.service.d/logging.conf','[Service]\nStandardOutput=append:/var/log/space-station/api.log\nStandardError=append:/var/log/space-station/api.log\n')
 write('/etc/logrotate.d/space-station','/var/log/space-station/api.log {\n daily\n maxsize 50M\n rotate 7\n compress\n delaycompress\n missingok\n notifempty\n copytruncate\n}\n')
 run('systemctl','daemon-reload')
 # try-restart: on a fresh host the executable is not installed yet; install-backend.sh starts it.
 run('systemctl','try-restart','space-station')
backup='''#!/bin/bash
set -euo pipefail
umask 077
stamp=$(date -u +%Y%m%dT%H%M%SZ)
'''
if role=='api':
 backup+='''file=/var/lib/space-station/backup-${stamp}.dump
runuser -u postgres -- pg_dump -Fc space_station > "$file"
aws s3 cp "$file" s3://space-station-production-234951665042-us-east-1/backups/postgres/${stamp}.dump --only-show-errors
rm "$file"
'''
else:
 backup+='''file=/var/lib/clickhouse/backups/space-station-${stamp}.zip
clickhouse-client --query "BACKUP DATABASE space_station TO File('${file}')"
aws s3 cp "$file" s3://space-station-production-234951665042-us-east-1/backups/clickhouse/${stamp}.zip --only-show-errors
rm "$file"
'''
write('/opt/space-station/bin/backup',backup,0o700)
write('/etc/systemd/system/space-station-backup.service','[Unit]\nDescription=Space Station logical database backup to encrypted S3\n[Service]\nType=oneshot\nExecStart=/opt/space-station/bin/backup\nTimeoutStartSec=3600\n')
write('/etc/systemd/system/space-station-backup.timer','[Unit]\nDescription=Daily Space Station logical backup\n[Timer]\nOnCalendar=*-*-* 03:15:00 UTC\nPersistent=true\n[Install]\nWantedBy=timers.target\n')
run('systemctl','daemon-reload');run('systemctl','enable','--now','space-station-backup.timer')
