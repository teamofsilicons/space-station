#!/usr/bin/env python3
"""Upload source and run native builds through SSM. Local AWS credentials are never copied."""
import argparse,json,pathlib,subprocess,secrets,tarfile,sys
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("--initialize", action="store_true", help="Initialize a new production installation; refuses existing secrets")
parser.add_argument("--api-only", action="store_true", help="Update the API without rebuilding or restarting ClickHouse")
args=parser.parse_args()
ROOT=pathlib.Path(__file__).resolve().parents[2]
def aws(*args):
 p=subprocess.run(['aws',*args,'--region','us-east-1','--output','json'],text=True,capture_output=True)
 if p.returncode: raise RuntimeError(p.stderr)
 return json.loads(p.stdout) if p.stdout.strip() else {}
stack=aws('cloudformation','describe-stacks','--stack-name','space-station-production')['Stacks'][0]
if stack['StackStatus'] not in {'CREATE_COMPLETE', 'UPDATE_COMPLETE'}:
 raise RuntimeError(f"Stack is not ready: {stack['StackStatus']}")
out={x['OutputKey']:x['OutputValue'] for x in stack['Outputs']}
instances=aws('ec2','describe-instances','--instance-ids',out['ApiInstanceId'],out['ClickhouseInstanceId'])
ips={i['InstanceId']:i['PrivateIpAddress'] for r in instances['Reservations'] for i in r['Instances']}
if args.initialize:
 def dotenv(p):return dict((k.strip(),v.strip().strip('\"\'')) for l in p.read_text().splitlines() if l.strip() and not l.lstrip().startswith('#') and '=' in l for k,v in [l.split('=',1)])
 iam=dotenv(ROOT/'.env.iam');pg=secrets.token_hex(32);ch=secrets.token_hex(32)
 api={'SS_ORIGIN':'https://spacestation.teamofsilicons.com','SS_COOKIE_DOMAIN':'spacestation.teamofsilicons.com','SILICON_IAM_AUTH_URL':'https://auth.iam.teamofsilicons.com','SS_KEY':secrets.token_hex(32),'PORT':'8080','RUST_LOG':'space_station_backend=info',
 'DATABASE_URL':f'postgres://spacestation:{pg}@127.0.0.1/space_station','_POSTGRES_PASSWORD':pg,'REDIS_URL':'redis://127.0.0.1:6379',
 'CLICKHOUSE_URL':f'http://spacestation:{ch}@{ips[out["ClickhouseInstanceId"]]}:8123/space_station','CLICKHOUSE_QUERY_PASSWORD':secrets.token_hex(32)}
 for k in ['SILICON_IAM_URL','SILICON_IAM_APP_ID','SILICON_IAM_APP_SECRET','SILICON_IAM_WEBHOOK_SECRET']:api[k]=iam[k]
 api['SILICON_IAM_APP_ID']='spacestation'
 for key,value in [('ApiRuntime',api),('ClickhouseRuntime',{'ADMIN_PASSWORD':ch,'API_IP':ips[out['ApiInstanceId']]})]:
  # Refuse accidental secret rotation of an already configured server.
  try:existing=aws('secretsmanager','get-secret-value','--secret-id',out[key+'Arn'])
  except RuntimeError as e:
   if 'ResourceNotFoundException' not in str(e):raise
   existing={}
  if existing.get('SecretString'):
   raise RuntimeError(f"{key} is already initialized; omit --initialize")
  p=ROOT/'.local'/('production-'+key+'.json')
  try:
   p.touch(mode=0o600, exist_ok=True)
   p.chmod(0o600)
   p.write_text(json.dumps(value))
   aws('secretsmanager','put-secret-value','--secret-id',out[key+'Arn'],'--secret-string','file://'+str(p))
  finally:
   p.unlink(missing_ok=True)
archive=ROOT/'.local/production-source.tar.gz'
archive.parent.mkdir(parents=True, exist_ok=True)

def archive_filter(info):
 # Bytecode and cache directories are local build artifacts, not deployable
 # source. Excluding them keeps uploads smaller and prevents stale code from
 # being copied to a host.
 name = info.name.replace('\\\\', '/')
 if '/__pycache__' in name or name.endswith('.pyc') or '/target/' in name:
  return None
 return info

with tarfile.open(archive,'w:gz') as tar:
 for name in ['Cargo.toml','Cargo.lock','rustfmt.toml','crates','vendor','infra/production',
              'scripts/migrate-public-identifiers.py','docs/PUBLIC-ID-MIGRATION.md','LICENSE']:
  tar.add(ROOT/name,arcname=name,filter=archive_filter)
subprocess.run(['aws','s3','cp',str(archive),'s3://'+out['ArtifactBucket']+'/releases/source.tar.gz','--region','us-east-1','--only-show-errors'],check=True)
commands={}
roles = [('api','ApiInstanceId')] if args.api_only else [('api','ApiInstanceId'),('clickhouse','ClickhouseInstanceId')]
for role,key in roles:
 command=f'''set -eu
cloud-init status --wait
source=/opt/space-station/source
staging=/opt/space-station/source.new.$$
old=/opt/space-station/source.old.$$
rm -rf "$staging" "$old"
mkdir -p "$staging"
aws s3 cp s3://{out['ArtifactBucket']}/releases/source.tar.gz /tmp/space-station.tar.gz --region us-east-1 --only-show-errors
tar xzf /tmp/space-station.tar.gz -C "$staging"
# Replace source atomically while retaining Cargo's target cache; this removes
# files deleted from the repository without making rebuilds start cold.
if [ -d "$source/target" ]; then mv "$source/target" "$staging/target"; fi
if [ -d "$source" ]; then mv "$source" "$old"; fi
mv "$staging" "$source"
rm -rf "$old"
bash "$source/infra/production/setup-native.sh" {role}
'''
 result=aws('ssm','send-command','--instance-ids',out[key],'--document-name','AWS-RunShellScript','--parameters',json.dumps({'commands':[command],'executionTimeout':['7200']}),'--timeout-seconds','600','--comment','Deploy Space Station native '+role)
 commands[role]={'instance':out[key],'command':result['Command']['CommandId']}
(ROOT/'.local/production-commands.json').write_text(json.dumps(commands,indent=2))
(ROOT/'.local/production-outputs.json').write_text(json.dumps(out,indent=2))
print(json.dumps(commands,indent=2))
