#!/usr/bin/env python3
"""Ship the native backend: build one Linux ARM64 executable here, upload it with the host scripts
as one checksummed release, and install it on the API host through SSM. No server compiles
anything; local AWS credentials are never copied."""
import argparse,hashlib,json,os,pathlib,subprocess,secrets,tarfile,time
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("--initialize", action="store_true", help="Initialize a new production installation (implies --setup); refuses existing secrets")
parser.add_argument("--setup", action="store_true", help="Also converge both hosts' native services: packages, configuration, systemd, backups, CloudWatch")
parser.add_argument("--binary", type=pathlib.Path, help="Ship this aarch64 Linux executable instead of building one")
parser.add_argument("--rollback", action="store_true", help="Put the API host's previous executable back and restart it")
args=parser.parse_args()
ROOT=pathlib.Path(__file__).resolve().parents[2]
# glibc 2.31 is Ubuntu 20.04; any newer Ubuntu runs the same executable.
TARGET,GLIBC='aarch64-unknown-linux-gnu','2.31'
def aws(*args):
 p=subprocess.run(['aws',*args,'--region','us-east-1','--output','json'],text=True,capture_output=True)
 if p.returncode: raise RuntimeError(p.stderr)
 return json.loads(p.stdout) if p.stdout.strip() else {}
stack=aws('cloudformation','describe-stacks','--stack-name','space-station-production')['Stacks'][0]
if stack['StackStatus'] not in {'CREATE_COMPLETE', 'UPDATE_COMPLETE'}:
 raise RuntimeError(f"Stack is not ready: {stack['StackStatus']}")
out={x['OutputKey']:x['OutputValue'] for x in stack['Outputs']}
(ROOT/'.local').mkdir(exist_ok=True)
(ROOT/'.local/production-outputs.json').write_text(json.dumps(out,indent=2))
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

commands={}
def run_on(role,body,comment):
 """Runs a script as root on one host and waits for it; its output is the deploy's output."""
 instance=out['ApiInstanceId' if role=='api' else 'ClickhouseInstanceId']
 cid=aws('ssm','send-command','--instance-ids',instance,'--document-name','AWS-RunShellScript','--parameters',json.dumps({'commands':[body],'executionTimeout':['3600']}),'--timeout-seconds','600','--comment',comment)['Command']['CommandId']
 commands[role]={'instance':instance,'command':cid}
 (ROOT/'.local/production-commands.json').write_text(json.dumps(commands,indent=2))
 print(f'{role}: {comment} ({cid})',flush=True)
 while True:
  time.sleep(3)
  try:r=aws('ssm','get-command-invocation','--command-id',cid,'--instance-id',instance)
  except RuntimeError:continue
  if r['Status'] not in {'Pending','InProgress','Delayed'}:break
 print(r['StandardOutputContent'][-4000:]+r['StandardErrorContent'][-4000:],flush=True)
 if r['Status']!='Success':raise SystemExit(f'{role}: {r["Status"]}')

if args.rollback:
 run_on('api','bash /opt/space-station/release/infra/production/install-backend.sh --rollback','Roll back Space Station backend')
 raise SystemExit(0)

binary=args.binary
if not binary:
 subprocess.run(['cargo','zigbuild','-p','space-station-backend','--release','--locked','--target',f'{TARGET}.{GLIBC}'],cwd=ROOT,check=True)
 binary=pathlib.Path(os.environ.get('CARGO_TARGET_DIR',ROOT/'target'))/TARGET/'release'/'space-station-backend'
head=binary.read_bytes()[:20]
# ELF, 64-bit, e_machine 183 = AArch64: refuse to ship this Mac's own build by mistake.
if head[:5]!=b'\x7fELF\x02' or int.from_bytes(head[18:20],'little')!=183:
 raise SystemExit(f'{binary} is not an aarch64 Linux executable')

def keep(info):
 if '__pycache__' in info.name or info.name.endswith('.pyc'):return None
 # Owned by root on the host, whoever built the release.
 info.uid=info.gid=0;info.uname=info.gname='root'
 return info
# The release says which commit it is; uncommitted changes make it "-dirty".
git=lambda *a:subprocess.run(['git',*a],cwd=ROOT,capture_output=True,text=True,check=True).stdout.strip()
revision=ROOT/'.local/production-REVISION'
revision.write_text(git('rev-parse','HEAD')+('-dirty' if git('status','--porcelain','--untracked-files=no') else '')+'\n')
archive=ROOT/'.local/production-release.tar.gz'
with tarfile.open(archive,'w:gz') as tar:
 tar.add(binary,arcname='space-station-backend',filter=keep)
 tar.add(revision,arcname='REVISION',filter=keep)
 for name in ['infra/production','scripts/migrate-public-identifiers.py','docs/PUBLIC-ID-MIGRATION.md','LICENSE']:
  tar.add(ROOT/name,arcname=name,filter=keep)
digest=hashlib.sha256(archive.read_bytes()).hexdigest()
key=f'releases/{digest}.tar.gz'
subprocess.run(['aws','s3','cp',str(archive),f"s3://{out['ArtifactBucket']}/{key}",'--region','us-east-1','--only-show-errors'],check=True)
print(f'release {digest} of {revision.read_text().strip()} ({binary.stat().st_size:,} byte backend)',flush=True)
# Every host unpacks the same release to /opt/space-station/release, checked against its digest.
fetch=f'''set -eu
cloud-init status --wait
aws s3 cp s3://{out['ArtifactBucket']}/{key} /tmp/space-station-release.tar.gz --region us-east-1 --only-show-errors
echo '{digest}  /tmp/space-station-release.tar.gz' | sha256sum -c -
staging=/opt/space-station/release.new.$$
rm -rf "$staging" /opt/space-station/release.old
mkdir -p "$staging"
tar xzf /tmp/space-station-release.tar.gz -C "$staging"
rm /tmp/space-station-release.tar.gz
if [ -d /opt/space-station/release ]; then mv /opt/space-station/release /opt/space-station/release.old; fi
mv "$staging" /opt/space-station/release
rm -rf /opt/space-station/release.old
r=/opt/space-station/release/infra/production
'''
setup=args.setup or args.initialize
# ClickHouse first: the API checks its connections as it starts.
if setup:run_on('clickhouse',fetch+'bash "$r/setup-native.sh" clickhouse\n','Converge Space Station ClickHouse host')
run_on('api',fetch+('bash "$r/setup-native.sh" api\n' if setup else '')+'bash "$r/install-backend.sh"\n','Install Space Station backend '+digest[:12])
