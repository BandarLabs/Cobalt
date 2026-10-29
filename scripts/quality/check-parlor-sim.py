#!/usr/bin/env python3
"""Play each of the four table games on the panel against the computer: take a
turn, see the reply marked and named, take both back, find Rules and the menu
where every game keeps them, pick a game up again after closing it, and play
a turn between two people."""
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

            def marked(layout):
                """The board squares drawn in their chosen state."""
                return [node for node in layout['nodes']
                        if node['kind'].startswith('Cell(') and node['kind'].endswith('true)')]

            try:
                panel = start()
                drive('wait-for Board games')
                menu = capture('01-the-table')
                said = words(menu)
                for game in GAMES:
                    assert game in said, said
                assert 'Play the computer' in said, said
                checks.append('all four games are offered, each by its own name')

                for at, game in enumerate(GAMES):
                    short = game.split()[0].lower().strip(',')
                    drive(f'tap {game}', 'wait-idle')
                    setup = capture(f'{at + 2:02d}-{short}-setup')
                    said = words(setup)
                    # The computer is the opponent a new reader starts with,
                    # and the two removed dead ends stay removed.
                    assert 'Computer strength' in said, said
                    for gone in ('How to play', 'International', '10×10', 'house', 'South'):
                        assert gone not in said, f'{game}: {gone}: {said}'
                    drive('tap Start game', 'wait-idle')
                    board = capture(f'{at + 2:02d}-{short}-board')
                    said = words(board)
                    assert 'Your move' in said, said
                    assert 'Rules' in said and 'Undo' in said, said
                    # A legal destination is a drawn mark rather than whatever
                    # dotted circle the typeface happens to carry.
                    if game in ('Reversi', "Nine Men's Morris"):
                        assert legal_cells(board), f'{game}: no legal square was marked'
                    drive('tap Back', 'wait-idle')
                checks.append('each game sets up against the computer, says whose move it is, '
                              'and marks where a piece may go')

                # One real turn: the computer answers without being asked, and
                # its move is marked on the board and named under the status.
                drive('tap Reversi', 'wait-idle', 'tap Start game', 'wait-idle')
                board = capture('06-reversi-before')
                assert not marked(board), 'a mark before anybody moved'
                target = legal_cells(board)[0]
                drive(f"tap-id {target['action']}", 'wait-idle')
                after = capture('07-reversi-after')
                said = words(after)
                assert 'The computer placed the marked disc' in said, said
                assert 'Your move' in said, said
                assert len(marked(after)) == 1, marked(after)
                checks.append('the computer replies at once and its move is marked')

                drive('tap Undo', 'wait-idle')
                undone = capture('08-undone')
                said = words(undone)
                assert "Your last move and the computer's reply are taken back." in said, said
                assert not marked(undone), marked(undone)
                drive(f"tap-id {target['action']}", 'wait-idle')
                checks.append('Undo takes back the move and the reply together')

                drive('tap-id game-more', 'wait-idle')
                more = capture('09-more')
                said = words(more)
                assert 'New game' in said and 'Change game' in said, said
                # A tap beside the open menu closes it and leaves the game alone.
                drive('tap-at 300,900', 'wait-idle', 'wait-for Your move', 'tap Rules',
                      'wait-idle')
                rules = capture('10-rules')
                assert 'Bracketed discs flip' in words(rules), words(rules)
                drive('tap Back', 'wait-idle', 'wait-for Your move')
                checks.append('Rules and the menu sit in the top bar, and Back returns to play')

                # Closed and opened again: the game is where it was left, and
                # the computer's last move is still marked.
                stop()
                panel = start()
                drive('wait-for Board games')
                resumed = capture('11-resume-offered')
                assert 'Resume saved game' in words(resumed), words(resumed)
                drive('tap Resume saved game', 'wait-idle')
                back = capture('12-resumed')
                said = words(back)
                assert 'The computer placed the marked disc' in said, said
                assert len(marked(back)) == 1, marked(back)
                checks.append('a game put down is picked up where it was left, mark and all')

                # Between two people: one tap away, the other player's move is
                # marked too, and Undo waits for the other player to agree.
                drive('tap-id game-more', 'wait-idle', 'tap New game', 'wait-idle',
                      'tap Pass-and-play', 'wait-idle', 'tap Start game', 'wait-idle')
                board = capture('13-pass-and-play')
                assert 'Black to move' in words(board), words(board)
                drive(f"tap-id {legal_cells(board)[0]['action']}", 'wait-idle')
                passed = capture('14-pass-and-play-after-black')
                said = words(passed)
                assert 'Board turned. Pass the reader to White.' in said, said
                assert 'Black placed the marked disc' in said, said
                drive('tap Undo', 'wait-idle', 'wait-for The other player taps Undo to agree',
                      'tap Undo', 'wait-idle', 'wait-for The last move is taken back.')
                checks.append('pass-and-play is one tap away and its undo needs both players')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'four-table-games',
                    'checks': checks}, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()
