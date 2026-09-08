#!/usr/bin/env python3
"""Native CloudWatch metrics/logs and daily logical backups. Run on either role as root."""
import json,pathlib,subprocess,sys
if len(sys.argv) != 2 or sys.argv[1] not in {'api', 'clickhouse'}:
 raise SystemExit('usage: operations.py api|clickhouse')
role=sys.argv[1]
def run(*args):subprocess.run(args,check=True)
def write(path,text,mode=0o644):
 p=pathlib.Path(path);p.parent.mkdir(parents=True,exist_ok=True);p.write_text(text);p.chmod(mode)
if not pathlib.Path('/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl').exists():
 run('curl','-fsSL','https://amazoncloudwatch-agent.s3.amazonaws.com/ubuntu/arm64/latest/amazon-cloudwatch-agent.deb','-o','/tmp/cloudwatch.deb')
 run('dpkg','-i','/tmp/cloudwatch.deb')
log='/var/log/space-station/api.log' if role=='api' else '/var/log/clickhouse-server/clickhouse-server.err.log'
if role=='api':
 pathlib.Path('/var/log/space-station').mkdir(exist_ok=True)
 write('/etc/systemd/system/space-station.service.d/logging.conf','[Service]\nStandardOutput=append:/var/log/space-station/api.log\nStandardError=append:/var/log/space-station/api.log\n')
 write('/etc/logrotate.d/space-station','/var/log/space-station/api.log {\n daily\n maxsize 50M\n rotate 7\n compress\n delaycompress\n missingok\n notifempty\n copytruncate\n}\n')
 run('systemctl','daemon-reload')
 run('systemctl','restart','space-station')
cfg={'agent':{'metrics_collection_interval':60,'run_as_user':'root'},'metrics':{'namespace':'SpaceStation','append_dimensions':{'InstanceId':'${aws:InstanceId}'},'aggregation_dimensions':[['InstanceId']],'metrics_collected':{'mem':{'measurement':['mem_used_percent']},'disk':{'measurement':['used_percent'],'resources':['/']},'swap':{'measurement':['swap_used_percent']}}},'logs':{'logs_collected':{'files':{'collect_list':[{'file_path':log,'log_group_name':'/space-station/production/'+role,'log_stream_name':'{instance_id}'}]}}}}
write('/opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json',json.dumps(cfg))
run('/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl','-a','fetch-config','-m','ec2','-s','-c','file:/opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json')
backup='''#!/bin/bash
set -euo pipefail
umask 077
stamp=$(date -u +%Y%m%dT%H%M%SZ)
metric() { aws cloudwatch put-metric-data --region us-east-1 --namespace SpaceStation --metric-name BackupSuccess --dimensions Role=ROLE --value "$1"; }
trap 'metric 0 || true' ERR
'''.replace('ROLE',role)
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
backup+='metric 1\n'
write('/opt/space-station/bin/backup',backup,0o700)
write('/etc/systemd/system/space-station-backup.service','[Unit]\nDescription=Space Station logical database backup to encrypted S3\n[Service]\nType=oneshot\nExecStart=/opt/space-station/bin/backup\nTimeoutStartSec=3600\n')
write('/etc/systemd/system/space-station-backup.timer','[Unit]\nDescription=Daily Space Station logical backup\n[Timer]\nOnCalendar=*-*-* 03:15:00 UTC\nPersistent=true\n[Install]\nWantedBy=timers.target\n')
run('systemctl','daemon-reload');run('systemctl','enable','--now','space-station-backup.timer')
