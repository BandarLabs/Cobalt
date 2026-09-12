#!/usr/bin/env python3
"""Exercise the CLI against the real standalone importer and an original deck."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--helper', type=Path, required=True)
    parser.add_argument('--fixture-generator', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, KOBO_FLASHCARDS_IMPORT=str(args.helper.resolve()))
    with tempfile.TemporaryDirectory(prefix='cobalt-flashcards-') as directory:
        root = Path(directory)
        package = root/'original study cards.apkg'
        bundle = root/'collection.cobfc'
        mount = root/'reader'
        mount.mkdir()
        subprocess.run([str(args.fixture_generator.resolve()), str(package)], check=True,
                       capture_output=True, timeout=30)
        transcript = []

        def run(*arguments, succeeds=True):
            result = subprocess.run([str(args.cli.resolve()), 'flashcards', *map(str, arguments)],
                                    env=env, capture_output=True, text=True, timeout=60)
            transcript.append({'arguments': [str(a).replace(str(root), '<fixture>') for a in arguments],
                               'exit_code': result.returncode,
                               'output': (result.stdout+result.stderr).replace(str(root), '<fixture>')})
            assert (result.returncode == 0) == succeeds, transcript[-1]
            return result

        run('status')
        run('import', package, '--merge', bundle)
        run('verify', bundle)
        run('stage', bundle, '--kobo-root', mount)
        destination = mount/'.adds/cobalt/data/flashcards/collection.cobfc'
        assert destination.read_bytes() == bundle.read_bytes()
        digest = hashlib.sha256(destination.read_bytes()).hexdigest()
        corrupt = root/'corrupt.cobfc'
        corrupt.write_bytes(b'not a collection')
        run('verify', corrupt, succeeds=False)
        run('stage', corrupt, '--kobo-root', mount, succeeds=False)
        assert hashlib.sha256(destination.read_bytes()).hexdigest() == digest
        (args.output/'transcript.json').write_text(json.dumps(transcript, indent=2)+'\n')
        (args.output/'result.json').write_text(json.dumps({
            'status': 'passed', 'basis': 'real-helper-and-original-three-card-package',
            'physical_hardware': False,
            'checks': ['helper notice', 'import with spaces in path', 'bundle verification',
                       'staged bytes match prepared bundle', 'helper failures reach CLI',
                       'corrupt stage preserves installed collection'],
        }, indent=2)+'\n')


if __name__ == '__main__':
    main()
