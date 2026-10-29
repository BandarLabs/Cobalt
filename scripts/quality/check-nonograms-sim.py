#!/usr/bin/env python3
"""Play original Nonograms fixtures through actual SDK IPC, including failed saves and restart."""
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
    with tempfile.TemporaryDirectory(prefix='cobalt-nonograms-', dir='/tmp') as private:
        env = dict(os.environ, TMPDIR=private, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE=args.profile, KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-nonograms-pack', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS='1788850860000', KOBO_SIM_UTC_OFFSET_MINUTES='0')
        store_root = Path(private)/'cobalt-sim-state/nonograms'
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            try:
                def start():
                    nonlocal process
                    log.seek(0)
                    log.truncate()
                    process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'], cwd=ROOT/'apps/nonograms', env=env,
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
                    drive('wait-idle', 'expect-state /activity#/effects/fetch 0', 'expect-state /activity#/effects/post 0', 'expect-state /activity#/effects/put 0', 'expect-state /activity#/effects/patch 0', 'shot '+name)
                    diagnostics = json.loads(get('diagnostics'))
                    assert not [issue for issue in diagnostics['issues'] if issue['severity']=='error'], diagnostics
                    layout = json.loads(get('layout'))
                    (args.output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                    provenance = json.loads((args.output/(name+'.json')).read_text())
                    assert provenance['app']=='nonograms' and provenance['mode']=='single-app'
                    assert provenance['source']['fixture']=='original-nonograms-pack'
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

                def choose(index):
                    drive('wait-for-id tab-patterns')
                    drive('tap-id ' + ('tab-patterns' if index < 60 else 'tab-pictures'))
                    for _ in range(60):
                        if has(f'puzzle-{index}'):
                            drive(f'tap-id puzzle-{index}', 'wait-for-id more', 'wait-idle')
                            return
                        drive('tap-id next-page')
                    raise AssertionError('Puzzle not reachable')

                def saved(index=0):
                    drive('wait-idle')
                    return json.loads((store_root/f'progress-pack-{index:02}').read_text())['payload']

                def restart(index=0):
                    nonlocal address
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                    address = start()
                    choose(index)

                choose(0)
                capture('01-attached-clues')
                drive('tap-id board.cell.0', 'wait-idle')
                before = saved()
                assert before['marks'][0] == '#'
                capture('02-marked-square')
                drive('tap-id board.row.0', 'expect Row 1')
                capture('03-full-row-clue')
                drive('tap-id resume')
                restart()
                assert saved() == before
                capture('04-restored-mark')
                drive('tap-id undo', 'wait-idle')
                assert saved()['marks'][0] == '.'
                drive('tap-id more', 'tap-id run-entry', 'tap-id resume', 'wait-idle')
                drive('tap-id board.cell.0', 'tap-id board.cell.4', 'wait-idle')
                assert saved()['marks'][:5] == '#####'
                capture('05-whole-run')
                restart()
                drive('tap-id undo', 'wait-idle')
                assert saved()['marks'][:5] == '.....'
                drive('tap-id more', 'tap-id run-entry', 'tap-id resume', 'wait-idle')
                durable = (store_root/'progress-pack-00').read_bytes()
                drive('scenario storage-full', 'tap-id board.cell.0', 'wait-idle')
                assert (store_root/'progress-pack-00').read_bytes() == durable
                drive('tap-id more')
                capture('06-save-recovery')
                drive('scenario normal', 'tap-id retry-save', 'wait-idle', 'tap-id resume')
                latest = saved()
                assert latest['marks'][0] == '#'
                restart()
                assert saved() == latest
                drive('tap-id more', 'tap-id reset')
                capture('07-restart-confirmation')
                drive('tap Back', 'tap-id more', 'tap-id reset', 'tap-id confirm-reset', 'wait-idle', 'tap-id undo', 'wait-idle')
                assert saved()['marks'][0] == '#'
                # Finish the original 5x5 puzzle through visible cell actions.
                for cell in range(1,25):
                    for _ in range(8):
                        if has(f'board.cell.{cell}'): break
                        drive('tap-id board.down')
                    else: raise AssertionError('Completion square not reachable')
                    drive(f'tap-id board.cell.{cell}', 'wait-idle')
                    if cell >= 10:
                        drive(f'tap-id board.cell.{cell}', 'wait-idle')
                drive('wait-for-id next-puzzle', 'wait-idle')
                assert saved()['marks'] == '##########xxxxxxxxxxxxxxx'
                capture('12-completed-puzzle')
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
                address = start()
                drive('wait-for-id tab-patterns', 'tap-id tab-patterns', 'tap-id puzzle-0', 'wait-for-id next-puzzle', 'wait-idle')
                capture('13-restored-completion')
                drive('tap-id undo', 'wait-for-id more', 'wait-idle')
                assert saved()['marks'][24] == '#'
                drive('tap-id more', 'tap-id back-browser')
                choose(48)
                capture('08-large-board')
                for direction in ['board.right', 'board.down']:
                    for _ in range(30):
                        if not has(direction): break
                        drive('tap-id '+direction)
                    else: raise AssertionError('Panning did not stop')
                assert has('board.cell.624')
                drive('tap-id board.cell.624', 'wait-idle')
                assert saved(48)['marks'][624] == '#'
                capture('09-last-square')
                restart(48)
                assert saved(48)['marks'][624] == '#' and has('board.cell.624')
                capture('10-restored-last-square')
                drive('tap-id more', 'tap-id back-browser')
                drive('tap-id how-to-play')
                for page in range(16):
                    capture(f'11-help-{page+1}')
                    before_help = json.loads(get('layout'))['nodes']
                    drive('tap-id help-next')
                    if json.loads(get('layout'))['nodes'] == before_help: break
                else: raise AssertionError('Help did not finish')
                drive('tap Back')
                # The picture demonstration is separate from earlier games.
                # Preserve the earlier record while opening and saving a new picture.
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
                earlier_bytes = (store_root/'progress-pack-00').read_bytes()
                address = start()
                drive('wait-for-id tab-pictures')
                subprocess.run([str(cli),'drive','--address',address,'--ideal','--shots',str(args.output/'route'),'--script',str(ROOT/'apps/nonograms/drive.kobo')],cwd=ROOT,env=env,stdout=log,stderr=log,check=True,timeout=60)
                capture('14-picture-selection')
                picture_key = store_root/'progress-picture-house-v1'
                picture_before = picture_key.read_bytes()
                assert (store_root/'progress-pack-00').read_bytes() == earlier_bytes
                restart(60)
                assert picture_key.read_bytes() == picture_before
                assert (store_root/'progress-pack-00').read_bytes() == earlier_bytes
                capture('15-picture-restored')
                drive('tap-id more', 'tap-id run-entry', 'tap-id resume', 'wait-idle')
                house = (ROOT/'apps/nonograms/assets/pictures.txt').read_text().splitlines()[0].split('|')[3]
                for cell, target in enumerate(house):
                    for _ in range(8):
                        if has(f'board.cell.{cell}'): break
                        direction = 'board.up' if cell < 5 else 'board.down'
                        drive('tap-id '+direction)
                    else: raise AssertionError('Picture square not reachable')
                    wanted = '#' if target == '#' else 'x'
                    for _ in range(3):
                        if json.loads(picture_key.read_text())['payload']['marks'][cell] == wanted: break
                        drive(f'tap-id board.cell.{cell}', 'wait-idle')
                drive('wait-for-id next-puzzle', 'wait-idle')
                assert (store_root/'progress-pack-00').read_bytes() == earlier_bytes
                capture('16-picture-completed')
                # Imported photo puzzles arrive as a manifest plus images in
                # the app's transfer directory.
                def browser_text():
                    return ' '.join(line for node in json.loads(get('layout'))['nodes'] for line in node['lines'])
                data_root = Path(private)/'cobalt-sim-data/nonograms'
                data_root.mkdir(parents=True, exist_ok=True)
                rows_image = Image.new('L', (10, 10))
                rows_image.putdata([24 if index//10 < 5 else 232 for index in range(100)])
                rows_image.save(data_root/'moon.png')
                columns_image = Image.new('L', (10, 10))
                columns_image.putdata([24 if index%10 < 5 else 232 for index in range(100)])
                columns_image.save(data_root/'eclipse.png')
                (data_root/'imported.txt').write_text('moon.png\tThe Moon\t5\neclipse.png\tEclipse\t5\n')
                # No Import button: the app reads the shelf again on waking.
                drive('tap-id next-puzzle', 'wait-for-id tab-photos', 'lifecycle background',
                      'lifecycle foreground', 'tap-id tab-photos', 'wait-for Eclipse')
                assert '2 new photo puzzles.' in browser_text(), browser_text()
                capture('17-imported-notice')
                seen = set()
                for _ in range(30):
                    seen.update(name for name in ['The Moon', 'Eclipse'] if name in browser_text())
                    if len(seen) == 2 or not has('next-page'):
                        break
                    drive('tap-id next-page', 'wait-idle')
                assert seen == {'The Moon', 'Eclipse'}, seen
                capture('18-imported-puzzles')
                # A push that no longer names a photo drops it from the shelf.
                (data_root/'imported.txt').write_text('moon.png\tThe Moon\t5\n')
                drive('lifecycle background', 'lifecycle foreground', 'wait-for The Moon', 'wait-idle')
                capture('19-import-synced-notice')
                eclipse_gone = True
                moon_found = False
                for _ in range(30):
                    page_text = browser_text()
                    if 'Eclipse' in page_text:
                        eclipse_gone = False
                    if 'The Moon' in page_text:
                        moon_found = True
                    if not has('next-page'):
                        break
                    drive('tap-id next-page', 'wait-idle')
                assert eclipse_gone and moon_found
                capture('20-import-synced')
                # Two pushes from the companion add to the photos already on
                # the reader instead of replacing them.
                from PIL import ImageDraw
                disc = Image.new('L', (360, 360), 255)
                ImageDraw.Draw(disc).ellipse((40, 40, 320, 320), fill=0)
                disc.save(Path(private)/'disc.png')
                plus = Image.new('L', (360, 360), 255)
                ImageDraw.Draw(plus).rectangle((140, 0, 220, 360), fill=0)
                ImageDraw.Draw(plus).rectangle((0, 140, 360, 220), fill=0)
                plus.save(Path(private)/'plus.png')
                for image, name in (('disc.png', 'Full moon'), ('plus.png', 'Crossroads')):
                    pushed = subprocess.run([str(cli), 'nonograms', 'push', str(Path(private)/image),
                                             '--name', name, '--size', '9', '--sim'],
                                            cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
                    assert pushed.returncode == 0, pushed.stderr
                assert 'On the reader: 3 photo puzzles' in pushed.stdout, pushed.stdout
                drive('lifecycle background', 'lifecycle foreground', 'wait-for Crossroads',
                      'wait-for Full moon', 'wait-for The Moon')
                assert '2 new photo puzzles.' in browser_text(), browser_text()
                capture('21-two-pushes-added')
                result = dict(status='passed',profile=args.profile,scale=args.scale,original_fixture=True,
                    source_head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                    source_dirty=bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),
                    checks=['attached clues','mark selection','full clue inspection','forced restart','persistent atomic run undo','failed-save preservation','explicit retry','confirmed restart undo','full completion','completion restart and undo','25x25 panning','last square restart','all help pages','committed route','new picture restart','earlier save preserved','new picture completion','manifest import','imported names listed','import sync removes missing photos','imports picked up on wake','companion pushes add photos'])
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
