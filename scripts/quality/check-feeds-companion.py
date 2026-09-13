#!/usr/bin/env python3
"""Validate subscription staging in an isolated simulator shelf, without fetching feeds."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    transcript = []
    with tempfile.TemporaryDirectory(prefix='cobalt-feeds-check-', dir='/tmp') as temp:
        root = Path(temp)
        env = dict(os.environ, TMPDIR=temp)
        source = root/'Sample subscriptions.opml'
        original = b'<opml version="2.0"><body><outline text="Sample feed" xmlUrl="https://example.org/feed.xml"/><outline text="Duplicate" xmlUrl="https://example.org/feed.xml"/><outline xmlUrl="http://example.org/insecure"/></body></opml>'
        source.write_bytes(original)

        def run(*arguments, succeeds=True):
            result = subprocess.run([str(args.cli.resolve()), 'feeds', *map(str, arguments)],
                                    env=env, capture_output=True, text=True, timeout=10)
            transcript.append({'arguments':[str(a).replace(temp, '<fixture>') for a in arguments],
                               'exit_code':result.returncode,
                               'output':(result.stdout+result.stderr).replace(temp, '<fixture>')})
            assert (result.returncode == 0) == succeeds, transcript[-1]
            return result.stdout+result.stderr

        assert '1 feed, 2 skipped' in run('check', source)
        run('push', source, '--sim')
        shelf = root/'cobalt-sim-data/rss'
        dest = shelf/'sample-subscriptions.opml'
        assert dest.read_bytes() == original
        assert source.read_bytes() == original
        run('push', source, '--sim')
        assert dest.read_bytes() == original
        for invalid in [b'<opml><body>', b'x'*(256*1024+1)]:
            source.write_bytes(invalid)
            run('push', source, '--sim', succeeds=False)
            assert dest.read_bytes() == original, 'Invalid import changed the valid list'
        replacement = b'<opml><body><outline text="Second feed" xmlUrl="https://example.org/second.xml"/></body></opml>'
        source.write_bytes(replacement)
        pending = shelf/'.sample-subscriptions.opml.writing'
        pending.write_bytes(b'other operation')
        run('push', source, '--sim', succeeds=False)
        assert pending.read_bytes() == b'other operation'
        assert dest.read_bytes() == original
        pending.unlink()
        run('push', source, '--sim')
        assert dest.read_bytes() == replacement and not pending.exists()
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output/'transcript.json').write_text(json.dumps(transcript, indent=2)+'\n')
    (args.output/'result.json').write_text(json.dumps({'status':'passed','physical_hardware':False,
        'checks':['duplicate and unsupported addresses reported', 'source bytes preserved',
                  'repeat staging preserves bytes', 'malformed and oversized input preserve existing list',
                  'occupied temporary file preserves both files', 'valid replacement published completely'],
        'scope':'OPML staging only; does not assert reader import or offline article downloads'},indent=2)+'\n')


if __name__ == '__main__':
    main()
