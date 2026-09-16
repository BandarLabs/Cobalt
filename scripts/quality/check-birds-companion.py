#!/usr/bin/env python3
"""Exercise the Birds companion's bounded atomic simulator import."""
import argparse,json,os,subprocess,tempfile
from pathlib import Path

def main():
 p=argparse.ArgumentParser();p.add_argument('--cli',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args(); transcript=[]
 with tempfile.TemporaryDirectory(prefix='cobalt-birds-check-',dir='/tmp') as temp:
  root=Path(temp);env=dict(os.environ,TMPDIR=temp); snapshot=root/'current.json'; image=root/'current.png'
  snapshot.write_text(json.dumps({'format':'cobalt-birds-v1','generated_at':1,'source':'fixture','recent':['Robin']})+'\n');image.write_bytes(b'\x89PNG\r\n\x1a\nfixture')
  def run(*args,ok=True):
   r=subprocess.run([str(a.cli.resolve()),'birds',*map(str,args)],env=env,capture_output=True,text=True,timeout=10);transcript.append({'args':list(map(str,args)),'code':r.returncode,'output':r.stdout+r.stderr});assert (r.returncode==0)==ok,transcript[-1]
  run('push',snapshot,image,'--sim'); shelf=root/'cobalt-sim-data'/'birds'; before=(shelf/'current.json').read_bytes(),(shelf/'current.png').read_bytes()
  snapshot.write_text('{}');run('push',snapshot,image,'--sim',ok=False);assert ((shelf/'current.json').read_bytes(),(shelf/'current.png').read_bytes())==before
  snapshot.write_text(json.dumps({'format':'cobalt-birds-v1','generated_at':2})+'\n');pending=shelf/'current.writing';pending.write_bytes(b'other');run('push',snapshot,image,'--sim',ok=False);assert pending.read_bytes()==b'other' and (shelf/'current.json').read_bytes()==before[0]
 a.output.mkdir(parents=True,exist_ok=True);(a.output/'transcript.json').write_text(json.dumps(transcript,indent=2)+'\n');(a.output/'result.json').write_text(json.dumps({'status':'passed','physical_hardware':False,'checks':['valid v1 snapshot published','invalid JSON preserves prior snapshot','occupied temporary path preserves prior unit']},indent=2)+'\n')
if __name__=='__main__':main()
