#!/usr/bin/env python3
"""Play a seeded backgammon turn in the actual simulator: roll, choose a
checker, move it, read the record of the turn, and find it after a restart."""
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

            def checker_controls(layout):
                """The point, bar and off controls offered right now."""
                return [(node['action'], str(node['lines'][0])) for node in layout['nodes']
                        if node['kind'].startswith('CellLabel') and node['lines']
                        and re.fullmatch(r'\d+', str(node['lines'][0]))]

            def tap_first_checker():
                layout = get('layout')
                offered = checker_controls(layout)
                assert offered, f'no checker to tap: {words(layout)}'
                action, label = offered[0]
                drive(f'tap-id {action}', 'wait-idle')
                return label

            try:
                address = start()
                drive('wait-for-id roll')
                opening = capture('01-before-the-first-roll')
                assert 'None rolled' in words(opening), words(opening)
                assert 'to play' in words(opening) or 'Your move' in words(opening), words(opening)
                checks.append('the board says whose move it is and that no dice are down')

                # Two people passing one reader, which is where the record of
                # each turn matters and where nothing is played for anybody.
                drive('tap-id match', 'tap-id mode')
                setup = capture('02-match-setup')
                assert 'Players: Pass and play' in words(setup), words(setup)
                checks.append('the match screen changes who is playing, off the board')
                drive('tap-id close-match', 'tap-id roll')
                rolled = capture('03-rolled')
                dice = re.search(r'Dice (\d)(?: and (\d))?', words(rolled))
                assert dice, words(rolled)
                checks.append(f'a seeded roll of {dice.group(0)[5:]} is stated in words')

                source = tap_first_checker()
                selected = capture('04-checker-in-hand')
                assert 'Tap a legal destination' in words(selected), words(selected)
                checks.append('the checker in hand is marked and asks for a destination')

                destination = tap_first_checker()
                moved = capture('05-moved')
                assert source != destination
                # The turn ends when both dice are used; play on until it does.
                for _ in range(6):
                    if 'Last:' in words(get('layout')):
                        break
                    if not checker_controls(get('layout')):
                        break
                    tap_first_checker()
                finished = capture('06-turn-recorded')
                record = re.search(r'Last: (White|Black) ([\d/a-z ]+)', words(finished))
                assert record, words(finished)
                checks.append(f'the finished turn is written down as "{record.group(0)[6:]}"')

                drive('tap-id match')
                match = capture('07-match')
                assert f'Fixture dice, seed {SEED}' in words(match), words(match)
                assert 'Turn history' in words(match), words(match)
                assert record.group(0)[6:].strip() in words(match), words(match)
                checks.append('the match screen names the seed and lists the turns')

                stop()
                address = start()
                drive('wait-for-id match', 'tap-id match')
                reopened = capture('08-reopened-match')
                assert record.group(0)[6:].strip() in words(reopened), words(reopened)
                checks.append('the record of play survives a restart')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale, 'seed': SEED,
                    'fixture': 'seeded-backgammon', 'checks': checks}, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()
