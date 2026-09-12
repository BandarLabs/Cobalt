#!/usr/bin/env python3
"""Check Frame planning, album names and repeated imports in a private shelf."""
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import zlib


def png(shade):
    def chunk(kind, data):
        return struct.pack('>I', len(data))+kind+data+struct.pack('>I', zlib.crc32(kind+data))
    return (b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 1, 8, 0, 0, 0, 0))
            +chunk(b'IDAT', zlib.compress(bytes([0, shade, 255-shade])))+chunk(b'IEND', b''))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    transcript = []
    with tempfile.TemporaryDirectory(prefix='cobalt-frame-plan-', dir='/tmp') as temp:
        root = Path(temp)
        env = dict(os.environ, TMPDIR=str(root), KOBO_SIM_PROFILE='clara-bw-391')
        first, second = root/'first.png', root/'second.png'
        first.write_bytes(png(0))
        second.write_bytes(png(80))

        def run(*arguments):
            result = subprocess.run([str(args.cli.resolve()), 'frame', *map(str, arguments)],
                                    env=env, capture_output=True, text=True, timeout=45)
            transcript.append({'arguments': [str(a).replace(temp, '<fixture>') for a in arguments],
                               'output': (result.stdout+result.stderr).replace(temp, '<fixture>'),
                               'exit_code': result.returncode})
            assert result.returncode == 0, transcript[-1]
            return result.stdout

        initial = run('plan', first, '--sim', '--album', 'Summer holiday')
        shelf = root/'cobalt-sim-data/frame'
        assert not shelf.exists(), 'Planning created or changed the shelf'
        assert '1 new' in initial and 'Summer holiday' in initial
        run('push', first, '--sim', '--album', 'Summer holiday')
        assert 'Summer holiday' in run('ls', '--sim')
        before = {p.name: p.read_bytes() for p in shelf.iterdir()}
        repeated = run('plan', first, '--sim', '--album', 'Summer holiday')
        assert '0 new, 1 already present' in repeated
        removal = run('plan', second, '--sim', '--delete', '--album', 'Autumn')
        assert 'Remove: Summer holiday / first.png' in removal
        assert 'Add: Autumn / second.png' in removal
        assert before == {p.name: p.read_bytes() for p in shelf.iterdir()}, 'Plan changed the shelf'
        repeated_push = run('push', first, '--sim', '--album', 'Summer holiday')
        assert '0 new, 1 already present' in repeated_push
        assert before == {p.name: p.read_bytes() for p in shelf.iterdir()}, 'Repeated import changed content'
    (args.output/'transcript.json').write_text(json.dumps(transcript, indent=2)+'\n')
    (args.output/'result.json').write_text(json.dumps({'status': 'passed', 'physical_hardware': False,
        'checks': ['plan does not create shelf', 'album name survives push and list',
                   'repeated import explicitly reuses photo', 'concrete deletion list',
                   'plan leaves existing shelf unchanged', 'repeated push preserves image bytes']}, indent=2)+'\n')


if __name__ == '__main__':
    main()
