#!/usr/bin/env python3
"""Check Sidekick help and cancellation without installing agent hooks."""
import argparse
import hashlib
import html
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--helper', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
helper = str(args.helper.resolve())
for suffix in (['--help'], ['-h'], ['help'], ['setup', '--help'], ['init', '-h'],
               ['run', '--help'], ['hook', '--help'], ['agents', '-h']):
    result = subprocess.run([helper, *suffix], capture_output=True, text=True, timeout=5)
    assert result.returncode == 0 and 'usage:' in result.stdout and not result.stderr
missing = subprocess.run([helper], capture_output=True, text=True, timeout=5)
assert missing.returncode != 0
plain = subprocess.run([helper, 'setup'], input='', capture_output=True, text=True, timeout=5)
assert plain.returncode == 0 and 'No configuration was changed.' in plain.stdout
master, slave = pty.openpty()
child = subprocess.Popen([helper, 'setup'], stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
transcript = bytearray()
deadline = time.monotonic() + 10
try:
    while b'Choose a number' not in transcript:
        assert time.monotonic() < deadline
        if select.select([master], [], [], .1)[0]:
            transcript.extend(os.read(master, 65536))
    assert b'Only the selected integration' in transcript
    os.write(master, b'99\n')
    while b'Choose one of the listed numbers.' not in transcript:
        assert time.monotonic() < deadline
        if select.select([master], [], [], .1)[0]:
            transcript.extend(os.read(master, 65536))
    os.write(master, b'0\n')
    assert child.wait(timeout=5) == 0
finally:
    if child.poll() is None:
        child.kill()
        child.wait(timeout=5)
    os.close(master)
    os.close(slave)
args.output.mkdir(parents=True, exist_ok=True)
(args.output/'terminal.txt').write_text(transcript.decode())
(args.output/'noninteractive.txt').write_text(plain.stdout)
(args.output/'result.json').write_text(json.dumps({'status': 'passed',
    'helper_sha256': hashlib.sha256(Path(helper).read_bytes()).hexdigest(),
    'checks': ['eight help invocations exit successfully', 'missing arguments remain an error',
               'redirected setup shows status and leaves configuration unchanged',
               'real terminal displays integrations, retries invalid number and cancels'],
    'hook_installation_tested': False}, indent=2)+'\n')

lines = transcript.decode().splitlines()
svg = '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="%d"><rect width="100%%" height="100%%" fill="#fafafa"/><g font-family="monospace" font-size="14" fill="#171717">' % (len(lines)*24+48)
for index, line in enumerate(lines):
    svg += '<text x="24" y="%d">%s</text>' % (32+index*24, html.escape(line))
svg += '</g></svg>\n'
(args.output/'selection.svg').write_text(svg)
