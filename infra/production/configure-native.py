#!/usr/bin/env python3
"""Converge native service configuration using only this instance's runtime secret."""
import sys,json,pathlib,subprocess,hashlib,urllib.request,base64
import boto3
if len(sys.argv) != 2 or sys.argv[1] not in {'api', 'clickhouse'}:
 raise SystemExit('usage: configure-native.py api|clickhouse')
role=sys.argv[1]
secret='ApiRuntime' if role=='api' else 'ClickhouseRuntime'
env=json.loads(boto3.client('secretsmanager',region_name='us-east-1').get_secret_value(SecretId='space-station/production/'+secret)['SecretString'])
def write(path,text,mode=0o644):
 p=pathlib.Path(path);p.parent.mkdir(parents=True,exist_ok=True);p.write_text(text);p.chmod(mode)
def run(*args,**kw):return subprocess.run(args,check=True,**kw)
if role=='api':
 write('/etc/space-station/runtime.env',''.join(k+'='+v+'\n' for k,v in env.items() if not k.startswith('_')),0o600)
 password=env['_POSTGRES_PASSWORD']
 # The generated passwords are hex; never interpolate arbitrary input into SQL.
 if not password or any(c not in '0123456789abcdef' for c in password):
  raise RuntimeError('POSTGRES password must be lowercase hexadecimal')
 sql="DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='spacestation') THEN CREATE ROLE spacestation LOGIN; END IF; END $$; ALTER ROLE spacestation PASSWORD '"+password+"';"
 run('runuser','-u','postgres','--','psql','-v','ON_ERROR_STOP=1',input=sql,text=True,stdout=subprocess.DEVNULL)
 exists=subprocess.check_output(['runuser','-u','postgres','--','psql','-tAc',"SELECT 1 FROM pg_database WHERE datname='space_station'"],text=True).strip()
 if not exists: run('runuser','-u','postgres','--','createdb','-O','spacestation','space_station')
 write('/etc/redis/redis.conf','''bind 127.0.0.1 ::1
protected-mode yes
port 6379
daemonize no
supervised systemd
dir /var/lib/redis
logfile /var/log/redis/redis-server.log
appendonly yes
appendfsync everysec
maxmemory-policy noeviction
save 900 1
save 300 10
save 60 10000
''')
 run('systemctl','restart','redis-server')
 write('/etc/systemd/system/space-station.service','''[Unit]
Description=Space Station native Rust API
After=network-online.target postgresql.service redis-server.service
Wants=network-online.target
Requires=postgresql.service redis-server.service
[Service]
User=spacestation
Group=spacestation
WorkingDirectory=/var/lib/space-station
EnvironmentFile=/etc/space-station/runtime.env
ExecStart=/opt/space-station/bin/space-station-backend
Restart=always
RestartSec=5
TimeoutStopSec=60
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/space-station
LimitNOFILE=65536
[Install]
WantedBy=multi-user.target
''')
 write('/opt/space-station/bin/backup','''#!/bin/bash
set -euo pipefail
umask 077
stamp=$(date -u +%Y%m%dT%H%M%SZ)
file=/var/lib/space-station/backup-${stamp}.dump
runuser -u postgres -- pg_dump -Fc space_station > "$file"
aws s3 cp "$file" s3://space-station-production-234951665042-us-east-1/backups/postgres/${stamp}.dump --only-show-errors
rm "$file"
''',0o700)
 write('/etc/systemd/system/space-station-backup.service','''[Unit]
Description=Space Station PostgreSQL backup to encrypted S3
[Service]
Type=oneshot
ExecStart=/opt/space-station/bin/backup
''')
 write('/etc/systemd/system/space-station-backup.timer','''[Unit]
Description=Daily Space Station PostgreSQL backup
[Timer]
OnCalendar=*-*-* 03:15:00 UTC
Persistent=true
[Install]
WantedBy=timers.target
''')
else:
 digest=hashlib.sha256(env['ADMIN_PASSWORD'].encode()).hexdigest()
 write('/etc/clickhouse-server/users.d/space-station.xml',f'''<clickhouse><users><default><networks replace="replace"><ip>127.0.0.1</ip><ip>::1</ip></networks></default><spacestation><password_sha256_hex>{digest}</password_sha256_hex><networks><ip>{env['API_IP']}/32</ip><ip>127.0.0.1</ip></networks><profile>default</profile><quota>default</quota><access_management>1</access_management></spacestation></users></clickhouse>''',0o640)
 run('chown','root:clickhouse','/etc/clickhouse-server/users.d/space-station.xml')
 write('/etc/clickhouse-server/config.d/space-station.xml','''<clickhouse><listen_host>0.0.0.0</listen_host><max_server_memory_usage_to_ram_ratio>0.75</max_server_memory_usage_to_ram_ratio><logger><level>warning</level><size>100M</size><count>3</count></logger><custom_settings_prefixes>SQL_</custom_settings_prefixes><backups><allowed_path>/var/lib/clickhouse/backups</allowed_path></backups></clickhouse>''')
 run('systemctl','enable','clickhouse-server')
 run('systemctl','restart','clickhouse-server')
 import time
 for i in range(30):
  try:
   req=urllib.request.Request('http://127.0.0.1:8123/',data=b'CREATE DATABASE IF NOT EXISTS space_station')
   req.add_header('Authorization','Basic '+base64.b64encode(('spacestation:'+env['ADMIN_PASSWORD']).encode()).decode())
   urllib.request.urlopen(req,timeout=3).read();break
  except Exception:
   if i==29:raise
   time.sleep(1)
 print('ClickHouse configured')
