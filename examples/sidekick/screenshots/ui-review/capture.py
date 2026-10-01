#!/usr/bin/env python3
"""Capture genuine app renderer pixels, without an interactive simulator.

Only test code is added to a temporary copy of the app source. It invokes the
original app screen methods and the shared Kobo renderer/fonts. No app socket,
network service, physical display, or simulated refresh behavior is exercised.
"""
import argparse, hashlib, json, os, pathlib, subprocess, tempfile, tomllib
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source-root',type=pathlib.Path)
p.add_argument('--out',type=pathlib.Path,required=True)
a=p.parse_args(); here=pathlib.Path(__file__).resolve().parent
config=json.loads((here/'capture.json').read_text())
root=(a.source_root or here.parents[3]).resolve()
app=root/config['app']; out=a.out.resolve();out.mkdir(parents=True,exist_ok=True)
source=(app/'src/main.rs').read_text();cargo=tomllib.loads((app/'Cargo.toml').read_text())
scene=(here/'capture-scene.rs').read_text()
scene=scene.replace('APP_SCREEN', 'app.screen(&context)' if 'fn screen(&self, context: &Context)' in source else 'app.screen()')
scene=scene.replace('APP_QUESTION_PAGES', 'app.question_pages(&context, app.ask.as_ref().unwrap()).len()' if 'fn question_pages(' in source else '1')
scene=scene.replace('APP_SET_PAGE', 'app.page = page;' if 'fn question_pages(' in source else '')
probe=app/'src/ui_review_capture_probe.rs'
if probe.exists():raise SystemExit('Refusing to replace an existing review probe')
with tempfile.TemporaryDirectory(prefix='cobalt-render-') as tmp:
 tmp=pathlib.Path(tmp);support=tmp/'capture-support.rs';support.write_text((here/'capture-support.rs').read_text())
 code=source+'\n#[cfg(test)]\nmod ui_review_capture {\nuse super::*;\ninclude!('+json.dumps(str(support))+');\n'+scene+'\n}\n'
 dependencies={}
 for group in ('dependencies','dev-dependencies'):
  for name,value in cargo.get(group,{}).items():
   if isinstance(value,dict) and 'path' in value:value={**value,'path':str((app/value['path']).resolve())}
   dependencies[name]=value
 for name in ('kobo-ui','kobo-text','kobo-image'):dependencies[name]={'path':str(root/'crates'/name)}
 version=cargo['package']['version']
 if not isinstance(version,str):version=tomllib.loads((root/'Cargo.toml').read_text())['workspace']['package']['version']
 manifest='[package]\nname="cobalt-render-review"\nversion='+json.dumps(version)+'\nedition="2021"\n[workspace]\n[[bin]]\nname="cobalt-render-review"\npath='+json.dumps(str(probe))+'\n[dependencies]\n'
 for name,value in dependencies.items():
  if isinstance(value,str):rendered=json.dumps(value)
  else:rendered='{'+','.join(key+'='+json.dumps(val) for key,val in value.items())+'}'
  manifest+=name+'='+rendered+'\n'
 (tmp/'Cargo.toml').write_text(manifest)
 (tmp/'Cargo.lock').write_text((root/'Cargo.lock').read_text())
 probe.write_text(code)
 try:
  env=dict(os.environ,COBALT_UI_CAPTURE_DIR=str(out))
  subprocess.run(['cargo','test','--manifest-path',str(tmp/'Cargo.toml'),'ui_review_capture','--','--nocapture'],env=env,check=True)
 finally:probe.unlink()
 metadata={'capture_kind':'native app renderer snapshot, not interactive simulator','source_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),'source_file':str(pathlib.Path(config['app'])/'src/main.rs'),'source_file_modified':bool(subprocess.check_output(['git','diff','HEAD','--',str(app/'src/main.rs')],cwd=root)), 'source_sha256':hashlib.sha256(source.encode()).hexdigest(),'profile':'Clara BW 1072x1448','status':'synthetic 00:00, 50% battery, connected radios','hardware_tested':False,'simulator_tested':False}
 (out/'provenance.json').write_text(json.dumps(metadata,indent=2)+'\n')
