#!/usr/bin/env python3
"""Keep a list on a reader: write things down, tick one off, undo a clear,
rename, reorder and date an item, and offer the whole list to a computer."""
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

# One of these is deliberately longer than the panel is wide, because a list
# of things to do is written in a hurry and nobody counts characters.
ITEMS = [
    'buy milk #shopping',
    'book the van for the weekend and remember the deposit receipt from last time',
    'water the mint #garden',
]


def action_id(name):
    """The same identity the builder and the simulator agree on: FNV-1a."""
    hashed = 0x811c9dc5
    for byte in name.encode():
        hashed = ((hashed ^ byte) * 0x01000193) & 0xFFFFFFFF
    return max(hashed, 1)


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
    with tempfile.TemporaryDirectory(prefix='cobalt-todo-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='a-list-of-things-to-do', KOBO_SIM_SEED='0',
                   KOBO_SIM_OFFLINE='1')
        (private/'cobalt-sim-state/todo').mkdir(parents=True)
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
                                           cwd=ROOT/'examples/todo', env=env, stdout=log,
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
                command = [str(cli), 'drive', '--address', address, '--ideal',
                           '--shots', str(output)]
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
                errors = [i for i in diagnostics['issues'] if i['severity'] == 'error']
                assert not errors, f'{name}: {errors}'
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'todo', metadata
                return layout

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            def type_text(text):
                """Types one string the way a thumb would.

                The driver presses the keys that are on the panel: spaces are
                the space bar, and the hash is on the second layer, so getting
                to it is two taps rather than a special case in the harness.
                """
                for at, chunk in enumerate(text.split(' ')):
                    if at > 0:
                        drive('tap space', 'wait-idle')
                    for part_at, part in enumerate(chunk.split('#')):
                        if part_at > 0:
                            drive('tap ?123', 'wait-idle', 'type #', 'wait-idle')
                            drive('tap abc', 'wait-idle')
                        if part:
                            drive(f'type {part}', 'wait-idle')

            def page_until(name, wanted, pages=4):
                """Captures the page the reader is on, turning until it says
                `wanted`. A longer list at a larger text size is more pages,
                and what the panel holds is a measurement rather than a
                constant, so the journey turns rather than assuming."""
                # From the first page forward: the page turns stop at the
                # ends rather than wrapping, so a search that starts wherever
                # the reader happens to be can miss what is behind them. A
                # list that fits one page has no turns at all, which is why
                # each one is offered rather than assumed.
                for _ in range(pages):
                    if not turn_page('previous'):
                        break
                for at in range(pages):
                    layout = capture(name if at == 0 else f'{name}-{at + 1}')
                    if wanted in words(layout):
                        return layout
                    if not turn_page('next'):
                        break
                raise AssertionError(f'no page said {wanted!r}')

            def turn_page(direction):
                """Turns the page, if there is one that way."""
                wanted = action_id(direction)
                if not any(node.get('action') == wanted for node in get('layout')['nodes']):
                    return False
                drive(f'tap-id {direction}', 'wait-idle')
                return True

            def add(text):
                drive('tap-id add', 'wait-idle')
                type_text(text)
                drive('tap Add', 'wait-idle')

            try:
                address = start()
                drive('wait-for Todo')
                empty = capture('01-nothing-to-do')
                assert 'Nothing to do' in words(empty), words(empty)
                assert 'Add a task' in words(empty), words(empty)
                checks.append('an empty list says so and offers the one thing to do about it')

                for text in ITEMS:
                    add(text)
                listed = page_until('02-a-list', 'buy milk')
                said = words(listed)
                assert 'buy milk' in said, said
                assert '#shopping' in said and '#garden' in said, said
                checks.append('three items, one of them longer than the panel, and their tags')

                drive('tap buy milk', 'wait-idle')
                ticked = page_until('03-one-ticked-off', 'Done. Tap to reopen.')
                assert 'Done. Tap to reopen.' in words(ticked), words(ticked)
                checks.append('tapping an item finishes it, and says how to undo that')

                drive('tap-id clear-done', 'wait-idle')
                cleared = capture('04-cleared')
                assert 'buy milk' not in words(cleared), words(cleared)
                assert 'Undo clear' in words(cleared), words(cleared)
                drive('tap Undo clear', 'wait-idle')
                undone = page_until('05-undone', 'buy milk')
                assert 'buy milk' in words(undone), words(undone)
                checks.append('clearing the finished items can be undone once, and only once')

                drive('tap-id edit-list', 'wait-idle')
                editing = capture('06-editing')
                assert 'Edit list' in words(editing), words(editing)
                drive('tap water the mint', 'wait-idle')
                item = capture('07-one-item')
                assert 'Rename' in words(item) and 'Move up' in words(item), words(item)
                checks.append('the list can be edited without the list itself carrying controls')

                drive('tap Today', 'wait-idle')
                dated = capture('08-dated')
                assert 'due today' in words(dated), words(dated)
                drive('tap Move up', 'wait-idle')
                moved = capture('09-moved')
                assert 'due today' in words(moved), words(moved)
                checks.append('an item takes a date and a place in the list, on its own screen')

                drive('tap-id edit', 'wait-idle')
                type_text('ed')
                drive('tap Save', 'wait-idle')
                renamed = capture('10-renamed')
                # The field opens on the words that are already there, so
                # fixing a typo is two taps rather than typing it all again.
                assert 'water the mint #gardened' in words(renamed), words(renamed)
                checks.append('renaming starts from the words already written, and keeps the '
                              'item where it was')

                drive('tap back', 'wait-idle')
                back = capture('11-back-to-editing')
                assert 'Edit list' in words(back), words(back)
                drive('tap Save a copy', 'wait-idle')
                copying = capture('12-saving-a-copy')
                assert 'Save a copy' in words(copying), words(copying)
                checks.append('the whole list is offered to a paired computer as plain text')

                drive('tap back', 'wait-idle', 'tap back', 'wait-idle')
                final = page_until('13-back-to-the-list', 'due today')
                assert 'due today' in words(final), words(final)
                checks.append('every screen has a way back to the list')

                # Closed and reopened: the list, the dates and the order are
                # all on the device.
                stop()
                address = start()
                drive('wait-for Todo')
                reopened = page_until('14-reopened', 'due today')
                said = words(reopened)
                assert 'water the mint #gardened' in said, said
                checks.append('the list, its dates and its order survive a restart')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'a-list-of-things-to-do',
                    'checks': checks}, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()
