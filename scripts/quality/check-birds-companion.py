#!/usr/bin/env python3
"""Exercise the Birds companion's bounded atomic simulator import."""
import argparse,json,os,struct,subprocess,tempfile
from pathlib import Path

def png():
 b=b'\x89PNG\r\n\x1a\n'
 b+=struct.pack('>I',13)+b'IHDR'+b'\0'*13+b'\0'*4
 b+=struct.pack('>I',0)+b'IEND'+b'\0'*4
 return b

def main():
 p=argparse.ArgumentParser();p.add_argument('--cli',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args(); transcript=[]
 with tempfile.TemporaryDirectory(prefix='cobalt-birds-check-',dir='/tmp') as temp:
  root=Path(temp);env=dict(os.environ,TMPDIR=temp); snapshot=root/'current.json'; image=root/'collage.png'; image.write_bytes(png())
  shelf=root/'cobalt-sim-data'/'birds'
  def pair():
   meta=json.loads((shelf/'current.json').read_bytes()); return (shelf/'current.json').read_bytes(),(shelf/meta['image']).read_bytes()
  def run(*args,ok=True):
   r=subprocess.run([str(a.cli.resolve()),'birds',*map(str,args)],env=env,capture_output=True,text=True,timeout=10);transcript.append({'args':list(map(str,args)),'code':r.returncode,'output':r.stdout+r.stderr});assert (r.returncode==0)==ok,transcript[-1]
  snapshot.write_text(json.dumps({'format':'cobalt-birds-v1','generated_at':1,'source':'fixture','recent':['Robin']})+'\n')
  run('push',snapshot,image,'--sim'); before=pair()
  meta=json.loads(before[0]); assert meta['image'].startswith('img-') and meta['image'].endswith('.png') and meta['image_checksum'],meta
  snapshot.write_text('{}');run('push',snapshot,image,'--sim',ok=False);assert pair()==before
  snapshot.write_text(json.dumps({'format':'cobalt-birds-v1','generated_at':2})+'\n')
  pending=shelf/'.current.json.writing';pending.write_bytes(b'other')
  newer=png()+b'' ; run('push',snapshot,image,'--sim',ok=False)
  assert pending.read_bytes()==b'other' and pair()==before, 'a failed pointer commit must leave the whole previous pair intact'
 a.output.mkdir(parents=True,exist_ok=True);(a.output/'transcript.json').write_text(json.dumps(transcript,indent=2)+'\n');(a.output/'result.json').write_text(json.dumps({'status':'passed','physical_hardware':False,'checks':['valid v1 snapshot published with content-addressed image','invalid JSON preserves prior snapshot','occupied pointer staging preserves the complete prior pair']},indent=2)+'\n')
if __name__=='__main__':main()
