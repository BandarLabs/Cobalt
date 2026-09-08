#!/usr/bin/env python3
"""Play original Crossword fixtures through actual SDK IPC, including failed saves and restart."""
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
    with tempfile.TemporaryDirectory(prefix='cobalt-crossword-', dir='/tmp') as private:
        env = dict(os.environ, TMPDIR=private, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE=args.profile, KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-crossword-pack', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS='1788850860000', KOBO_SIM_UTC_OFFSET_MINUTES='0')
        store_root = Path(private)/'cobalt-sim-state/crossword'
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            try:
                def start():
                    nonlocal process
                    log.seek(0)
                    log.truncate()
                    process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'], cwd=ROOT/'apps/crossword', env=env,
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
                    assert provenance['app']=='crossword' and provenance['mode']=='single-app'
                    assert provenance['source']['fixture']=='original-crossword-pack'
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

                store = store_root/'crossword-state-v1'
                def saved():
                    drive('wait-idle')
                    return json.loads(store.read_text())['payload']
                def restart(expected="more"):
                    nonlocal address
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                    address = start()
                    drive('wait-for-id '+expected, 'wait-idle')
                drive('wait-for-id puzzle-3')
                capture('01-puzzles')
                drive('tap-id puzzle-3', 'tap-id cell-0', 'type cat', 'tap Enter', 'wait-idle')
                assert saved()['games'][3]['position']['letters'].startswith('CAT##')
                capture('02-numbered-grid')
                drive('tap-id cell-5', 'type ore')
                capture('03-clue-and-entry')
                drive('tap Enter')
                before = saved()
                restart()
                assert saved()==before
                capture('04-restored-grid')
                drive('tap-id more', 'tap-id undo')
                assert saved()['games'][3]['position']['letters'].startswith('CAT##...##')
                drive('tap-id cell-5', 'type ore', 'scenario storage-full', 'tap Enter', 'wait-idle', 'expect Progress is not saved')
                capture('05-save-recovery')
                assert saved()['games'][3]['position']['letters'].startswith('CAT##...##')
                drive('scenario normal', 'tap-id retry-save', 'wait-for-id more')
                assert saved()['games'][3]['position']['letters'].startswith('CAT##ORE##')
                drive('tap-id cell-10', 'type wrong', 'tap Enter', 'tap-id more', 'tap-id check', 'expect incorrect')
                capture('06-check-word')
                drive('tap-id board', 'tap-id more', 'tap-id reveal')
                capture('07-reveal-confirmation')
                drive('tap-id confirm-reveal', 'wait-idle')
                assert saved()['games'][3]['reveals']==1
                drive('tap-id more','tap-id undo','tap-id cell-10','type wedge','tap Enter',
                      'tap-id cell-17','type dom','tap Enter','tap-id cell-22','type you','tap Enter','expect Puzzle complete')
                final = saved()
                assert final['games'][3]['solved'] and final['games'][3]['checks']==1
                capture('08-completed-crossword')
                restart()
                assert saved()==final
                capture('09-restored-completion')
                drive('tap-id more','tap-id restart')
                capture('10-restart-confirmation')
                drive('tap-id confirm-restart','tap-id more','tap-id undo','expect Puzzle complete')
                drive('tap-id more','tap-id help')
                for page in range(16):
                    capture(f'help-{page+1:02}')
                    old=get('layout')
                    drive('tap-id help-next')
                    if old==get('layout'): break
                else: raise AssertionError('Unbounded help')
                drive('tap Back', 'tap-id clues')
                for page in range(3):
                    capture(f'across-clues-{page+1}')
                    drive('tap-id clues-next')
                drive('tap-id direction')
                capture('11-down-clues')
                drive('tap-id clue-0','type cow','tap Enter')
                capture('12-down-selection')
                drive('tap Back')
                subprocess.run([str(cli),'drive','--address',address,'--ideal','--shots',str(args.output/'route'),
                    '--script',str(ROOT/'apps/crossword/drive.kobo')],cwd=ROOT,env=env,stdout=log,stderr=log,check=True,timeout=45)
                # All files below belong to this temporary simulator fixture.
                os.killpg(process.pid, signal.SIGKILL); process.wait(timeout=5)
                legacy=b'H........................;0;1'
                store.write_bytes(legacy)
                address=start()
                drive('wait-for-id more','expect Heart of the matter')
                assert store.read_bytes()==legacy
                capture('13-legacy-progress')
                drive('tap-id cell-0','type e','tap Enter','wait-idle')
                assert saved()['games'][2]['position']['letters'].startswith('E')
                restart()
                capture('14-migrated-progress')
                os.killpg(process.pid, signal.SIGKILL); process.wait(timeout=5)
                invalid=b'keep this unreadable record'
                store.write_bytes(invalid)
                address=start()
                drive('wait-for-id retry-load','tap-id retry-load','wait-idle','expect Cannot open progress')
                assert store.read_bytes()==invalid
                capture('15-unreadable-preserved')
                result=dict(status='passed', profile=args.profile, scale=args.scale, basis='simulator-sdk-ipc',
                    source_head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                    source_dirty=bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),
                    checks=['numbered blocked crossword','clue with word entry','exact restart','persistent undo',
                        'full-storage preservation','save retry','optional checking','confirmed reveal and undo',
                        'full completion','completion restart','restart confirmation and undo','all help pages',
                        'across clue pages','down clue and entry','committed drive route','legacy migration','unreadable-record preservation'])
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
