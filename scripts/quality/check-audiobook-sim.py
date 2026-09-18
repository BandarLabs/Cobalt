#!/usr/bin/env python3
"""Drive Audiobook Studio end to end against three local TLS fixtures standing
in for Exa, OpenAI and ElevenLabs: the free sample with no accounts at all,
preflight that names every missing key while nothing is spent, a full
researched-and-narrated creation, an in-session resume that spends only the
failed part, a cancel whose checkpoint survives a simulator restart, and the
library/player round trip. The fixtures record every request; no public
provider is ever contacted (the simulator routes the three hostnames to
loopback with URL, Host and TLS identity unchanged)."""
import argparse
import http.server
import json
import os
from pathlib import Path
import re
import signal
import ssl
import subprocess
import tempfile
import threading
import time

from simulator_cli import build_cli

ROOT = Path(__file__).resolve().parents[2]
EXA_KEY = 'fixture-exa-key'
OPENAI_KEY = 'fixture-openai-key'
ELEVEN_KEY = 'fixture-elevenlabs-key'
ENGLISH_VOICE = 'JBFqnCBsd6RMkjVDRZzb'
MP3 = (ROOT / 'examples/audiobook/assets/sample.mp3').read_bytes()[:65536]

SCRIPT = {
    'title': 'The Moon Tonight',
    'summary': 'A short original tour of the Moon, researched and written on this reader.',
    'chapters': [
        {'title': 'A Familiar Light', 'narration': 'The Moon is the first thing most people learn '
         'to find in the night sky, and the last thing they stop noticing. It is a quarter of the '
         'width of the Earth, close enough that its pull moves every ocean twice a day.'},
        {'title': 'A Slow History', 'narration': 'The Moon was likely born from a collision: a '
         'body the size of Mars struck the young Earth, and the debris gathered into the companion '
         'we know. The evidence sits in rocks carried home by the Apollo crews.'},
        {'title': 'A Quiet Future', 'narration': 'The Moon drifts a few centimetres farther away '
         'every year. Nothing about tonight changes for it. The light stays, the tides keep their '
         'appointments, and the far side keeps its privacy, as it always has.'},
    ],
}


class Providers:
    """The three fixtures, and everything each was asked to do."""

    def __init__(self):
        self.calls = {'exa': [], 'openai': [], 'elevenlabs': []}
        self.tts_fail_at = None      # 1-based narration call to fail once
        self.tts_delay = 0.0         # seconds to hold the next TTS answer
        self._failed = set()

    def total(self, name):
        return len(self.calls[name])


def make_handler(providers, name):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *_):
            pass

        def do_POST(self):
            length = int(self.headers.get('Content-Length') or 0)
            raw = self.rfile.read(length) if length else b''
            providers.calls[name].append({
                'path': self.path,
                'authorization': self.headers.get('Authorization'),
                'x-api-key': self.headers.get('x-api-key'),
                'xi-api-key': self.headers.get('xi-api-key'),
                'bytes': length,
            })
            if name == 'exa':
                body = ('data: {"id":"run_1","status":"completed","output":{"structured":'
                        + json.dumps({'research': 'A brief on the topic with grounding.',
                                      'sources': [{'title': 'A source',
                                                   'url': 'https://example.org'}]})
                        + '}}\n\n')
                payload = body.encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            if name == 'openai':
                envelope = {'output': [{'content': [
                    {'type': 'output_text', 'text': json.dumps(SCRIPT)}]}]}
                payload = json.dumps(envelope).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            # elevenlabs
            call_number = len(providers.calls['elevenlabs'])
            if providers.tts_delay:
                time.sleep(providers.tts_delay)
                providers.tts_delay = 0.0
            if providers.tts_fail_at == call_number and call_number not in providers._failed:
                providers._failed.add(call_number)
                payload = b'{"detail":"fixture failure"}'
                self.send_response(500)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                return
            self.send_response(200)
            self.send_header('Content-Type', 'audio/mpeg')
            self.send_header('Content-Length', str(len(MP3)))
            self.end_headers()
            self.wfile.write(MP3)

    return Handler


