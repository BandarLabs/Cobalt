#!/usr/bin/env python3
"""Play seeded backgammon turns against the computer in the actual simulator:
roll, tap a checker on the board and a marked point, watch the computer reply
without a tap, read the record of play, and find it all after a restart."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
SEED = '7'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='default')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT/'target'))).resolve()
    cli = target/'debug/kobo'
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-backgammon-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='seeded-backgammon', KOBO_SIM_SEED='0',
                   KOBO_BACKGAMMON_SEED=SEED)
        (private/'cobalt-sim-state/backgammon').mkdir(parents=True)
        log_path = output/'simulator.log'
        checks = []
        with log_path.open('w') as log:
            def stop():
                nonlocal process
                if process and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                process = None

            def start():
                nonlocal process
                log.seek(0)
                log.truncate()
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'apps/backgammon', env=env, stdout=log,
                                           stderr=log, start_new_session=True)
                deadline = time.monotonic()+120
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError(log_path.read_text()[-3000:])
                    found = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)',
                                      log_path.read_text())
                    if found:
                        return found.group(1)
                    time.sleep(.1)
                raise RuntimeError('Simulator did not start')

            def drive(*steps):
                command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=90)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as answer:
                    return json.load(answer)

            def capture(name):
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                assert not [i for i in diagnostics['issues'] if i['severity'] == 'error'], diagnostics
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'backgammon', metadata
                return layout

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            def board(layout):
                found = [node for node in layout['nodes'] if node['kind'] == 'BackgammonBoard']
                assert found, words(layout)
                return tuple(found[0][key] for key in ('x', 'y', 'width', 'height'))

            def stacks(layout):
                return [str(node['lines'][0]) for node in layout['nodes']
                        if node['kind'].startswith('BackgammonStack') and node['lines']]

            def message(layout):
                return ' '.join(' '.join(map(str, node['lines'])) for node in layout['nodes']
                                if node['kind'] == 'Secondary')

            def point(label):
                # Against the computer the board reads from White's side, so
                # the number beside a point is its index plus one.
                return 'point-' + str(int(label.split()[1]) - 1)

            def pick_up():
                """Taps the player's checkers, farthest first, until one is in hand."""
                for label in sorted((l for l in stacks(get('layout')) if ' white ' in l),
                                    key=lambda l: -int(l.split()[1])):
                    drive('tap-id ' + point(label), 'wait-idle')
                    if message(get('layout')).startswith(('Tap a marked point', 'Tap Off')):
                        return label
                if 'White' in words(get('layout')).split('Bar')[1][:20]:
                    drive('tap-id bar', 'wait-idle')
                    return 'bar'
                raise AssertionError('nothing could be picked up: ' + words(get('layout')))

            def put_down():
                open_points = [l for l in stacks(get('layout')) if l.endswith(', open')]
                drive('tap-id ' + (point(open_points[0]) if open_points else 'off'), 'wait-idle')

            try:
                address = start()
                drive('wait-for-id roll')
                opening = capture('01-before-the-first-roll')
                assert 'Not rolled yet' in words(opening), words(opening)
                assert 'Opening roll' in words(opening), words(opening)
                assert 'Computer move' not in words(opening), words(opening)
                bar = [str(n['lines'][0]) for n in opening['nodes'] if n['kind'].startswith('BarAction')
                       or n['kind'].startswith('BarGlyph')]
                foot = [str(n['lines'][0]) for n in opening['nodes']
                        if n['kind'].startswith('NavDestination')]
                assert sorted(bar) == ['More', 'Rules'], bar
                assert foot == ['Undo', 'Roll', 'Double'], foot
                place = board(opening)
                checks.append('the top bar carries Rules and More, and the foot reads Undo, Roll, Double')

                drive('tap-id roll', 'wait-idle')
                computer = capture('02-computer-opened')
                assert re.search(r'Computer opened with \d and \d and played ', message(computer)), \
                    words(computer)
                assert 'Computer move' not in words(computer), words(computer)
                assert board(computer) == place
                checks.append('the computer plays its opening without a tap: ' + message(computer))

                drive('tap-id roll', 'wait-idle')
                rolled = capture('03-rolled')
                assert message(rolled).startswith('You rolled'), words(rolled)
                assert board(rolled) == place
                checks.append('the player rolls with a tap and the board stays where it was')

                source = pick_up()
                selected = capture('04-checker-in-hand')
                assert any(l.endswith(', one in hand') for l in stacks(selected)) or source == 'bar', \
                    stacks(selected)
                checks.append(f'tapping {source.split(" white")[0]} on the board picks its checker up')

                put_down()
                capture('05-moved')
                for _ in range(8):
                    if 'Computer rolled' in message(get('layout')):
                        break
                    if not message(get('layout')).startswith(('Tap a marked point', 'Tap Off')):
                        pick_up()
                    put_down()
                replied = capture('06-computer-replied')
                assert 'Computer rolled' in message(replied), words(replied)
                assert board(replied) == place
                checks.append('the computer replies as soon as the turn ends: ' + message(replied))

                drive('tap-id game-more', 'tap-id turn-history', 'wait-idle')
                history = capture('07-history')
                record = re.search(r'You [\d/a-z ]+?(?= Computer| You|$)', words(history))
                assert record and 'Computer ' in words(history), words(history)
                drive('tap Back', 'tap-id game-more', 'tap-id match-settings', 'wait-idle')
                match = capture('08-match')
                assert f'Fixture dice, seed {SEED}' in words(match), words(match)
                drive('tap-id close-match', 'wait-idle')
                checks.append(f'the turn history records "{record.group(0).strip()}" and the settings name the seed')

                stop()
                address = start()
                drive('wait-for-id roll')
                reopened = capture('09-reopened')
                assert 'Resumed saved game' in words(reopened), words(reopened)
                assert board(reopened) == place
                drive('tap-id game-more', 'tap-id turn-history', 'wait-idle')
                assert record.group(0).strip() in words(get('layout')), words(get('layout'))
                checks.append('the match and its record survive a restart')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale, 'seed': SEED,
                    'fixture': 'seeded-backgammon', 'checks': checks}, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()
