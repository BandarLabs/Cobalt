#!/usr/bin/env python3
"""Exercise laptop Stop through a real PTY without contacting any reader."""
import argparse
import json
import os
from pathlib import Path
import pty
import select
import socket
import ssl
import urllib.request
import urllib.error
import urllib.parse
import subprocess
import sys
import tempfile
import termios
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--connection-states', action='store_true')
    args = parser.parse_args()
    cli = str(args.cli.resolve())
    with tempfile.TemporaryDirectory(prefix='paperterm-stop-', dir='/tmp') as private:
        env = dict(os.environ, KOBO_STREAM_CONFIG_DIR=private, HOME=private, SHELL="/bin/sh")
        subprocess.run([cli, 'stream', 'init', '--host', '127.0.0.1'], env=env,
                       capture_output=True, check=True, timeout=30)
        for preset, finished in [("demo", False), ("demo", True), ("terminal", False), ("monitor", False)]:
            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0))
                port = probe.getsockname()[1]
            master, slave = pty.openpty()
            def terminal_settings():
                settings = termios.tcgetattr(slave)
                # macOS may set this transient retype-pending flag on restored input.
                # It is not a raw/canonical mode setting.
                settings[3] &= ~getattr(termios, 'PENDIN', 0)
                return settings
            original = terminal_settings()
            process = subprocess.Popen([cli, 'stream', preset, '--port', str(port)], env=env,
                                       stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
            output = bytearray()

            def await_text(text):
                deadline = time.monotonic() + 15
                while text not in output:
                    assert time.monotonic() < deadline, 'Timed out waiting for Paperterm state'
                    assert process.poll() is None, 'Paperterm exited before expected state'
                    if select.select([master], [], [], .1)[0]:
                        output.extend(os.read(master, 65536))
            try:
                await_text(b'Press Ctrl+]')
                if preset == 'demo':
                    await_text(b'Press Enter to send.')
                elif preset == 'terminal':
                    os.write(master, b"printf 'TERMINAL_%s\\n' READY\r")
                    await_text(b'TERMINAL_READY')
                else:
                    await_text(b'Processes:' if sys.platform == 'darwin' else b'top -')
                if args.connection_states and preset == 'demo' and not finished:
                    await_text(b'waiting for a reader')
                    context = ssl.create_default_context(cafile=str(Path(private)/'stream/cert.pem'))
                    token = (Path(private)/'stream/pairing').read_text().strip()
                    def get(path):
                        url = f'https://127.0.0.1:{port}'+path
                        with urllib.request.urlopen(url, context=context, timeout=5) as response:
                            return json.load(response)
                    try:
                        get('/lease?token=incorrect')
                        raise AssertionError('Incorrect token was accepted')
                    except urllib.error.HTTPError as error:
                        assert error.code == 403
                    assert b'reader connected.' not in output
                    query = urllib.parse.urlencode({'token': token})
                    lease = get('/lease?'+query)['lease']
                    hello = '/hello?'+query+f'&lease={lease}&generation=0&grid=80x24'
                    get(hello)
                    await_text(b'reader connected.')
                    output.clear()
                    deadline = time.monotonic() + 50
                    while b'waiting for the reader to reconnect' not in output:
                        assert time.monotonic() < deadline, 'Missing reconnect state'
                        if select.select([master], [], [], .1)[0]:
                            output.extend(os.read(master, 65536))
                    output.clear()
                    get(hello)
                    await_text(b'reader connected.')
                if finished:
                    os.write(master, b'exit\r')
                    await_text(b'Connection check finished.')
                    await_text(b'command stopped. Final screen')
                    deadline = time.monotonic() + 5
                    while terminal_settings() != original:
                        assert time.monotonic() < deadline, 'Terminal was not restored after child exit'
                        time.sleep(.05)
                os.write(master, b'\x1d\n' if finished else b'\x1d')
                deadline = time.monotonic() + 5
                while process.poll() is None:
                    assert time.monotonic() < deadline, f'{preset} did not stop within five seconds'
                    # Keep consuming output like a terminal emulator, including top's redraws.
                    if select.select([master], [], [], .05)[0]:
                        output.extend(os.read(master, 65536))
                assert process.returncode == 0, 'Stop failed'
                assert terminal_settings() == original, 'Stop changed terminal mode settings'
                while select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                assert b'Paperterm stopped.' in output
                with socket.socket() as probe:
                    probe.settimeout(1)
                    assert probe.connect_ex(('127.0.0.1', port)) != 0, 'Sharing port remains open'
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
                os.close(master)
                os.close(slave)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output/'result.json').write_text(json.dumps({'status': 'passed', 'physical_hardware': False, 'connection_states_tested': args.connection_states,
        'checks': ['connection check, login shell and system monitor run in real PTYs',
                   'Ctrl+] stops each preset within five seconds',
                   'Ctrl+] then Enter closes the final-screen service',
                   'terminal settings restored', 'sharing port closed', 'clear stopped message']}, indent=2)+'\n')


if __name__ == '__main__':
    main()
