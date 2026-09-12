#!/usr/bin/env python3
"""Drive the bare CLI in a terminal and check the noninteractive fallback."""
import argparse
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    cli = str(args.cli.resolve())
    plain = subprocess.run([cli], input='', capture_output=True, text=True, timeout=5)
    assert plain.returncode == 0 and 'Developer and release commands: kobo --help' in plain.stdout
    assert 'Choose a number:' not in plain.stdout
    with tempfile.TemporaryDirectory(prefix='cobalt-owner-menu-', dir='/tmp') as temp:
        source = Path(temp)/'Sample feeds.opml'
        source.write_text('<opml version="2.0"><head><title>Sample</title></head><body><outline text="Sample feed" type="rss" xmlUrl="https://example.org/feed.xml"/></body></opml>')
        master, slave = pty.openpty()
        child = subprocess.Popen([cli], stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        transcript = bytearray()
        try:
            deadline = time.monotonic()+10
            while b'Choose a number:' not in transcript:
                assert time.monotonic() < deadline
                if select.select([master], [], [], .1)[0]:
                    transcript.extend(os.read(master, 65536))
            os.write(master, b'3\n')
            while b'OPML file path' not in transcript:
                assert time.monotonic() < deadline
                if select.select([master], [], [], .1)[0]:
                    transcript.extend(os.read(master, 65536))
            os.write(master, (str(source)+'\n').encode())
            while child.poll() is None:
                assert time.monotonic() < deadline
                if select.select([master], [], [], .1)[0]:
                    transcript.extend(os.read(master, 65536))
            while select.select([master], [], [], 0)[0]:
                transcript.extend(os.read(master, 65536))
            assert child.returncode == 0
            text = transcript.decode().replace(temp, '<fixture>')
            assert 'Sample feed' in text and 'https://example.org/feed.xml' in text
        finally:
            if child.poll() is None:
                child.kill(); child.wait(timeout=5)
            os.close(master); os.close(slave)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output/'terminal.txt').write_text(text)
    (args.output/'noninteractive.txt').write_text(plain.stdout)
    (args.output/'result.json').write_text(json.dumps({'status':'passed','checks':[
        'noninteractive help exits without prompting', 'real PTY displays numbered owner choices',
        'menu reads a path containing spaces', 'existing feed checker validates original OPML fixture']},indent=2)+'\n')


if __name__ == '__main__':
    main()
