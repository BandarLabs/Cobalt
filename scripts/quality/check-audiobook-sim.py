#!/usr/bin/env python3
"""Drive Audiobook Studio end to end against three local TLS fixtures standing
in for Exa, OpenAI and ElevenLabs: the free sample with no accounts at all,
a keys screen that names the exact command for every missing key while
nothing is spent, keys installed with the real `kobo secret set` and picked
up by Check again, the time and cost estimate before the first paid request,
a full researched-and-narrated creation that leaves no narrated parts behind,
an in-session resume that spends only the failed part, a cancel whose saved
parts survive a simulator restart so Resume pays only for the rest, an
offline failure that keeps the topic, and the library/player round trip.
The fixtures record every request; no public provider is ever contacted (the
simulator routes the three hostnames to loopback with URL, Host and TLS
identity unchanged)."""
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
        self.tts_delay_at = None     # 1-based narration call to hold instead
        self.exa_delay = 0.0         # seconds to hold the next research answer
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
                if providers.exa_delay:
                    time.sleep(providers.exa_delay)
                    providers.exa_delay = 0.0
                body = ('event: agent_run.created\n'
                        'data: {"id":"run_1","status":"queued"}\n\n'
                        'event: agent_run.completed\n'
                        'data: {"id":"run_1","status":"completed","output":{"structured":'
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
            if providers.tts_delay_at == call_number:
                providers.tts_delay_at = None
                time.sleep(providers.tts_delay)
                providers.tts_delay = 0.0
            elif providers.tts_delay and providers.tts_delay_at is None:
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
    parser.add_argument('--profile', default='clara-bw-391')
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target'))).resolve()
    cli, provenance = build_cli(ROOT, target)
    providers = Providers()
    result = dict(provenance=provenance, scale=args.scale, profile=args.profile, checks=[])

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
                   KOBO_SIM_PROFILE=args.profile)
        env.pop('KOBO_SIM_OFFLINE', None)
        # The setup steps say `kobo secret set NAME`, which reads the key from
        # a file and writes the owner-wide copy. That command has no --sim, so
        # its --volume target is pointed at the simulator's secret directory,
        # and KOBO_SECRETS_DIR at fake key files rather than the owner's own.
        keys = private / 'keys'
        keys.mkdir()
        for name, value in [('exa', EXA_KEY), ('openai', OPENAI_KEY),
                            ('elevenlabs', ELEVEN_KEY)]:
            (keys / name).write_text(value + '\n')
        env['KOBO_SECRETS_DIR'] = str(keys)
        volume = private / 'volume'
        (volume / '.adds' / 'cobalt').mkdir(parents=True)
        (private / 'cobalt-sim-secrets').mkdir()
        os.symlink(private / 'cobalt-sim-secrets', volume / '.adds' / 'cobalt' / 'secrets')
        shelf = private / 'cobalt-sim-data/audiobook'

        def secret(verb, name):
            with open(out / 'simulator.log', 'a') as sink:
                subprocess.run([str(cli), 'secret', verb, name, '--volume', str(volume)],
                               cwd=ROOT, env=env, stdout=sink, stderr=sink, check=True, timeout=60)

        def parts_on_shelf():
            return sorted(path.name for path in shelf.glob('*.part-*'))

        process = None
        address = None
        seen_addresses = set()
        (out / 'simulator.log').write_text('')
        if True:
            def stop():
                nonlocal process
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=15)
                process = None

            def start():
                nonlocal process, address
                sink = open(out / 'simulator.log', 'a')
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT / 'examples/audiobook', env=env,
                                           stdout=sink, stderr=sink, start_new_session=True)
                deadline = time.monotonic() + 300
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError('Simulator exited; see simulator.log')
                    for match in re.finditer(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)',
                                             (out / 'simulator.log').read_text()):
                        if match.group(1) not in seen_addresses:
                            address = match.group(1)
                            seen_addresses.add(address)
                            sink.close()
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
                # creation attempt names the command for every missing key,
                # keeps the topic, and spends nothing.
                start()
                drive('wait-for No audiobooks yet')
                capture('audiobook-empty')
                drive('tap-id sample', 'wait-for The Quiet Shelf', 'wait 2500')
                capture('audiobook-sample-player')
                drive('tap back', 'wait-for The Quiet Shelf')
                drive('tap Create', 'wait-for Your topic')
                drive('tap Type any topic', 'type the moon', 'tap Create',
                      'wait-for Add your keys', 'expect kobo secret set exa',
                      'expect kobo secret set openai', 'expect kobo secret set elevenlabs',
                      'expect Nothing has been spent', 'expect-missing Try another topic')
                capture('audiobook-keys-missing')
                assert (providers.total('exa') == 0 and providers.total('openai') == 0
                    and providers.total('elevenlabs') == 0), \
                    f'a provider was called with no keys installed: {providers.calls}'
                result['checks'].append(dict(
                    name='sample and missing keys without accounts',
                    detail='free sample saved and played with no secrets installed; a create '
                           'attempt named kobo secret set exa, openai and elevenlabs, offered '
                           'Check again, and no fixture received any request',
                    status='passed'))

                # Phase B: install the keys with the real command while the
                # simulator runs. Check again names only what is still
                # missing, then shows the estimate; nothing is spent.
                secret('set', 'exa')
                secret('set', 'openai')
                drive('tap Check again', 'wait-for Add your keys',
                      'expect kobo secret set elevenlabs',
                      'expect-missing kobo secret set exa')
                capture('audiobook-keys-one-missing')
                secret('set', 'elevenlabs')
                drive('tap Check again', 'wait-for Before you start', 'expect 3 to 8 minutes',
                      'expect $1 to $5', 'expect 7 to 11 thousand ElevenLabs characters',
                      'expect An estimate')
                capture('audiobook-estimate')
                drive('tap Edit topic', 'wait-for Your topic', 'expect the moon',
                      'tap-id kb.enter', 'wait-for Before you start')
                assert (providers.total('exa') == 0 and providers.total('openai') == 0
                    and providers.total('elevenlabs') == 0), providers.calls
                result['checks'].append(dict(
                    name='keys installed with kobo secret set, then the estimate',
                    detail='with two of three keys installed by the CLI, Check again named only '
                           'the narration key; with all three it showed time, cost and '
                           'narration characters before any request; Edit topic kept the topic',
                    status='passed'))

                # Phase C: a full creation from the estimate.
                providers.exa_delay = 5.0
                drive('tap-id start', 'wait-for Researching the topic')
                capture('audiobook-researching')
                providers.tts_delay = 5.0
                drive('wait-for Narrating part 1 of 3')
                capture('audiobook-narrating')
                drive('wait-for Now playing', 'wait 2000')
                capture('audiobook-player')
                assert providers.total('exa') == 1, providers.calls['exa']
                assert providers.total('openai') == 1, providers.calls['openai']
                assert providers.total('elevenlabs') == 3, providers.calls['elevenlabs']
                assert providers.calls['exa'][0]['x-api-key'] == EXA_KEY
                assert providers.calls['openai'][0]['authorization'] == f'Bearer {OPENAI_KEY}'
                for call in providers.calls['elevenlabs']:
                    assert call['xi-api-key'] == ELEVEN_KEY
                    assert f'/v1/text-to-speech/{ENGLISH_VOICE}' in call['path'], call
                assert parts_on_shelf() == [], parts_on_shelf()
                result['checks'].append(dict(
                    name='full creation over three fixture providers',
                    detail='research (1 Exa SSE call), script (1 OpenAI responses call) and '
                           'narration (3 ElevenLabs calls, English voice) ran in order with the '
                           'keys the CLI installed; the packaged book saved to the shelf, opened '
                           'in the player, and no narrated part was left behind',
                    status='passed'))

                # Phase D: a narration call fails; Resume spends only that call.
                drive('tap back', 'wait-for The Moon Tonight')
                drive('tap Create', 'wait-for Your topic')
                providers.tts_fail_at = providers.total('elevenlabs') + 2
                drive('tap Type any topic', 'type the tides', 'tap Create',
                      'wait-for Before you start', 'tap-id start',
                      'wait-for Could not create audiobook',
                      'expect Resume carries on from part 2 of 3')
                capture('audiobook-failed-resume')
                before = providers.total('elevenlabs')
                drive('tap-id resume', 'wait-for Now playing', 'wait 1500')
                assert providers.total('elevenlabs') == before + 2, \
                    f'resume re-narrated too much: {before} -> {providers.total("elevenlabs")}'
                result['checks'].append(dict(
                    name='in-session resume spends only the failed part',
                    detail='part 2 of 3 failed once at the fixture; the failure screen said '
                           'where Resume carries on, and one tap re-asked only part 2 (two '
                           'further calls: the retry and part 3), then packaged and played',
                    status='passed'))

                # Phase E: cancel once part 1 is saved; after a simulator
                # restart, Resume reads part 1 back and pays for 2 and 3 only.
                drive('tap back', 'wait-for Audiobooks')
                drive('tap Create', 'wait-for Your topic')
                providers.tts_delay_at = providers.total('elevenlabs') + 2
                providers.tts_delay = 8.0
                drive('tap Type any topic', 'type the stars', 'tap Create',
                      'wait-for Before you start', 'tap-id start',
                      'wait-for Narrating part 2 of 3', 'wait 1500')
                drive('tap Cancel', 'wait-for Audiobooks', 'wait 1500')
                saved = parts_on_shelf()
                assert saved == ['the-moon-tonight.part-001'], saved
                stop()
                time.sleep(9)
                start()
                drive('wait-for The Quiet Shelf')
                drive('tap Create', 'wait-for Your topic', 'expect Resume part 2 of 3')
                capture('audiobook-compose-resume')
                restart_base = providers.total('elevenlabs')
                drive('tap-id resume', 'wait-for Now playing', 'wait 1500')
                spent = providers.total('elevenlabs') - restart_base
                assert spent == 2, f'a restarted resume narrated {spent} parts; wanted 2'
                assert parts_on_shelf() == [], parts_on_shelf()
                result['checks'].append(dict(
                    name='cancel and resume across a restart without paying twice',
                    detail='part 1 was written to the shelf as it arrived; after a cancel and a '
                           'simulator restart the composer offered Resume part 2 of 3, and '
                           'resuming read part 1 back and narrated parts 2 and 3 only (2 calls); '
                           'the parts were removed once the book was saved',
                    status='passed'))

                # Phase F: offline. The failure says so and Try again keeps
                # the topic rather than asking for a new one.
                drive('tap back', 'wait-for Audiobooks', 'scenario offline',
                      'tap Create', 'wait-for Your topic', 'tap Type any topic', 'type the sea',
                      'tap Create', 'wait-for Before you start', 'tap-id start',
                      'wait-for Could not create audiobook', 'expect not on Wi-Fi',
                      'expect Try again', 'expect-missing Try another topic')
                capture('audiobook-offline')
                drive('scenario normal', 'tap-id check', 'wait-for Before you start')
                result['checks'].append(dict(
                    name='offline failure keeps the topic',
                    detail='with the simulator offline the research request failed with the '
                           'Wi-Fi cause; Try again went back through the key check to the '
                           'estimate with the same topic',
                    status='passed'))

                # Phase G: a key removed with the CLI is named again, alone.
                secret('remove', 'elevenlabs')
                drive('tap Edit topic', 'wait-for Your topic', 'tap-id kb.enter',
                      'wait-for Add your keys', 'expect kobo secret set elevenlabs',
                      'expect-missing kobo secret set exa')
                capture('audiobook-keys-narration-missing')
                result['checks'].append(dict(
                    name='a removed key is named with its command',
                    detail='kobo secret remove elevenlabs, then Create named only the narration '
                           'key and its command',
                    status='passed'))

                # Phase H: library and player are one path both ways.
                drive('tap Edit topic', 'wait-for Your topic', 'tap back', 'wait-for Audiobooks')
                drive('tap The Moon Tonight', 'wait-for Now playing', 'wait 1500')
                capture('audiobook-library-player')
                drive('tap back', 'wait-for The Moon Tonight')
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
