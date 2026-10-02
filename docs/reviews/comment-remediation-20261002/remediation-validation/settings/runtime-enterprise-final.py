#!/usr/bin/env python3
"""Exercise host Back routing separately from native renderer/callback tests."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import urllib.request


def action_id(text):
    value = 2166136261
    for byte in text.encode():
        value = ((value ^ byte) * 16777619) & 0xffffffff
    return value


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--root', type=Path, required=True)
    ap.add_argument('--cli', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--scale', default='170')
    args = ap.parse_args()
    root, cli, output = args.root.resolve(), args.cli.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    checks = []
    with tempfile.TemporaryDirectory(prefix='settings-runtime-',dir='/tmp') as tmp:
        env = dict(os.environ,TMPDIR=tmp,KOBO_TEXT_SCALE=args.scale,KOBO_SIM_PROFILE='clara-bw-391',KOBO_SIM_OFFLINE='1')
        process = None
        with (output/'runtime.log').open('w') as log:
            try:
                process = subprocess.Popen([str(cli),'dev','--runtime','127.0.0.1:0','--apps','settings'],cwd=root,env=env,stdout=log,stderr=log,start_new_session=True)
                deadline = time.monotonic()+180
                while time.monotonic()<deadline:
                    assert process.poll() is None, (output/'runtime.log').read_text()[-2000:]
                    found = re.search(r'Kobo runtime simulator: http://(127\.0\.0\.1:\d+)',(output/'runtime.log').read_text())
                    if found: address=found.group(1);break
                    time.sleep(.1)
                else: raise TimeoutError('runtime startup')
                def get(name):
                    with urllib.request.urlopen(f'http://{address}/{name}',timeout=5) as response:return json.load(response)
                def drive(*steps):
                    command=[str(cli),'drive','--address',address,'--ideal','--shots',str(output)]
                    for step in steps:command += ['--step',step]
                    subprocess.run(command,cwd=root,env=env,stdout=log,stderr=log,check=True,timeout=45)
                def wait_app(app):
                    deadline=time.monotonic()+12
                    while time.monotonic()<deadline:
                        if get('simulation')['navigation']['app']==app:
                            drive('wait-idle');return
                        time.sleep(.05)
                    raise AssertionError((app,get('simulation')['navigation']))
                def has(name):
                    expected = {'earlier':'PagePrevious(', 'previous':'PagePrevious(', 'later':'PageNext(', 'next':'PageNext('}.get(name)
                    return any(n.get('action')==action_id(name) and (expected is None or n.get('kind','').startswith(expected)) for n in get('layout')['nodes'])
                def open_app(app):
                    for _ in range(20):
                        if has('open-'+app):break
                        if has('next'):drive('tap-id next')
                        elif has('previous'):drive('tap-id previous')
                        else:raise AssertionError('app absent from launcher: '+app)
                    drive('tap-id open-'+app);wait_app(app)
                def stay(app):
                    time.sleep(2.1)
                    assert get('simulation')['navigation']['app']==app
                wait_app('launcher')
                drive('tap-id settings');wait_app('settings')
                drive('clean','shot settings-home','tap-id wifi','wait-idle','clean','shot settings-wifi')
                (output/'wifi-layout.json').write_text(json.dumps(get('layout'),indent=2)+'\n')
                if not has('wifi-network-eduroam'):drive('tap-id toggle','wait-idle')
                def enter_trust():
                    drive('tap-id wifi-network-eduroam','wait-idle','type reader','tap-id kb.enter','type syntheticpass','tap-id kb.enter','wait-idle','expect 5E5E5E5E')
                enter_trust()
                drive('clean','shot enterprise-trust-cancel')
                drive('tap-id distrust-server','wait-idle');stay('settings')
                drive('expect-missing Tap to disconnect','clean','shot enterprise-cancelled')
                enter_trust()
                drive('clean','shot enterprise-trust-confirm','tap-id trust-server','wait-idle','expect Tap to disconnect','clean','shot enterprise-connected')
                (output/'connected-layout.json').write_text(json.dumps(get('layout'),indent=2)+'\n')
                drive('tap Back');stay('settings')
                drive('tap Back');wait_app('launcher')
                drive('clean','shot launcher-returned')
                checks.append('Synthetic enterprise network: typed username/password; certificate shown; refusal returns Wi-Fi disconnected; retry and explicit trust joins; nested Back remains Settings, root Back returns launcher')
                (output/'result.json').write_text(json.dumps({'source':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'cli_sha256':hashlib.sha256(cli.read_bytes()).hexdigest(),'profile':'clara-bw-391','scale':args.scale,'runtime_navigation':True,'checks':checks,'status':'passed','hardware':False,'validation_fixture':True,'production_repair_patch_sha256':hashlib.sha256((output.parent/'trust-repair-final.patch').read_bytes()).hexdigest(),'fixture_patch_sha256':hashlib.sha256((output.parent/'enterprise-fixture-final.patch').read_bytes()).hexdigest()},indent=2)+'\n')
            finally:
                if process is not None and process.poll() is None:
                    os.killpg(process.pid,signal.SIGTERM)
                    process.wait(timeout=15)

if __name__=='__main__':main()
