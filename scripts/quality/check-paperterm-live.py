#!/usr/bin/env python3
"""Share one real host PTY with a laptop terminal and the actual Paperterm SDK app."""
import argparse
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import socket
import subprocess
import tempfile
import termios
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--profile', default='clara-bw-391')
    parser.add_argument('--pair-on-reader', action='store_true')
    parser.add_argument('--load-failure', action='store_true')
    parser.add_argument('--save-failure', action='store_true')
    parser.add_argument('--temporary-pairing', action='store_true')
    parser.add_argument('--scale', default='extra-large')
    args = parser.parse_args()
    if (args.load_failure or args.save_failure or args.temporary_pairing) and not args.pair_on_reader:
        parser.error('storage checks require --pair-on-reader')
    if args.temporary_pairing and (not args.load_failure or args.save_failure):
        parser.error('--temporary-pairing requires --load-failure and excludes --save-failure')
    args.output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT/'target'))).resolve()
    cli = target/'debug/kobo'
    host = simulator = None
    master = slave = None
    host_bytes = bytearray()
    with tempfile.TemporaryDirectory(prefix='cobalt-paperterm-', dir='/tmp') as private:
        private = Path(private)
        config = private/'config'
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config/'trust'),
                   KOBO_SIM_PROFILE=args.profile, KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='live-paperterm-pty', KOBO_SIM_SEED='0')
        init = subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'],
                              env=env, cwd=ROOT, capture_output=True, text=True, timeout=30)
        assert init.returncode == 0, init.stderr
        assert (config/'stream/cert.pem').is_file(), 'Use a rebuilt CLI with private stream config support'
        with socket.socket() as port_socket:
            port_socket.bind(('127.0.0.1', 0))
            port = port_socket.getsockname()[1]
        fixture = private/'terminal_fixture.py'
        fixture.write_text('''import os, signal, time
def resized(*_):
    size = os.get_terminal_size(1)
    print("SIZE %s %s" % (size.columns, size.lines), flush=True)
signal.signal(signal.SIGWINCH, resized)
print("PTY", os.isatty(0), os.isatty(1), flush=True)
print("READY> ", end="", flush=True)
first = input()
print("FROM READER: " + first, flush=True)
second = input()
print("FROM LAPTOP: " + second, flush=True)
print("WIDE:" + "0123456789" * 12, flush=True)
print("WAITING FOR CTRL-C", flush=True)
try:
    time.sleep(60)
except KeyboardInterrupt:
    print("INTERRUPTED", flush=True)
print("DONE", flush=True)
''')
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            try:
                master, slave = pty.openpty()
                original = termios.tcgetattr(slave)
                host = subprocess.Popen([str(cli), 'stream', '--interactive', '--port', str(port),
                                         '--', '/usr/bin/env', 'python3', '-u', str(fixture)],
                                        cwd=ROOT, env=env, stdin=slave, stdout=slave, stderr=slave,
                                        start_new_session=True)

                def drain_host():
                    while select.select([master], [], [], 0)[0]:
                        try:
                            chunk = os.read(master, 65536)
                        except OSError:
                            break
                        if not chunk:
                            break
                        host_bytes.extend(chunk)
                    return host_bytes.decode('utf-8', errors='replace')

                deadline = time.monotonic()+10
                while 'READY>' not in drain_host() and time.monotonic() < deadline:
                    assert host.poll() is None, drain_host()
                    time.sleep(.05)
                assert 'READY>' in drain_host(), 'Prompt without newline did not reach laptop'
                raw = termios.tcgetattr(slave)
                assert not raw[3] & (termios.ICANON | termios.ECHO), 'Laptop input was not put in raw mode'

                store = private/'cobalt-sim-state/paperterm'
                store.mkdir(parents=True)
                code = (config/'stream/pairing').read_text().strip()
                if args.load_failure:
                    (store/'pairing').mkdir()
                if not args.pair_on_reader:
                    (store/'pairing').write_text(f'127.0.0.1:{port}\n{code}')
                simulator = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'], cwd=ROOT/'apps/paperterm',
                                             env=env, stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic()+120
                address = None
                while time.monotonic() < deadline:
                    assert simulator.poll() is None, log_path.read_text()[-1500:]
                    match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                    if match:
                        address = match.group(1)
                        break
                    time.sleep(.1)
                assert address, 'Simulator did not start'

                def drive(*steps):
                    command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(args.output)]
                    for step in steps:
                        command.extend(['--step', step])
                    subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, check=True, timeout=45)

                def get(endpoint):
                    with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as response:
                        return json.load(response)

                def capture(name):
                    # A live long poll is intentionally outstanding; wait-idle would be wrong here.
                    drive('wait 600', 'shot '+name)
                    metadata = json.loads((args.output/(name+'.json')).read_text())
                    assert metadata['orientation'] == 'portrait', metadata['orientation']
                    diagnostics = get('diagnostics')
                    assert not [i for i in diagnostics['issues'] if i['severity']=='error'], diagnostics
                    (args.output/(name+'.layout.json')).write_text(json.dumps(get('layout'), indent=2)+'\n')

                if args.load_failure:
                    drive('wait-for-id retry-pairing-load')
                    capture('00-read-failed')
                    if args.temporary_pairing:
                        drive('tap-id temporary-pairing')
                    else:
                        (store/'pairing').rmdir()
                        drive('tap-id retry-pairing-load')
                if args.pair_on_reader:
                    drive('wait-for-id setup', 'tap-id setup', 'tap-id trust', 'tap-id start',
                          'tap-id enter-address', 'tap-id kb.layer', f'type 127.0.0.1:{port}', 'tap-id kb.enter')
                    symbols = False
                    for character in code:
                        if character.isdigit() != symbols:
                            drive('tap-id kb.layer')
                            symbols = not symbols
                        drive('type '+character)
                    if args.save_failure:
                        drive('scenario storage-full')
                    drive('tap-id kb.enter')
                drive('wait-for-id toggle-keyboard', 'wait-for READY>')
                if args.save_failure or args.temporary_pairing:
                    drive('wait-for Not saved')
                    assert not (store/'pairing').is_file()
                    drive('tap-id pairing-menu')
                    capture('00-pairing-not-saved')
                    drive('tap-id return-session')
                else:
                    assert (store/'pairing').read_text() == f'127.0.0.1:{port}\n{code}'
                capture('01-connected-terminal')
                drive('tap-id toggle-keyboard', 'wait-for SIZE')
                capture('02-keyboard-and-resize')
                # Fail a real SDK key request before it reaches the fixture.
                drive('scenario network-timeout', 'type x', 'wait-for Queued keys were discarded')
                capture('02a-input-paused')
                assert 'x' not in drain_host().split('READY>', 1)[-1], 'Failed key reached fixture'
                drive('scenario normal', 'wait-for-id resume-input')
                drive('tap-id resume-input', 'wait-for-id kb.r0c0')
                capture('02b-input-resumed')
                drive('scenario offline', 'tap-id toggle-keyboard', 'wait-for Reconnecting')
                capture('02c-reconnecting-keys-closed')
                drive('tap-id toggle-keyboard', 'expect Reconnecting')
                capture('02d-reconnecting-keys-open')
                drive('scenario normal', 'wait-for Connected')
                capture('02e-reconnected')
                drive('type reader', 'tap enter', 'wait-for FROM READER: reader')
                capture('03-reader-to-host')
                if args.save_failure:
                    assert not (store/'pairing').exists(), 'Pairing retried without an explicit request'
                    drive('tap-id pairing-menu', 'tap-id retry-pairing', 'wait-for Pairing is saved')
                    capture('03a-pairing-saved')
                    assert (store/'pairing').read_text() == f'127.0.0.1:{port}\n{code}'
                    drive('tap-id return-session', 'wait-for Connected')
                if args.temporary_pairing:
                    assert (store/'pairing').is_dir(), 'Temporary connection changed saved storage'
                assert 'FROM READER: reader' in drain_host(), 'Reader input did not reach the host PTY'
                os.write(master, b'laptop')
                drive('wait-for laptop')
                assert 'FROM LAPTOP: laptop' not in drain_host(), 'Test sent Enter too early'
                capture('04-laptop-before-enter')
                os.write(master, b'\r')
                drive('wait-for FROM LAPTOP: laptop', 'wait-for WAITING FOR CTRL-C')
                capture('05-shared-session-and-wide-output')
                drive('tap-id term.ctrl', 'type c', 'wait-for INTERRUPTED', 'wait-for DONE')
                capture('06-control-c-and-final-screen')
                assert 'FROM LAPTOP: laptop' in drain_host() and 'INTERRUPTED' in drain_host()
                deadline = time.monotonic()+5
                while termios.tcgetattr(slave) != original and time.monotonic() < deadline:
                    time.sleep(.05)
                assert termios.tcgetattr(slave) == original, 'Laptop terminal settings were not restored'
                grids = [list(map(int, match)) for match in re.findall(r'SIZE (\d+) (\d+)', drain_host())]
                assert grids and grids[0][0] == grids[-1][0], grids
                assert grids[0][1] >= grids[-1][1], grids
                # A large panel can reach the 64-row limit with either keyboard state.
                assert len(grids) >= 2 or grids[0][1] == 64, grids
                result = dict(status='passed', profile=args.profile, scale=args.scale,
                              orientation='portrait', negotiated_grids=grids,
                              storage_checks=dict(load_failure=args.load_failure, save_failure=args.save_failure, temporary=args.temporary_pairing),
                              source_head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                              source_dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)),
                              basis='real-host-pty-and-sdk-simulator-over-trusted-tls', pairing='entered through on-screen keyboard' if args.pair_on_reader else 'private seeded fixture',
                              checks=['reconnect retains output and keyboard toggle', 'connection restored', 'failed input pauses without replay', 'explicit input resume', 'portrait captures', 'measured grid on keyboard toggle', 'prompt without newline', 'laptop raw mode', 'trusted TLS', 'reader input to host',
                                      'laptop input before Enter', 'same-session output', 'PTY grid negotiation', 'wide output',
                                      'reader Ctrl-C', 'retained final screen', 'laptop terminal restoration'])
                if args.load_failure:
                    result['checks'].append('failed pairing read has an explicit recovery path')
                if args.save_failure:
                    result['checks'].append('failed pairing save retries only on request')
                if args.temporary_pairing:
                    result['checks'].append('temporary connection leaves pairing storage untouched')
                (args.output/'result.json').write_text(json.dumps(result, indent=2)+'\n')
            finally:
                for process in [simulator, host]:
                    if process is not None and process.poll() is None:
                        os.killpg(process.pid, signal.SIGTERM)
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait(timeout=5)
                if slave is not None:
                    os.close(slave)
                if master is not None:
                    os.close(master)
                # Only synthetic terminal output; pairing credentials are not included.
                (args.output/'host-terminal.log').write_bytes(host_bytes)


if __name__ == '__main__':
    main()
