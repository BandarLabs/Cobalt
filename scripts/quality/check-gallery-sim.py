#!/usr/bin/env python3
"""Walk every page of the component reference in the simulator, turn the panels
the crowded pages spread onto, and carry one job through from choosing a book
to opening it."""
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

# The five destinations, and the pages inside each of them.
TABS = [
    ('Type', ['Type', 'Quotes', 'Marks', 'Tone']),
    ('Lists', ['Rows', 'Tiles', 'Boards', 'Icons']),
    ('Input', ['Buttons', 'Groups', 'Fields', 'Overlays']),
    ('States', ['Empty', 'Offline', 'Denied', 'Error']),
    ('Panel', ['Folio', 'Ghosting', 'Transfer', 'Requests']),
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
    with tempfile.TemporaryDirectory(prefix='cobalt-gallery-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='component-gallery', KOBO_SIM_SEED='0',
                   KOBO_SIM_OFFLINE='1')
        (private/'cobalt-sim-state/gallery').mkdir(parents=True)
        log_path = output/'simulator.log'
        checks = []
        panels = {}
        with log_path.open('w') as log:
            def stop():
                nonlocal process
                if process and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                process = None

            def start():
                nonlocal process
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'examples/gallery', env=env, stdout=log,
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

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            def capture(name):
                """One screen, with nothing on it drawn off the panel."""
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                errors = [i for i in diagnostics['issues'] if i['severity'] == 'error']
                assert not errors, f'{name}: {errors}'
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'gallery', metadata
                return layout

            def can_reach(layout, action):
                wanted = action_id(action)
                return any(node.get('action') == wanted for node in layout['nodes'])

            try:
                address = start()
                drive('wait-for Type and tone')
                for tab, pages in TABS:
                    drive(f'tap {tab}', 'wait-idle')
                    for page in pages:
                        drive(f'tap {page}', 'wait-idle')
                        name = f'{tab}-{page}'.lower()
                        layout = capture(f'{len(panels):02d}-{name}')
                        # A page the reference spreads over more than one panel
                        # is walked to its end, because the panel nobody turns
                        # to is the panel nobody has ever seen.
                        seen = 1
                        while can_reach(layout, 'panel-next'):
                            before = words(layout)
                            drive('tap-id panel-next', 'wait-idle')
                            layout = capture(f'{len(panels):02d}-{name}-{seen + 1}')
                            if words(layout) == before:
                                break
                            seen += 1
                        panels[name] = seen
                checks.append(f'every page of all five destinations drew without an error, '
                              f'across {sum(panels.values())} panels')

                # The icon sheet is the one page that pages itself, and the
                # last sheet is where a dropped row would show.
                drive('tap Lists', 'tap Icons', 'wait-idle')
                sheets = 1
                while can_reach(get('layout'), 'icons-next'):
                    drive('tap-id icons-next', 'wait-idle')
                    if not capture(f'icons-{sheets + 1}'):
                        break
                    sheets += 1
                    if sheets > 12:
                        break
                assert sheets >= 5, f'only {sheets} sheets of icons'
                checks.append(f'every icon the system draws was seen, {sheets} sheets of them')

                # The worked example, end to end, the way a reader would.
                drive('tap Panel', 'tap Folio', 'wait-idle')
                folio = get('layout')
                if not can_reach(folio, 'folio-store'):
                    drive('tap-id panel-next', 'wait-idle')
                drive('tap App Store', 'wait-idle')
                chosen = capture('flow-1-chosen')
                assert 'Mrs Dalloway' in words(chosen), words(chosen)
                drive('tap Download it', 'wait-idle')
                asked = capture('flow-2-asked')
                assert 'Download Mrs Dalloway?' in words(asked), words(asked)
                drive('tap Download', 'wait-idle')
                arriving = capture('flow-3-arriving')
                assert 'Arriving' in words(arriving), words(arriving)
                drive('tap Let it finish', 'wait-idle')
                arrived = capture('flow-4-arrived')
                assert 'is on this device' in words(arrived), words(arrived)
                drive('tap Open it', 'wait-idle')
                reading = capture('flow-5-reading')
                assert 'Close the book' in words(reading), words(reading)
                checks.append('a book was chosen, confirmed, downloaded and opened')

                # And back out of the job, one step at a time, to the page it
                # was started from.
                drive('tap Close the book', 'wait-idle')
                shelf = capture('flow-6-back-to-the-shelf')
                assert 'Ready' in words(shelf), words(shelf)
                drive('tap back', 'wait-idle')
                chosen_again = capture('flow-7-back-to-the-choice')
                assert 'Nothing has been downloaded yet' in words(chosen_again), words(chosen_again)
                drive('tap back', 'wait-idle')
                out = capture('flow-8-out')
                # Back where it started: the reference, on the panel the job
                # was begun from, rather than one step further back than that.
                assert 'App Store' in words(out), words(out)
                checks.append('every step of the job had a way back out of it')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'component-gallery',
                    'panels': panels, 'icon_sheets': sheets,
                    'checks': checks}, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()