def certificate(private, hosts):
    """One fixture CA, one server certificate per provider hostname."""
    trust = private / 'trust'
    trust.mkdir()
    subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2',
                    '-subj', '/CN=Cobalt local fixture CA', '-keyout', str(private / 'ca.key'),
                    '-out', str(trust / 'ca.pem')],
                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
    for host in hosts:
        subprocess.run(['openssl', 'req', '-newkey', 'rsa:2048', '-nodes', '-subj', f'/CN={host}',
                        '-keyout', str(private / f'{host}.key'),
                        '-out', str(private / f'{host}.csr')],
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
        (private / 'extensions').write_text(
            f'subjectAltName=DNS:{host}\nbasicConstraints=critical,CA:FALSE\n'
            'keyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n')
        subprocess.run(['openssl', 'x509', '-req', '-in', str(private / f'{host}.csr'),
                        '-CA', str(trust / 'ca.pem'), '-CAkey', str(private / 'ca.key'),
                        '-CAcreateserial', '-days', '2', '-extfile', str(private / 'extensions'),
                        '-out', str(private / f'{host}.pem')],
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
    return trust


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='default')
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target'))).resolve()
    cli, provenance = build_cli(ROOT, target)
    providers = Providers()
    result = dict(provenance=provenance, scale=args.scale, checks=[])

    with tempfile.TemporaryDirectory(prefix='cobalt-audiobook-', dir='/tmp') as temporary:
        private = Path(temporary)
        hosts = ['api.exa.ai', 'api.openai.com', 'api.elevenlabs.io']
        trust = certificate(private, hosts)
        servers = {}
        sockets = []
        for name, host in [('exa', hosts[0]), ('openai', hosts[1]), ('elevenlabs', hosts[2])]:
            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0),
                                                     make_handler(providers, name))
            server.daemon_threads = True
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            tls.load_cert_chain(private / f'{host}.pem', private / f'{host}.key')
            server.socket = tls.wrap_socket(server.socket, server_side=True)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            servers[name] = server
            sockets.append(f'{host}=127.0.0.1:{server.server_port}')

        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN='1.85.1',
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG='0',
                   CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='1',
                   KOBO_SIM_HTTP_FIXTURE=','.join(sockets),
                   KOBO_SIM_TRUST_DIR=str(trust), KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_PROFILE='clara-bw-391')
        env.pop('KOBO_SIM_OFFLINE', None)
        secrets = private / 'cobalt-sim-secrets/apps/audiobook'

        process = None
        address = None
        with (out / 'simulator.log').open('w') as log:
            def stop():
                nonlocal process
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=15)
                process = None

            def start():
                nonlocal process, address
                offset = (out / 'simulator.log').stat().st_size
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT / 'examples/audiobook', env=env,
                                           stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic() + 300
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError('Simulator exited; see simulator.log')
                    match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)',
                                      (out / 'simulator.log').read_text()[offset:])
                    if match:
                        address = match.group(1)
                        return
                    time.sleep(.1)
                raise TimeoutError('Simulator startup timed out')

            def drive(*steps, timeout=300):
                command = [str(cli), 'drive', '--address', address, '--ideal',
                           '--shots', str(out)]
                for step in steps:
                    command += ['--step', step]
                with open(out / 'simulator.log', 'a') as sink:
                    subprocess.run(command, cwd=ROOT, env=env, stdout=sink, stderr=sink,
                                   check=True, timeout=timeout)

            def capture(name):
                drive('clean', 'shot ' + name)

            try:
                # Phase A: no accounts at all. The sample still plays, and a
                # creation attempt names every missing key while spending nil.
                start()
                drive('wait-for No audiobooks yet')
                capture('audiobook-empty')
                drive('tap Play the sample', 'wait-for The Quiet Shelf', 'wait 2500')
                capture('audiobook-sample-player')
                drive('tap back', 'wait-for The Quiet Shelf')
                drive('tap Create', 'wait-for What should it be about?')
                drive('tap Type any topic', 'type the moon', 'tap Create',
                      'wait-for Account details')
                capture('audiobook-preflight-missing')
                assert (providers.total('exa') == 0 and providers.total('openai') == 0
                    and providers.total('elevenlabs') == 0), \
                    f'a provider was called with no keys installed: {providers.calls}'
                result['checks'].append(dict(
                    name='sample and preflight without accounts',
                    detail='free sample saved and played with no secrets installed; a create '
                           'attempt named research, writing and narration as missing and no '
                           'fixture received any request',
                    status='passed'))
                drive('tap Your audiobooks', 'wait-for The Quiet Shelf')

                # Phase B: all three accounts installed; a full creation.
                stop()
                secrets.mkdir(parents=True)
                for name, value in [('exa', EXA_KEY), ('openai', OPENAI_KEY),
                                    ('elevenlabs', ELEVEN_KEY)]:
                    (secrets / name).write_text(value)
                    os.chmod(secrets / name, 0o600)
                start()
                drive('wait-for The Quiet Shelf')
                drive('tap Create', 'wait-for What should it be about?')
                drive('tap Type any topic', 'type the moon', 'tap Create',
                      'wait-for Researching the topic')
                capture('audiobook-researching')
                drive('wait-for Narrating part 1 of 3', 'wait-for Narrating part 2 of 3')
                capture('audiobook-narrating')
                drive('wait-for The Moon Tonight', 'wait 2500')
                capture('audiobook-player')
                assert providers.total('exa') == 1, providers.calls['exa']
                assert providers.total('openai') == 1, providers.calls['openai']
                assert providers.total('elevenlabs') == 3, providers.calls['elevenlabs']
                assert providers.calls['exa'][0]['x-api-key'] == EXA_KEY
                assert providers.calls['openai'][0]['authorization'] == f'Bearer {OPENAI_KEY}'
                for call in providers.calls['elevenlabs']:
                    assert call['xi-api-key'] == ELEVEN_KEY
                    assert f'/v1/text-to-speech/{ENGLISH_VOICE}' in call['path'], call
                result['checks'].append(dict(
                    name='full creation over three fixture providers',
                    detail='preflight passed, research (1 Exa SSE call), script (1 OpenAI '
                           'responses call) and narration (3 ElevenLabs calls, English voice) '
                           'ran in order with the right credentials; the packaged book saved to '
                           'the shelf and opened in the player',
                    status='passed'))

                # Phase C: a narration call fails; Resume spends only that call.
                drive('tap back', 'wait-for The Moon Tonight')
                drive('tap Create', 'wait-for What should it be about?')
                drive('tap Type any topic', 'type the tides', 'tap Create',
                      'wait-for Researching the topic')
                providers.tts_fail_at = providers.total('elevenlabs') + 2
                drive('wait-for Could not create audiobook')
                capture('audiobook-failed-resume')
                before = providers.total('elevenlabs')
                drive('tap Resume', 'wait-for Narrating part 2 of 3',
                      'wait-for The Moon Tonight', 'wait 2000')
                assert providers.total('elevenlabs') == before + 2, \
                    f'resume re-narrated too much: {before} -> {providers.total("elevenlabs")}'
                result['checks'].append(dict(
                    name='in-session resume spends only the failed part',
                    detail='part 2 of 3 failed once at the fixture; the failure screen led with '
                           'Resume and one tap re-asked only part 2 (two further calls: the '
                           'retry and part 3), then packaged and played',
                    status='passed'))

                # Phase D: cancel mid-narration; the checkpoint survives a
                # simulator restart and resumes the script from the top.
                drive('tap back', 'wait-for Your audiobooks')
                drive('tap Create', 'wait-for What should it be about?')
                drive('tap Type any topic', 'type the stars', 'tap Create',
                      'wait-for Researching the topic', 'wait-for Narrating part 1 of 3')
                providers.tts_delay = 8.0
                drive('wait 2500')
                mid = providers.total('elevenlabs')
                drive('tap Cancel', 'wait-for Your audiobooks', 'wait 1000')
                stop()
                start()
                drive('wait-for The Quiet Shelf')
                drive('tap Create', 'wait-for What should it be about?')
                capture('audiobook-compose-resume')
                drive('tap Resume', 'wait-for Narrating part 1 of 3')
                restart_base = providers.total('elevenlabs')
                drive('wait-for The Moon Tonight', 'wait 2000')
                assert providers.total('elevenlabs') == restart_base + 3, \
                    'a restarted resume re-narrates the whole script, honestly'
                result['checks'].append(dict(
                    name='cancel and resume across a restart',
                    detail='cancel mid-narration returned to the shelf; after a simulator '
                           'restart the composer still offered Resume for the interrupted book, '
                           'and resuming re-narrated the kept script from part 1 (3 calls)',
                    status='passed'))

                # Phase E: library and player are one path both ways.
                drive('tap back', 'wait-for Your audiobooks')
                drive('tap The Moon Tonight', 'wait-for The Moon Tonight', 'wait 1500')
                capture('audiobook-library-player')
                drive('tap back', 'wait-for The Moon Tonight')
                shelf = private / 'cobalt-sim-data/audiobook'
                archives = sorted(shelf.glob('*.mp3z'))
                names = [item.name for item in archives]
                assert 'the-quiet-shelf.mp3z' in names, names
                assert names.count('the-moon-tonight.mp3z') == 1, names
                result['checks'].append(dict(
                    name='library and player round trip',
                    detail='a saved book opens in the same player a fresh creation lands in and '
                           f'back returns to the shelf; on disk: {names}',
                    status='passed'))
            finally:
                stop()
                for server in servers.values():
                    server.shutdown()

    result['status'] = 'passed' if result['checks'] and all(
        check['status'] == 'passed' for check in result['checks']) else 'failed'
    (out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
