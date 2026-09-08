#!/usr/bin/env python3
"""Play original Logic Pack fixtures through actual SDK IPC, including failed saves and restart."""
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
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--profile', default='clara-bw-391')
    parser.add_argument('--scale', default='extra-large')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT/'target'))).resolve()
    cli = target/'debug/kobo'
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-logicpack-', dir='/tmp') as private:
        env = dict(os.environ, TMPDIR=private, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE=args.profile, KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-logicpack-games', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS='1788850860000', KOBO_SIM_UTC_OFFSET_MINUTES='0')
        store_root = Path(private)/'cobalt-sim-state/logicpack'
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            try:
                def start():
                    nonlocal process
                    log.seek(0)
                    log.truncate()
                    process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'], cwd=ROOT/'apps/logicpack', env=env,
                                               stdout=log, stderr=log, start_new_session=True)
                    deadline = time.monotonic()+120
                    while time.monotonic()<deadline:
                        if process.poll() is not None:
                            raise RuntimeError('Simulator exited: '+log_path.read_text()[-2000:])
                        match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                        if match:
                            return match.group(1)
                        time.sleep(.1)
                    raise RuntimeError('Simulator did not start')

                address = start()

                def drive(*steps):
                    command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(args.output)]
                    for step in steps:
                        command.extend(['--step', step])
                    subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, check=True, timeout=45)

                def get(endpoint):
                    with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as response:
                        return response.read()

                def capture(name):
                    drive('wait-idle', 'expect-state /activity#/effects/fetch 0', 'expect-state /activity#/effects/post 0', 'shot '+name)
                    diagnostics = json.loads(get('diagnostics'))
                    assert not [issue for issue in diagnostics['issues'] if issue['severity']=='error'], diagnostics
                    layout = json.loads(get('layout'))
                    (args.output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                    provenance = json.loads((args.output/(name+'.json')).read_text())
                    assert provenance['app']=='logicpack' and provenance['mode']=='single-app'
                    assert provenance['source']['fixture']=='original-logicpack-games'
                    assert len(provenance['source']['binarySha256'])==64 and provenance['fonts']
                    with Image.open(args.output/(name+'.png')) as image:
                        assert image.size==(provenance['simulation']['profile']['width'],provenance['simulation']['profile']['height'])

                def aid(name):
                    value = 0x811c9dc5
                    for byte in name.encode():
                        value = ((value ^ byte) * 0x01000193) & 0xffffffff
                    return max(value, 1)

                def has(name):
                    return any(node['action'] == aid(name) for node in json.loads(get('layout'))['nodes'])

                store=store_root/'logicpack-state-v1'
                def saved():
                    drive('wait-idle')
                    return json.loads(store.read_text())['payload']
                def restart(expected='restart'):
                    nonlocal address
                    os.killpg(process.pid,signal.SIGKILL)
                    process.wait(timeout=5)
                    address=start()
                    drive('wait-for-id '+expected,'wait-idle')
                def position(index):
                    return saved()['games'][index]['position'].split('|')
                drive('wait-for-id slither')
                capture('01-puzzles')
                drive('tap-id hashi','tap-id route-0','wait-idle')
                before=saved()
                restart()
                assert saved()==before
                drive('tap-id back','tap-id kakuro','tap-id kakuro-1','tap-id back','tap-id hashi')
                assert position(1)[1].startswith('1,')
                drive('tap-id undo')
                assert position(1)[1].startswith('0,')
                drive('tap-id route-0','scenario storage-full','tap-id route-1','wait-for-id retry-save')
                assert position(1)[1].startswith('1,0,')
                capture('02-save-failed')
                drive('scenario normal','tap-id retry-save','wait-for-id check')
                assert position(1)[1].startswith('1,1,')
                drive('tap-id route-2','tap-id route-3','tap-id check','expect Solved.')
                assert position(1)[-1]=='1'
                capture('03-hashi-complete')
                before=saved(); restart(); assert saved()==before
                drive('tap-id restart')
                capture('04-restart-confirmation')
                drive('tap-id cancel-restart'); assert saved()==before
                drive('tap-id restart','tap-id confirm-restart','tap-id undo')
                assert position(1)[-1]=='1'
                drive('tap-id back','tap-id kakuro')
                assert position(2)[1].split(',')[1]=='1'
                for cell,count in [(0,3),(1,1),(2,4)]:
                    for _ in range(count): drive('tap-id kakuro-'+str(cell))
                drive('tap-id check','expect Solved.')
                capture('05-kakuro-complete')
                before=saved(); restart(); assert saved()==before
                drive('tap-id back','tap-id slither')
                for edge in range(12):
                    if 0b101101110011 & (1<<edge): drive('tap-id edge-'+str(edge))
                drive('tap-id check','expect Solved.')
                capture('06-slitherlink-complete')
                before=saved(); restart(); assert saved()==before
                drive('tap-id back','tap-id mines','tap-id mine-5','tap-id undo')
                assert position(3)[2]=='33824' and position(3)[4]=='1'
                drive('tap-id mine-0','tap-id mine-5','expect Mine opened','tap-id undo')
                assert position(3)[-2]=='0'
                for cell in range(16):
                    if cell not in (5,10,15): drive('tap-id mine-'+str(cell))
                drive('expect Field cleared.')
                assert position(3)[-1]=='1'
                capture('07-mines-complete')
                before=saved(); restart(); assert saved()==before
                for game in ['mines','hashi','kakuro','slither']:
                    drive('tap Back','tap-id '+game,'tap-id how-to-play')
                    capture('08-help-'+game)
                    drive('tap-id close-help')
                # Reopening each completed game never resets another game's marks.
                assert all(run['position'].endswith('|0|1') for run in saved()['games'])
                legacy=b'2|1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0'
                os.killpg(process.pid,signal.SIGKILL); process.wait(timeout=5)
                store.write_bytes(legacy); address=start(); drive('wait-for-id check','wait-idle')
                assert store.read_bytes()==legacy
                drive('tap-id route-1'); assert position(1)[1].startswith('1,1,')
                capture('09-legacy-migrated')
                os.killpg(process.pid,signal.SIGKILL); process.wait(timeout=5)
                store.unlink(); address=start(); drive('wait-for-id mines')
                route=[line.strip() for line in (ROOT/'apps/logicpack/drive.kobo').read_text().splitlines() if line.strip() and not line.startswith('#')]
                drive(*route)
                capture('10-committed-route')
                os.killpg(process.pid,signal.SIGKILL); process.wait(timeout=5)
                invalid=b'{"schema":"logicpack.games","version":99,"payload":{}}'
                store.write_bytes(invalid); address=start(); drive('wait-for-id retry-load','wait-idle')
                assert store.read_bytes()==invalid
                capture('10-unreadable-preserved')
                result=dict(status='passed',profile=args.profile,scale=args.scale,basis='simulator-sdk-ipc',
                    source_head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                    source_dirty=bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),
                    checks=['separate game progress','forced restart restoration','persistent undo','full-storage preservation',
                            'explicit save retry','restart confirmation and undo','Hashi completion and reopen',
                            'Kakuro completion and reopen','Slitherlink completion and reopen','Mines completion and reopen',
                            'first-mine relocation undo','loss undo','all game help screens','legacy migration','future record preservation','committed drive route'])
                (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
            finally:
                if process is not None and process.poll() is None:
                    os.killpg(process.pid,signal.SIGTERM)
                    try: process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid,signal.SIGKILL)
                        process.wait(timeout=5)

if __name__=='__main__':
    main()
