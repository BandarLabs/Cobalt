#!/usr/bin/env python3
"""Exercise provider help and refusal paths without credentials or readers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--cli', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
cli = str(args.cli.resolve())
checks = []
with tempfile.TemporaryDirectory(prefix='cobalt-provider-help-') as temp:
    root = Path(temp)
    for command in ('secret', 'trust'):
        for suffix in (['--help'], ['-h'], ['help'], ['set', '--help'],
                       ['list', '--help'], ['remove', '-h']):
            result = subprocess.run([cli, command, *suffix], capture_output=True,
                                    text=True, timeout=5)
            assert result.returncode == 0 and 'usage: kobo ' + command in result.stdout
            assert not result.stderr
            # The target is optional and the help says where it goes without one.
            assert '[--device IP | --reader NAME' in result.stdout, result.stdout
            assert 'the reader kobo setup saved' in result.stdout, result.stdout
            checks.append({'args': [command, *suffix], 'exit': result.returncode})
            if suffix == ['--help']:
                args.output.mkdir(parents=True, exist_ok=True)
                (args.output/(command+'-help.txt')).write_text(result.stdout)
        for suffix in (
            ['set', 'fixture', '--from', str(root/'absent'), '--device', '127.0.0.1', '--volume', str(root/'reader')],
            ['set', 'fixture', '--from', str(root/'absent'), '--volume', str(root/'reader'), '--device', '127.0.0.1'],
            ['list', '--device', '127.0.0.1', '--device', '127.0.0.2'],
            ['list', '--volume', str(root/'reader'), '--from', str(root/'absent')],
            ['set', 'fixture', '--volume', str(root/'reader'), '--from', str(root/'absent'), '--from', str(root/'also-absent')],
        ):
            result = subprocess.run([cli, command, *suffix], capture_output=True,
                                    text=True, timeout=5)
            assert result.returncode != 0
            assert 'Choose one reader' in result.stderr or 'Use --from once' in result.stderr
            assert not list(root.iterdir()), 'refused command changed destination'
            checks.append({'command': command, 'refused': result.stderr.strip()})
        missing = subprocess.run([cli, command], capture_output=True, text=True, timeout=5)
        assert missing.returncode != 0, 'missing arguments must remain an error'
        # No target and nothing saved: the named next step, as a target error.
        unsaved = subprocess.run([cli, command, 'list'], capture_output=True, text=True,
                                 timeout=5, env=dict(os.environ, KOBO_CONFIG_DIR=str(root/'config'),
                                                     KOBO_READER=''))
        assert unsaved.returncode == 3, unsaved
        assert 'No reader saved. Run kobo setup with the reader plugged in, or kobo pair.' \
            in unsaved.stderr, unsaved.stderr
        checks.append({'args': [command, 'list'], 'no_reader_saved_exit': unsaved.returncode})
    source = root/'synthetic-token'
    source.write_text('fixture-only-not-a-live-token')
    source.chmod(0o600)
    reader = root/'reader'
    result = subprocess.run([cli, 'secret', 'set', 'fixture', '--from', str(source),
                             '--volume', str(reader)], capture_output=True, text=True, timeout=5)
    assert result.returncode == 0
    assert source.read_text() not in result.stdout + result.stderr
    installed = reader/'.adds/cobalt/secrets/fixture'
    assert installed.read_text().strip() == source.read_text()
    previous = installed.read_bytes()
    for value in (b'', b'x' * 4097, b'\xff\xfe'):
        source.write_bytes(value)
        result = subprocess.run([cli, 'secret', 'set', 'fixture', '--from', str(source),
                                 '--volume', str(reader)], capture_output=True, text=True, timeout=5)
        assert result.returncode != 0 and installed.read_bytes() == previous
    source.write_text('replacement-fixture-value')
    # A staging file left by an interrupted run is cleared rather than
    # blocking every later set until somebody finds and deletes it by hand.
    partial = installed.with_name('.fixture.writing')
    partial.write_text('left by an interrupted run')
    result = subprocess.run([cli, 'secret', 'set', 'fixture', '--from', str(source),
                             '--volume', str(reader)], capture_output=True, text=True, timeout=5)
    assert result.returncode == 0 and installed.read_text().strip() == source.read_text()
    assert not partial.exists()
    assert installed.stat().st_mode & 0o777 == 0o600
    checks.append({'preserved_previous': ['empty', 'oversized', 'invalid UTF-8'],
                   'stale_staging_file': 'cleared; replacement published with mode 0600'})
    result = subprocess.run([cli, 'secret', 'list', '--volume', str(reader)],
                            capture_output=True, text=True, timeout=5)
    assert result.returncode == 0 and 'fixture' in result.stdout
    assert source.read_text() not in result.stdout + result.stderr
    result = subprocess.run([cli, 'secret', 'remove', 'fixture', '--volume', str(reader)],
                            capture_output=True, text=True, timeout=5)
    assert result.returncode == 0 and not installed.exists()
    checks.append({'synthetic_volume_lifecycle': 'set/list/remove passed without printing value'})
(args.output/'result.json').write_text(json.dumps({'status': 'passed',
    'cli_sha256': hashlib.sha256(Path(cli).read_bytes()).hexdigest(),
    'checks': checks, 'live_services_contacted': False}, indent=2)+'\n')
