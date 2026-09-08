#!/usr/bin/env python3
"""Play original Sudoku fixtures through actual SDK IPC, including failed saves and restart."""
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
    puzzle, solution = ROOT.joinpath('apps/sudoku/assets/puzzles.txt').read_text().splitlines()[0].split('|')[1:]
    cell = puzzle.index('0')
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-sudoku-', dir='/tmp') as private:
        env = dict(os.environ, TMPDIR=private, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE=args.profile, KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-sudoku-pack', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS='1788850860000', KOBO_SIM_UTC_OFFSET_MINUTES='0')
        store = Path(private)/'cobalt-sim-state/sudoku/game'
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            try:
                def start():
                    nonlocal process
                    log.seek(0)
                    log.truncate()
                    process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'], cwd=ROOT/'apps/sudoku', env=env,
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
                    assert provenance['app']=='sudoku' and provenance['mode']=='single-app'
                    assert provenance['source']['fixture']=='original-sudoku-pack'
                    assert len(provenance['source']['binarySha256'])==64 and provenance['fonts']
                    with Image.open(args.output/(name+'.png')) as image:
                        assert image.size==(provenance['simulation']['profile']['width'],provenance['simulation']['profile']['height'])

                def saved():
                    drive('wait-idle')
                    return json.loads(store.read_text())['payload']

                def restart():
                    nonlocal address
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                    address = start()
                    drive('wait-for-id more','wait-idle')

                drive('wait-for-id more','wait-idle')
                capture('01-first-puzzle')
                drive(f'tap-id cell-{cell}', 'tap-id pencil', 'tap-id digit-2', 'tap-id digit-9','wait-idle')
                before = saved()
                assert int(before['position']['notes'][cell*3:cell*3+3],16)==258
                capture('02-pencil-notes')
                restart()
                assert saved()==before
                capture('03-restored-notes')
                drive('tap-id undo','wait-idle')
                assert int(saved()['position']['notes'][cell*3:cell*3+3],16)==2
                drive('scenario storage-full','tap-id digit-3','wait-idle','expect Not saved')
                capture('04-unsaved-move')
                assert int(saved()['position']['notes'][cell*3:cell*3+3],16)==2, 'Failed write replaced durable data'
                drive('tap-id more','expect Retry save')
                capture('05-save-recovery')
                drive('scenario normal','tap-id retry-save','wait-idle','expect Saved','tap-id play')
                assert int(saved()['position']['notes'][cell*3:cell*3+3],16)==6
                restart()
                assert int(saved()['position']['notes'][cell*3:cell*3+3],16)==6
                drive('tap-id pencil',f'tap-id digit-{int(solution[cell])%9+1}','wait-idle')
                assert saved()['position']['board'][cell]==str(int(solution[cell])%9+1)
                assert not saved()['checking']
                capture('06-unassisted-entry')
                drive('tap-id more','tap-id checking','tap-id play','expect Check this answer')
                capture('07-optional-checking')
                drive('tap-id more','tap-id hint','expect Reveal answer?')
                capture('08-reveal-confirmation')
                drive('tap-id play')
                for index, (clue, answer) in enumerate(zip(puzzle,solution)):
                    if clue=='0':
                        drive(f'tap-id cell-{index}',f'tap-id digit-{answer}')
                drive('wait-idle','expect Puzzle complete')
                assert saved()['position']['board']==solution
                capture('09-completed-puzzle')
                restart()
                drive('expect Puzzle complete')
                capture('10-completion-restored')
                drive('tap-id undo','wait-idle')
                assert saved()['position']['board']!=solution
                drive('tap-id more','tap-id new-game')
                capture('11-difficulty-choice')
                drive('tap-id new-hard','wait-idle')
                assert 24<=int(saved()['puzzle'])<36
                capture('12-hard-puzzle')
                drive('tap-id more','tap-id view','tap-id how-to-play')
                for page in range(12):
                    capture(f'13-help-{page+1}')
                    before_help = json.loads(get('layout'))
                    drive('tap-id help-next')
                    if json.loads(get('layout')) == before_help:
                        break
                else:
                    raise AssertionError('Help did not finish within 12 pages')
                drive('tap Back','tap-id more','tap-id view','tap-id rotate','wait-idle','expect Rows 1–6')
                capture('14-landscape-upper')
                drive('tap-id rows','expect Rows 4–9','tap-id cell-80','wait-idle')
                capture('15-landscape-lower')
                before_rotation_restart = saved()
                assert before_rotation_restart['landscape'] and before_rotation_restart['position']['selected']=='80'
                restart()
                assert saved()==before_rotation_restart
                drive('expect Rows 4–9')
                capture('16-landscape-restored')
                drive('tap-id more','tap-id view','tap-id how-to-play')
                for page in range(12):
                    capture(f'17-landscape-help-{page+1}')
                    before_help = json.loads(get('layout'))
                    drive('tap-id help-next')
                    if json.loads(get('layout')) == before_help:
                        break
                else:
                    raise AssertionError('Landscape help did not finish within 12 pages')
                drive('tap Back','tap-id more','tap-id view','tap-id rotate','tap-id more','tap-id new-game','tap-id new-easy','wait-idle')
                subprocess.run([str(cli),'drive','--address',address,'--ideal','--shots',str(args.output/'route'),'--script',str(ROOT/'apps/sudoku/drive.kobo')],cwd=ROOT,env=env,stdout=log,stderr=log,check=True,timeout=45)
                result = dict(status='passed',profile=args.profile,scale=args.scale,original_fixture=True,
                              basis='simulator-sdk-ipc',puzzles=36,source_head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                              source_dirty=bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),
                              pack_sha256=hashlib.sha256(ROOT.joinpath('apps/sudoku/assets/puzzles.txt').read_bytes()).hexdigest(),
                              checks=['notes','exact restart','persistent undo','failed-save preservation','explicit retry','optional checking','reveal confirmation','full completion','completion restart','new difficulty','all help pages','landscape row windows','landscape restart','landscape help','committed drive route'])
                (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
            finally:
                if process is not None and process.poll() is None:
                    os.killpg(process.pid,signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid,signal.SIGKILL)
                        process.wait(timeout=5)

if __name__=='__main__':
    main()
