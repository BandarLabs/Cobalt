#!/usr/bin/env python3
"""Drive owner-initiated text/image exports through SDK storage and the CLI receiver."""
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
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    cli = ROOT / 'target/debug/kobo'
    fixture = ROOT / 'target/debug/examples/export'
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    results = []
    for kind in ('text', 'image'):
        evidence = args.output / kind
        evidence.mkdir()
        with tempfile.TemporaryDirectory(prefix='cobalt-export-', dir='/tmp') as private:
            private = Path(private)
            env = dict(os.environ, TMPDIR=str(private), KOBO_SIM_PROFILE='clara-bw-391',
                       KOBO_TEXT_SCALE='extra-large', KOBO_SIM_SEED='0', KOBO_FIXTURE_REVISION=head)
            env.pop('KOBO_SOCKET', None)
            env.pop('KOBO_EXPORT_IMAGE', None)
            expected = b'Water the mint after sunset.\n'
            if kind == 'image':
                artwork = Image.new('L', (400, 400), 255)
                draw = ImageDraw.Draw(artwork)
                draw.rectangle((45, 45, 355, 355), outline=0, width=7)
                draw.ellipse((120, 90, 280, 250), fill=96)
                draw.line((80, 310, 320, 310), fill=0, width=8)
                path = private / 'original.png'
                artwork.save(path)
                expected = path.read_bytes()
                env['KOBO_EXPORT_IMAGE'] = str(path)
            process = None
            with (evidence / 'simulator.log').open('w') as log:
                try:
                    process = subprocess.Popen([str(fixture), '127.0.0.1:0', str(private/'sdk.sock'),
                                                str(ROOT/'target/debug/kobo-launcher')], cwd=ROOT, env=env,
                                               stdout=log, stderr=log, start_new_session=True)
                    deadline = time.monotonic() + 25
                    while time.monotonic() < deadline:
                        match = re.search(r'Kobo runtime simulator: http://(127\.0\.0\.1:\d+)', (evidence/'simulator.log').read_text())
                        if match:
                            address = match.group(1)
                            break
                        if process.poll() is not None:
                            raise RuntimeError('Export fixture stopped during startup')
                        time.sleep(.05)
                    else:
                        raise RuntimeError('Export fixture startup timed out')
                    def drive(*steps):
                        command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(evidence)]
                        for step in steps:
                            command.extend(['--step', step])
                        subprocess.run(command, env=env, cwd=ROOT, stdout=log, stderr=log, check=True, timeout=30)
                    destination = private/'received'
                    receive = [str(cli), 'export', '--app', 'todo', '--sim', '--out', str(destination)]
                    drive('wait-idle', 'tap-id open-todo', 'wait-idle', 'shot preview')
                    before = subprocess.run(receive, env=env, cwd=ROOT, capture_output=True, timeout=10)
                    assert before.returncode != 0 and not destination.exists(), 'Content was available before owner confirmation'
                    drive('scenario storage-full', 'tap-id export-confirm', 'wait-idle', 'wait-for-id export-retry', 'shot failed')
                    assert not (private/'cobalt-sim-state/todo/cobalt-export').exists()
                    drive('scenario normal', 'tap-id export-retry', 'wait-idle', 'expect Ready for your computer', 'shot ready')
                    record = json.loads((private/'cobalt-sim-state/todo/cobalt-export').read_text())['payload']
                    assert record['sha256'] == hashlib.sha256(expected).hexdigest()
                    subprocess.run(receive, env=env, cwd=ROOT, stdout=log, stderr=log, check=True, timeout=10)
                    files = list(destination.iterdir())
                    assert len(files) == 1 and files[0].read_bytes() == expected
                    subprocess.run(receive, env=env, cwd=ROOT, stdout=log, stderr=log, check=True, timeout=10)
                    assert list(destination.iterdir()) == files, 'Retry made a second identical copy'
                    source = private/'cobalt-sim-data/todo'/record['sha256']
                    source.write_bytes(b'damaged fixture')
                    damaged = subprocess.run(receive, env=env, cwd=ROOT, capture_output=True, timeout=10)
                    assert damaged.returncode != 0 and files[0].read_bytes() == expected
                    with urllib.request.urlopen(f'http://{address}/activity', timeout=5) as response:
                        activity = json.load(response)
                    assert activity['effects']['fetch'] == 0 and activity['effects']['post'] == 0
                    results.append(dict(kind=kind, status='passed', sha256=record['sha256']))
                finally:
                    if process is not None and process.poll() is None:
                        os.killpg(process.pid, signal.SIGTERM)
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait(timeout=5)
    (args.output/'result.json').write_text(json.dumps(dict(status='passed', hardware_validation=False,
        source_head=head, source_dirty=True, original_fixtures=True, checks=results), indent=2)+'\n')


if __name__ == '__main__':
    main()
