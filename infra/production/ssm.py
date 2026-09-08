#!/usr/bin/env python3
"""Run a command file on one Space Station host, or inspect a command. Never put secrets in command files."""
import sys,json,subprocess,time,pathlib
out=json.loads(pathlib.Path('.local/production-outputs.json').read_text())
def aws(*args):return json.loads(subprocess.check_output(['aws',*args,'--region','us-east-1','--output','json'],text=True))
role=sys.argv[1];instance=out['ApiInstanceId' if role=='api' else 'ClickhouseInstanceId']
command=pathlib.Path(sys.argv[2]).read_text()
r=aws('ssm','send-command','--instance-ids',instance,'--document-name','AWS-RunShellScript','--parameters',json.dumps({'commands':[command],'executionTimeout':['7200']}))
id=r['Command']['CommandId'];print('Command',id,flush=True)
while True:
 time.sleep(2)
 try:r=aws('ssm','get-command-invocation','--command-id',id,'--instance-id',instance)
 except subprocess.CalledProcessError:continue
 if r['Status'] not in ['Pending','InProgress','Delayed']:break
print(r['Status']);print(r['StandardOutputContent']);print(r['StandardErrorContent']);sys.exit(0 if r['Status']=='Success' else 1)
