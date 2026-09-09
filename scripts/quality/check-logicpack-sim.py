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
                pack=json.loads((ROOT/'apps/logicpack/assets/collection.json').read_text())
                def saved():
                    drive('wait-idle')
                    return json.loads(store.read_text())['payload']
                def restart(expected='more'):
                    nonlocal address
                    os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)
                    address=start();drive('wait-for-id '+expected,'wait-idle')
                def home():
                    for _ in range(4):
                        if has('slither'):return
                        drive('tap Back','wait-idle')
                    assert has('slither'),'Cannot return to games'
                def choose(index):
                    home();drive('tap-id '+pack[index]['kind'],'wait-idle')
                    if not has('puzzle-'+str(index)):drive('tap-id next-puzzles','wait-idle')
                    drive('tap-id puzzle-'+str(index),'wait-for-id more','wait-idle')
                def position(index):
                    return saved()['games'][index]['position'].split('|')
                def tool(name):
                    drive('tap-id more','tap-id '+name,'wait-idle')
                drive('wait-for-id slither')
                capture('01-games')
                drive('tap-id slither','wait-idle')
                capture('02-puzzle-picker')
                completed=[]
                for index,puzzle in enumerate(pack):
                    choose(index)
                    kind=puzzle['kind']
                    steps=[]
                    if kind=='slither':
                        steps=['tap-id edge-'+str(i) for i,n in enumerate(puzzle['solution']) if n]
                    elif kind=='hashi':
                        steps=['tap-id route-'+str(i) for i,n in enumerate(puzzle['solution']) for _ in range(n)]
                    elif kind=='kakuro':
                        free=[n for n,given in zip(puzzle['solution'],puzzle['givens']) if not given]
                        for i,n in enumerate(free):
                            steps+=['tap-id kakuro-'+str(i),'wait-for-id digit-0','tap-id digit-'+str(n),'wait-for-id more']
                    else:
                        steps=['tap-id mine-'+str(i) for i in range(puzzle['side']**2) if i not in puzzle['mines']]
                    # Each mark is committed before resolving the next touch location.
                    route=[]
                    for step in steps:route+=[step,'wait-idle']
                    drive(*route)
                    if kind!='mines':drive('tap-id check','expect Solved.','wait-idle')
                    else:drive('expect Field cleared.','wait-idle')
                    before=saved();assert before['games'][index]['id']==puzzle['id']
                    assert before['games'][index]['position'].endswith('|0|1'),puzzle['id']
                    capture(f'completed-{index:02}-{kind}')
                    restart();assert saved()==before,puzzle['id']+' restart changed progress'
                    completed.append(puzzle['id'])
                    if index in (7,11,15,19):
                        capture('reopened-'+kind)
                        drive('tap-id how-to-play','wait-idle')
                        capture('rules-'+kind)
                        drive('tap-id next-help','wait-idle')
                        capture('difficulty-'+kind)
                        drive('tap-id close-help','wait-for-id more')
                assert len(completed)==20 and all(run['position'].endswith('|0|1') for run in saved()['games'])

                choose(15)
                original=saved()['games'][15]['position']
                drive('tap-id kakuro-0','wait-for-id digit-0')
                capture('03-kakuro-entry')
                drive('tap-id digit-0','wait-for-id more','wait-idle')
                assert position(15)[1].split(',')[0]=='0'
                tool('undo');assert saved()['games'][15]['position']==original
                tool('restart');capture('04-restart-confirmation')
                drive('tap-id cancel-restart');assert saved()['games'][15]['position']==original
                tool('restart');drive('tap-id confirm-restart','wait-for-id more','wait-idle')
                assert all(n=='0' for n in position(15)[1].split(','))
                tool('undo');assert saved()['games'][15]['position']==original

                choose(1);before=store.read_bytes()
                drive('scenario storage-full','tap-id route-0','wait-for-id retry-save','wait-idle')
                assert store.read_bytes()==before
                capture('05-save-failed')
                drive('scenario normal','tap-id retry-save','wait-for-id more','wait-idle')
                assert position(1)[1].split(',')[0]=='2'
                capture('06-double-bridge')
                tool('undo');assert position(1)[1].split(',')[0]=='1'
                choose(0);drive('tap-id edge-0','wait-idle')
                capture('07-excluded-edge')
                tool('undo')

                choose(3);tool('restart');drive('tap-id confirm-restart','wait-idle')
                before=saved()['games'][3]['position']
                drive('tap-id mine-5','wait-idle');assert position(3)[2]!='33824'
                tool('undo');assert saved()['games'][3]['position']==before
                drive('tap-id mine-0','tap-id mine-5','expect Mine opened','wait-idle')
                tool('undo');assert position(3)[-2]=='0'

                legacy=b'2|1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0'
                old_games=[dict(id=p['id'],position='',undo=[]) for p in pack[:4]]
                old_games[1].update(position=legacy.decode(),undo=['2|0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0'])
                old_record=json.dumps(dict(schema='logicpack.games',version=1,payload=dict(current=2,games=old_games))).encode()
                for name,record in [('single',legacy),('four-game',old_record)]:
                    os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)
                    store.write_bytes(record);address=start();drive('wait-for-id more','wait-idle')
                    assert store.read_bytes()==record,'Read must not rewrite older saves'
                    if name=='four-game':tool('undo');assert position(1)[1].startswith('0,0,')
                    else:drive('tap-id route-1','wait-idle');assert position(1)[1].startswith('1,1,')
                    assert json.loads(store.read_text())['version']==2 and len(saved()['games'])==20
                    assert all(not run['position'] for run in saved()['games'][4:])
                    capture('migrated-'+name)

                os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)
                store.unlink();address=start();drive('wait-for-id slither')
                route=[line.strip() for line in (ROOT/'apps/logicpack/drive.kobo').read_text().splitlines() if line.strip() and not line.startswith('#')]
                drive(*route);capture('08-committed-route')
                os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)
                invalid=b'{"schema":"logicpack.games","version":99,"payload":{}}'
                store.write_bytes(invalid);address=start();drive('wait-for-id retry-load','wait-idle')
                assert store.read_bytes()==invalid
                capture('09-unreadable-preserved')
                result=dict(status='passed',profile=args.profile,scale=args.scale,basis='simulator-sdk-ipc',
                    source_head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                    source_dirty=bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),
                    completed_and_reopened=completed,
                    checks=['all 20 puzzles complete and reopen exactly','separate puzzle identities and progress',
                        'paged puzzle picker','rules and difficulty for each game','direct Kakuro digit entry and clear',
                        'persistent undo','confirmed restart and undo','full-storage preservation and explicit retry',
                        'single and double bridges','excluded loop edges','first-mine relocation and loss undo',
                        'single-game legacy migration','four-game record and history migration','committed drive route',
                        'future record preservation','zero network effects'])
                (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
            finally:
                if process is not None and process.poll() is None:
                    os.killpg(process.pid,signal.SIGTERM)
                    try:process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)

if __name__=='__main__':main()
