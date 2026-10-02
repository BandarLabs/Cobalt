import subprocess,json
cli='/workspace/cobalt-cli-remediation-target/debug/kobo'
cases=[]
bases={
 'needles':['needles','push','/nonexistent','--title','fixture'],
 'vault':['vault','push','/nonexistent','--exclude','fixture'],
 'post':['post','login','--gateway','https://letters.example','--token-file','/nonexistent'],
 'readlater':['readlater','login','--server','https://bag.example','--client-id','id','--username','user','--client-secret-file','/nonexistent'],
}
for app,base in bases.items():
 for flag in ['--device','-s']:
  for host in ['--sim','--device','-s','-reader','']:
   for first in [False,True]:
    args=base.copy();index=(3 if app in ['needles','vault'] else 2) if first else len(args)
    args[index:index]=[flag,host]
    p=subprocess.run([cli,*args],capture_output=True,text=True,timeout=5)
    assert p.returncode!=0 and 'device host' in p.stderr,(args,p.returncode,p.stdout,p.stderr)
    assert 'read /nonexistent' not in p.stderr
    cases.append({'args':args,'status':p.returncode,'stderr':p.stderr.strip()})
print(json.dumps({'passed':len(cases),'cases':cases},indent=2))
