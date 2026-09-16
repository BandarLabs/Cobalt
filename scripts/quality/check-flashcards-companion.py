#!/usr/bin/env python3
"""Exercise the CLI against the real standalone importer and an original deck."""
import argparse
import base64
import struct
import zlib
import hashlib
import json
import os
import re
import shutil
import signal
import sqlite3
import zipfile
import time
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--helper', type=Path, required=True)
    parser.add_argument('--fixture-generator', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--reader-sim', action='store_true')
    parser.add_argument('--scale', default='100')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, KOBO_FLASHCARDS_IMPORT=str(args.helper.resolve()))
    with tempfile.TemporaryDirectory(prefix='cobalt-fc-', dir='/tmp') as directory:
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
                               'output': (result.stdout+result.stderr).replace(str(root), '<fixture>').replace(str(args.helper.resolve()), '<helper>')})
            assert (result.returncode == 0) == succeeds, transcript[-1]
            return result

        status = run('status')
        assert 'flashcards-import 0.' in status.stdout, 'Helper version missing'
        formats = run('formats')
        assert 'collection.anki21b' in formats.stdout, 'Modern-package boundary missing'
        assert 'does not update Anki scheduling' in formats.stdout, 'Review boundary missing'
        run('import', package, '--merge', bundle)
        run('verify', bundle)
        preview = root/'preview.html'
        run('preview', bundle, '--out', preview)
        preview_bytes = preview.read_bytes()
        assert b'Magnetic north.' in preview_bytes and b'Front' in preview_bytes
        assert b'Card 1 of 3' in preview_bytes
        run('preview', bundle, '--out', preview, succeeds=False)
        assert preview.read_bytes() == preview_bytes, 'Preview replaced an existing file'
        missing_card = root/'invalid-card.html'
        run('preview', bundle, '--out', missing_card, '--card', '0', succeeds=False)
        assert not missing_card.exists()
        shutil.copyfile(preview, args.output/'preview.html')

        run('stage', bundle, '--kobo-root', mount)
        destination = mount/'.adds/cobalt/data/flashcards/collection.cobfc'
        assert destination.read_bytes() == bundle.read_bytes()
        digest = hashlib.sha256(destination.read_bytes()).hexdigest()
        # A second original package with distinct card/note identities exercises real merge.
        second = root/'second deck.apkg'
        database_path = root/'second.anki2'
        with zipfile.ZipFile(package) as archive:
            database_path.write_bytes(archive.read('collection.anki2'))
        with sqlite3.connect(database_path) as database:
            database.execute('UPDATE notes SET id = id + 100000')
            database.execute('UPDATE cards SET id = id + 100000, nid = nid + 100000')
            database.execute('UPDATE revlog SET id = id + 100000, cid = cid + 100000')
            database.execute("UPDATE notes SET flds = replace(flds, 'What does a compass point toward?', 'What does a compass point toward?<img src=\"sample.png\">')")
        with zipfile.ZipFile(second, 'w') as archive:
            archive.write(database_path, 'collection.anki2')
            archive.writestr('media', '{"0":"sample.png"}')
            def chunk(kind, data):
                return struct.pack('>I', len(data))+kind+data+struct.pack('>I', zlib.crc32(kind+data))
            png = b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR', struct.pack('>IIBBBBB', 1, 1, 8, 0, 0, 0, 0))
            png += chunk(b'IDAT', zlib.compress(b'\x00\x80'))+chunk(b'IEND', b'')
            archive.writestr('0', png)
        media_bundle = root/'with-media.cobfc'
        run('import', second, '--merge', media_bundle)
        media_preview = root/'with-media.html'
        run('preview', media_bundle, '--out', media_preview)
        document = media_preview.read_text()
        images = re.findall(r'data:image/png;base64,([^"]+)', document)
        assert images and all(base64.b64decode(image) == png for image in images)
        assert 'sample.png' in document and '1 media files' in document
        merged = root/'merged.cobfc'
        run('import', second, '--merge', merged, '--merge-into', bundle)
        merged_report = run('verify', merged)
        assert '6 due cards' in merged_report.stdout, 'Merge did not preserve both decks of cards'
        assert hashlib.sha256(bundle.read_bytes()).hexdigest() == digest, 'Merge modified its source'
        replacement = root/'replacement.colpkg'
        shutil.copyfile(package, replacement)
        run('import', replacement, '--replace', merged)
        replaced_report = run('verify', merged)
        assert '3 due cards' in replaced_report.stdout, 'Explicit replacement retained unrelated cards'
        bad_package = root/'broken.apkg'
        bad_package.write_bytes(b'not a package')
        before_failure = merged.read_bytes()
        run('import', bad_package, '--merge', merged, succeeds=False)
        assert merged.read_bytes() == before_failure, 'Failed import replaced the prepared collection'
        corrupt = root/'corrupt.cobfc'
        corrupt.write_bytes(b'not a collection')
        run('verify', corrupt, succeeds=False)
        run('stage', corrupt, '--kobo-root', mount, succeeds=False)
        assert hashlib.sha256(destination.read_bytes()).hexdigest() == digest
        if args.reader_sim:
            reader_review(args, root, env, destination)
            log_name = 'cobalt-review-log.ndjson'
            reader_log = root/'cobalt-sim-data/flashcards'/log_name
            assert reader_log.is_file(), 'Reader did not save a review log'
            records = [json.loads(line) for line in reader_log.read_text().splitlines()]
            assert len(records) == 1, 'Expected exactly one saved review'
            assert records[0]['grade'] == 'good', 'Reader saved a different grade'
            assert records[0]['bundle_sha256'] == digest, 'Review belongs to another collection'
            shutil.copyfile(reader_log, destination.parent/log_name)
            exported = root/'reviews.ndjson'
            run('export-review-log', '--kobo-root', mount, exported)
            assert exported.read_bytes() == reader_log.read_bytes(), 'Export changed the review log'
        (args.output/'transcript.json').write_text(json.dumps(transcript, indent=2)+'\n')
        (args.output/'result.json').write_text(json.dumps({
            'status': 'passed', 'basis': 'real-helper-and-original-three-card-package',
            'physical_hardware': False, 'reader_simulator': args.reader_sim,
            'text_scale': args.scale if args.reader_sim else None,
            'checks': ['front/back HTML preview and card count', 'embedded image bytes and media count', 'preview preserves existing files', 'helper version and notice', 'supported formats and review limits', 'import with spaces in path', 'bundle verification',
                       'staged bytes match prepared bundle', 'helper failures reach CLI',
                       'corrupt stage preserves installed collection',
                       'merge preserves both card inventories and source bundle',
                       'explicit replacement replaces card inventory',
                       'failed import preserves prepared collection'] +
                      (['reader opened staged deck', 'revealed and graded a card',
                        'one saved review exported without changes'] if args.reader_sim else []),
        }, indent=2)+'\n')


