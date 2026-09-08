#!/usr/bin/env python3
"""Drive Panels against original local fixtures in isolated simulator storage."""
import argparse
import io
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import urllib.request
import zipfile

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]


def comic_bytes():
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as volume:
        for number in (10, 2, 1):
            page = Image.new('L', (800, 1100), 255)
            draw = ImageDraw.Draw(page)
            # Original geometric panels, with no external artwork or story text.
            font = ImageFont.load_default(size=40)
            draw.text((50, 35), f'Fixture page {number}', fill=0, font=font)
            for index, box in enumerate(((45, 120, 755, 410), (45, 435, 385, 1030), (410, 435, 755, 1030))):
                draw.rectangle(box, outline=0, width=5)
                draw.ellipse((box[0]+40, box[1]+45, box[0]+170, box[1]+175), fill=64 + index*64)
            encoded = io.BytesIO()
            page.save(encoded, format='PNG')
            volume.writestr(f'{number}.png', encoded.getvalue())
    return archive.getvalue()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='default')
    parser.add_argument('--profile', default='clara-bw-391')
    parser.add_argument('--reader-tools', action='store_true', help='Exercise shared controls, RTL, spreads and process restart')
    parser.add_argument('--cbr', action='store_true', help='Check explicit CBR refusal')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binary = ROOT / 'target/debug/kobo'
    if not binary.is_file():
        parser.error('Build kobo-cli before running this check.')
    process = None
    with tempfile.TemporaryDirectory(prefix='cq-comic-', dir='/tmp') as private:
        env = dict(os.environ, TMPDIR=private, CARGO_TARGET_DIR=str(ROOT/'target'),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE=args.profile, KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE="original-geometric-comic", KOBO_SIM_SEED="0",
                   KOBO_SIM_CLOCK_MILLIS="1788850860000", KOBO_SIM_UTC_OFFSET_MINUTES="0")
        storage = Path(private)/'cobalt-sim-data/panels'
        storage.mkdir(parents=True)
        (storage/'volume.cbz').write_bytes(b'Rar!\x1a\x07\x01\x00' if args.cbr else comic_bytes())
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            try:
                def start():
                    nonlocal process
                    log.seek(0)
                    log.truncate()
                    process = subprocess.Popen([str(binary), 'dev', '127.0.0.1:0'], cwd=ROOT/'apps/panels',
                                               env=env, stdout=log, stderr=log, start_new_session=True)
                    deadline = time.monotonic() + 120
                    address = None
                    while time.monotonic() < deadline:
                        if process.poll() is not None:
                            raise RuntimeError('Simulator exited: ' + log_path.read_text()[-2000:])
                        match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                        if match:
                            address = match.group(1)
                            break
                        time.sleep(.1)
                    if address is None:
                        raise RuntimeError('Simulator startup timed out')

                    return address

                address = start()

                def get(endpoint):
                    with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as response:
                        return response.read()

                def drive(step):
                    subprocess.run([str(binary), 'drive', '--address', address, '--shots', str(args.output/'cli-shots'), '--step', step], cwd=ROOT,
                                   env=env, check=True, timeout=20, stdout=log, stderr=log)

                def wait_for(text):
                    deadline = time.monotonic() + 30
                    while time.monotonic() < deadline:
                        if text in get('layout').decode():
                            return
                        if process.poll() is not None:
                            raise RuntimeError('Simulator stopped while waiting for ' + text)
                        time.sleep(.1)
                    raise RuntimeError('Timed out waiting for ' + text + ': ' + get('layout').decode()[-1800:])

                def capture(label):
                    # Await application responses, then check frame reads themselves do not mutate history.
                    drive('wait-idle')
                    drive('expect-state /activity#/effects/post 0')
                    drive('expect-state /activity#/effects/fetch 0')
                    before = json.loads(get('simulation'))
                    for _ in range(8):
                        get('frame')
                        raw = get('ideal-frame')
                    after = json.loads(get('simulation'))
                    assert before == after, 'Screenshot requests changed simulation state'
                    layout = json.loads(get('layout'))
                    diagnostics = json.loads(get('diagnostics'))
                    width, height = before['profile']['width'], before['profile']['height']
                    Image.frombytes('L', (width, height), raw).save(args.output/(label+'.png'))
                    drive('shot '+label)
                    with Image.open(args.output/'cli-shots'/(label+'.png')) as cli_image:
                        assert cli_image.size == (width, height), 'CLI screenshot used different panel dimensions'
                    provenance = json.loads((args.output/'cli-shots'/(label+'.json')).read_text())
                    assert provenance['app'] == 'panels' and provenance['mode'] == 'single-app'
                    assert provenance['source']['fixture'] == 'original-geometric-comic'
                    assert len(provenance['source']['binarySha256']) == 64
                    assert provenance['fonts'], 'Capture has no installed font provenance'
                    assert provenance['simulation']['profile'] == after['profile']
                    data = dict(simulation=after, layout=layout, diagnostics=diagnostics)
                    (args.output/(label+'.json')).write_text(json.dumps(data, indent=2)+'\n')
                    errors = [issue for issue in diagnostics['issues'] if issue['severity'] == 'error']
                    assert not errors, f'{label}: {errors}'
                    return json.dumps(layout)

                wait_for('Open added comic')
                drive('wait-for-id load-sideload')
                drive('wait-idle')
                drive('expect-state /clock#/mode "manual"')
                drive('clock advance 60000')
                drive('expect-state /clock#/monotonicMillis "60000"')
                drive('clock set 1788850920000 330')
                drive('expect-state /clock#/utcOffsetMinutes 330')
                drive('device battery 18 charging')
                drive('expect-state /device#/batteryPercent 18')
                drive('expect-state /device#/charging true')
                drive('device frontlight 39')
                drive('expect-state /device#/frontlightPercent 39')
                drive('device cover closed')
                drive('expect-state /device#/coverClosed true')
                drive('device cover open')
                drive('device battery 72 unplugged')
                drive('panel hold')
                held_frame = get('frame')
                drive('clock advance 60000')
                drive('expect-state /panel#/status "busy"')
                drive('expect-state /panel#/contentsKnown false')
                for _ in range(8):
                    assert get('frame') == held_frame, 'Pending refresh changed visible pixels'
                drive('device battery 44 charging')
                drive('expect-state /panel#/queued true')
                drive('panel complete')
                drive('expect-state /panel#/status "busy"')
                drive('panel fail')
                drive('expect-state /panel#/status "failed"')
                capture('00-panel-refresh-failed')
                request = urllib.request.Request(f'http://{address}/touch', data=b'x=500&y=500', method='POST')
                try:
                    urllib.request.urlopen(request, timeout=5)
                    raise AssertionError('An uncertain panel accepted a tap')
                except urllib.error.HTTPError as error:
                    assert error.code == 409
                drive('panel retry')
                drive('expect-state /panel#/contentsKnown false')
                drive('panel complete')
                drive('expect-state /panel#/status "idle"')
                drive('expect-state /panel#/contentsKnown true')
                drive('panel auto')
                drive('device battery 72 unplugged')

                drive('expect-state /clock#/monotonicMillis "120000"')
                drive('input touch 0 3 0;0 0 0')
                drive('expect-state /input#/needsResynchronization true')
                drive('expect-state /input#/quiescent false')
                drive('input resync unknown')
                drive('input touch 0 0 0')
                drive('input resync released')
                drive('expect-state /input#/quiescent true')
                capture('01-library')
                drive('tap-id load-sideload')
                wait_for('CBR is not supported yet' if args.cbr else 'Page 1 of 3')
                text = capture('02-opened')
                if args.cbr:
                    assert 'CBR is not supported yet' in text, 'Missing CBR explanation'
                    assert 'CBZ copy' in text, 'Missing recovery action'
                else:
                    assert 'Page 1 of 3' in text, 'Comic did not open'
                    drive('input gpio 4 3 24')
                    drive('input gpio 1 194 1')
                    wait_for('Page 2 of 3')
                    drive('input gpio 1 194 0')
                    assert 'Page 2 of 3' in capture('02-page-key-next'), 'Key release turned another page'
                    drive('input gpio 1 193 1')
                    drive('input gpio 1 193 0')
                    wait_for('Page 1 of 3')

                    profile = json.loads(get('simulation'))['profile']
                    x, y = profile['width']*9//10, profile['height']//2
                    drive(f'tap-at {x},{y}')
                    wait_for('Page 2 of 3')
                    assert 'Page 2 of 3' in capture('03-next'), 'Page turn failed'
                    drive(f'tap-at {profile["width"]//10},{y}')
                    wait_for('Page 1 of 3')
                    assert 'Page 1 of 3' in capture('04-previous'), 'Previous page failed'
                    if args.reader_tools:
                        drive('scenario storage-full')
                        drive(f'tap-at {x},{y}')
                        wait_for('Position not saved')
                        capture('15-position-save-failed')
                        drive('tap-id comic-controls')
                        drive('scenario normal')
                        drive('tap-id comic-save-retry')
                        wait_for('Read')
                        drive('tap-id comic-read')
                        wait_for('Page 2 of 3')
                        assert 'Position not saved' not in capture('16-position-retry-saved')
                        drive(f'tap-at {profile["width"]//10},{y}')
                        wait_for('Page 1 of 3')
                        drive('tap-id comic-controls')
                        capture('05-reading-controls')
                        drive('tap-id comic-jump')
                        capture('06-page-keypad')
                        drive('tap 2')
                        drive('tap Go')
                        wait_for('Page 2 of 3')
                        drive('tap-id comic-controls')
                        drive('tap-id comic-zoom-in')
                        drive('tap-id comic-pan')
                        drive('tap-id comic-down')
                        capture('07-pan')
                        drive('tap-id comic-read')
                        drive('tap-id comic-controls')
                        drive('tap-id comic-pages')
                        capture('08-page-previews')
                        drive('tap-id comic-read')
                        drive('tap-id comic-controls')
                        drive('tap-id comic-options')
                        drive('tap-id comic-direction')
                        capture('09-reading-options')
                        drive('tap-id comic-read')
                        drive(f'tap-at {profile["width"]//10},{y}')
                        wait_for('Page 3 of 3')
                        drive(f'tap-at {x},{y}')
                        wait_for('Page 2 of 3')
                        drive('tap-id comic-controls')
                        drive('tap-id comic-options')
                        drive('tap-id comic-spreads')
                        drive('tap-id comic-rotate')
                        wait_for('Pages 2')
                        capture('10-rtl-spread')
                        drive('tap-id comic-controls')
                        drive('tap-id comic-options')
                        drive('tap-id comic-rotate')
                        wait_for('Page 2 of 3')
                        capture('11-before-restart')
                        os.killpg(process.pid, signal.SIGTERM)
                        process.wait(timeout=5)
                        address = start()
                        wait_for('Open added comic')
                        drive('tap-id load-sideload')
                        wait_for('Page 2 of 3')
                        capture('12-reopened')
                        drive('tap-id comic-controls')
                        assert '125%' in capture('13-restored-zoom')
                        drive('tap-id comic-options')
                        restored = capture('14-restored-direction')
                        assert 'Right to left' in restored and 'Two-page spreads' in restored
                        drive('tap-id comic-read')
                    back = next(node for node in json.loads(get('layout'))['nodes'] if node['kind'] == 'Back')
                    drive(f'tap-at {back["centre"]["x"]},{back["centre"]["y"]}')
                    wait_for('Open added comic')
                    assert 'On this reader' in capture('05-library-return') or 'Open added comic' in get('layout').decode(), 'Back did not return to library'
                (args.output/'result.json').write_text(json.dumps(dict(status='passed', profile=args.profile,
                    scale=args.scale, cbr=args.cbr, reader_tools=args.reader_tools, original_fixture=True), indent=2)+'\n')
            finally:
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)


if __name__ == '__main__':
    main()
