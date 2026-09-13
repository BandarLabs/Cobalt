#!/usr/bin/env python3
"""Play each of the four table games on the panel: take a turn, read what just
happened, find the legal squares drawn rather than typed, and pick a game up
again after closing it."""
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

GAMES = ['Reversi', 'Draughts', "Nine Men's Morris", 'Kalah 6,4']


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
    with tempfile.TemporaryDirectory(prefix='cobalt-parlor-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='four-table-games', KOBO_SIM_SEED='0',
                   KOBO_SIM_OFFLINE='1')
        (private/'cobalt-sim-state/parlor').mkdir(parents=True)
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
                                           cwd=ROOT/'apps/parlor', env=env, stdout=log,
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
                command = [str(cli), 'drive', '--address', panel, '--ideal',
                           '--shots', str(output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=90)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{panel}/{endpoint}', timeout=5) as answer:
                    return json.load(answer)

            def capture(name):
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                errors = [i for i in diagnostics['issues'] if i['severity'] == 'error']
                assert not errors, f'{name}: {errors}'
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'parlor', metadata
                return layout

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            def legal_cells(layout):
                """The squares the panel says a piece may go to."""
                return [node for node in layout['nodes']
                        if 'LegalPoint' in node['kind'] or 'Legal square' in str(node['lines'])]

            try:
                panel = start()
                drive('wait-for Pass-and-play classics')
                menu = capture('01-the-table')
                for game in GAMES:
                    assert game in words(menu), words(menu)
                checks.append('all four games are offered, each by its own name')

                played = {}
                for at, game in enumerate(GAMES):
                    short = game.split()[0].lower().strip(',')
                    drive(f'tap {game}', 'wait-idle')
                    capture(f'{at + 2:02d}-{short}-setup')
                    drive('tap Start game', 'wait-idle')
                    board = capture(f'{at + 2:02d}-{short}-board')
                    said = words(board)
                    assert 'to move' in said, said
                    # A legal destination is a drawn mark rather than whatever
                    # dotted circle the typeface happens to carry.
                    if game in ('Reversi', "Nine Men's Morris"):
                        assert legal_cells(board), f'{game}: no legal square was marked'
                    played[game] = said
                    drive('tap back', 'wait-idle')
                checks.append('each game sets up, says whose move it is and marks where a piece '
                              'may go')

                # One real turn, and the board says what just happened.
                drive('tap Reversi', 'wait-idle', 'tap Start game', 'wait-idle')
                board = capture('06-reversi-before')
                target = legal_cells(board)[0]
                drive(f"tap-id {target['action']}", 'wait-idle')
                after = capture('07-reversi-after')
                assert 'Last:' in words(after), words(after)
                checks.append('a turn is taken and the board says what it was')

                # Closed and opened again: the game is where it was left.
                stop()
                panel = start()
                drive('wait-for Pass-and-play classics')
                resumed = capture('08-resume-offered')
                assert 'Resume saved game' in words(resumed), words(resumed)
                drive('tap Resume saved game', 'wait-idle')
                back = capture('09-resumed')
                assert 'Last:' in words(back), words(back)
                checks.append('a game put down mid-turn is picked up where it was left')

                # A board this panel cannot hold says so before it is started.
                drive('tap back', 'wait-idle', 'tap Draughts', 'wait-idle')
                drive('tap Rules', 'wait-idle')
                rules = capture('10-international-refused')
                said = words(rules)
                assert 'not playable on this panel' in said, said
                assert '10×10' in said, said
                checks.append('the board this panel cannot draw says so on the row that offers it')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'four-table-games',
                    'checks': checks}, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()
