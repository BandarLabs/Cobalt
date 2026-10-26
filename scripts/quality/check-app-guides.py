#!/usr/bin/env python3
"""Check offline setup guides and the actual numbered terminal entry point."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--cli', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
cli = str(args.cli.resolve())
args.output.mkdir(parents=True, exist_ok=True)

def run(*arguments):
    return subprocess.run([cli, *arguments], capture_output=True, text=True, timeout=10)

listing = run('apps')
assert listing.returncode == 0
ids = re.findall(r'^.+ \(([a-z0-9-]+)\)$', listing.stdout, re.M)
assert 'lichess' in ids and 'frame' in ids and len(ids) > 10
for app_id in ids:
    result = run('apps', 'setup', app_id)
    assert result.returncode == 0 and 'Setup:' in result.stdout, app_id
    assert 'does not check installation or account access' in result.stdout
    if app_id in ('lichess', 'frame'):
        (args.output / (app_id + '.txt')).write_text(result.stdout)
search = run('apps', 'search', 'CHESS')
assert search.returncode == 0 and 'lichess' in search.stdout
assert run('apps', 'setup', 'missing-app-fixture').returncode != 0
master, slave = pty.openpty()
child = subprocess.Popen([cli], stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
transcript = bytearray()
deadline = time.monotonic() + 15

def until(marker):
    while marker not in transcript:
        assert time.monotonic() < deadline, transcript.decode(errors='replace')
        if select.select([master], [], [], .1)[0]:
            transcript.extend(os.read(master, 65536))

try:
    until(b'Choose a number:')
    os.write(master, b'6\n')
    until(b'App number (blank cancels):')
    match = re.search(rb'(\d+)\. Lichess\r?\n', transcript)
    assert match
    os.write(master, b'0\n')
    until(b'Choose an app number from the list.')
    os.write(master, match.group(1) + b'\n')
    until(b'This guide does not check installation or account access.')
    assert child.wait(timeout=5) == 0
    assert b'board:play' in transcript and b'kobo secret set lichess' in transcript
finally:
    if child.poll() is None:
        child.kill()
        child.wait(timeout=5)
    os.close(master)
    os.close(slave)
(args.output/'terminal.txt').write_text(transcript.decode())
(args.output/'catalog.txt').write_text(listing.stdout)
(args.output/'result.json').write_text(json.dumps({
    'status': 'passed', 'guides_checked': len(ids),
    'cli_sha256': hashlib.sha256(Path(cli).read_bytes()).hexdigest(),
    'checks': ['every bundled app renders a setup guide', 'case-insensitive purpose search',
               'unknown app fails', 'real terminal menu retries invalid app number',
               'terminal selection opens Lichess guide with account scope and command'],
}, indent=2)+'\n')