def reader_review(args, root, env, destination):
    repo = Path(__file__).resolve().parents[2]
    shelf = root/'cobalt-sim-data/flashcards'
    shelf.mkdir(parents=True)
    shutil.copyfile(destination, shelf/'collection.cobfc')
    simulator_env = dict(env, TMPDIR=str(root), KOBO_SIM_OFFLINE='1',
                         KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                         CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='3')
    cli = str(args.cli.resolve())
    with (args.output/'simulator.log').open('w') as log:
        simulator = subprocess.Popen([cli, 'dev', '127.0.0.1:0'], cwd=repo/'apps/flashcards',
                                     env=simulator_env, stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic()+120
            address = None
            while time.monotonic() < deadline:
                assert simulator.poll() is None, 'Simulator exited; see simulator.log'
                match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)',
                                  (args.output/'simulator.log').read_text())
                if match:
                    address = match.group(1)
                    break
                time.sleep(.1)
            assert address, 'Simulator did not start'
            steps = ['wait-for-id deck-0', 'clean', 'shot 01-decks', 'tap-id deck-0',
                     'wait-for-id answer', 'clean', 'shot 02-question', 'tap-id answer',
                     'wait-for-id good', 'clean', 'shot 03-answer', 'tap-id good',
                     'wait 1000', 'wait-for-id answer', 'clean', 'shot 04-next-card']
            command = [cli, 'drive', '--address', address, '--ideal', '--shots', str(args.output.resolve())]
            for step in steps:
                command.extend(['--step', step])
            subprocess.run(command, cwd=repo, env=simulator_env, stdout=log, stderr=log,
                           check=True, timeout=90)
        finally:
            if simulator.poll() is None:
                os.killpg(simulator.pid, signal.SIGTERM)
                try:
                    simulator.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(simulator.pid, signal.SIGKILL)
                    simulator.wait(timeout=5)


if __name__ == '__main__':
    main()
