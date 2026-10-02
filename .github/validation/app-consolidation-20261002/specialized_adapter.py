#!/usr/bin/env python3
"""Validation-only fixture initialization and live text diagnostics; no source edits."""
import hashlib
import json
import os
from pathlib import Path
import runpy
import ssl
import subprocess
import sys
import tempfile
import re
import urllib.request

UNION_SHA = '218b5172262767be2fd25265d2b35f7f9337b8c6'
ALLOWED_HELPERS = {
    'check-post-sim.py', 'check-deck-sim.py', 'check-inkling-sim.py',
    'check-musicstand-shelf-sim.py', 'check-audiobook-sim.py', 'check-fieldbook-sim.py',
}
FIXTURE_HELPERS = {'check-post-sim.py', 'check-deck-sim.py'}
RUST_FIXTURE = '''fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args != ["--host", "127.0.0.1"] {
        eprintln!("validation fixture accepts only --host 127.0.0.1");
        std::process::exit(2);
    }
    if let Err(error) = kobo_stream::init(&args) {
        eprintln!("fixture identity generation failed: {error}");
        std::process::exit(1);
    }
}
'''


def build_fixture(root, target, dest, env, original_run):
    # Ask Cargo which exact production rlib it built. Do not guess among stale
    # artifacts or resolve an independent fixture dependency graph.
    built = original_run(['cargo', '+1.85.1', 'build', '--locked', '-p', 'kobo-stream',
                          '--message-format=json'], cwd=root, env=env, check=True,
                         capture_output=True, text=True, timeout=600)
    libraries, native_paths = [], set()
    for line in built.stdout.splitlines():
        value = json.loads(line)
        if value.get('reason') == 'compiler-artifact' and value.get('target', {}).get('name') == 'kobo_stream':
            libraries += [Path(p) for p in value['filenames'] if p.endswith('.rlib')]
        if value.get('reason') == 'build-script-executed':
            native_paths.update(value.get('linked_paths', []))
    if len(libraries) != 1:
        raise RuntimeError(f'Cargo returned {len(libraries)} production kobo_stream rlibs; refusing to guess')
    source, binary = dest / 'stream_fixture.rs', dest / 'stream_fixture'
    source.write_text(RUST_FIXTURE)
    command = ['rustc', '+1.85.1', '--edition=2021', '--crate-name', 'validation_stream_fixture',
               '-C', 'debuginfo=0', '--extern', 'kobo_stream=' + str(libraries[0]),
               '-L', 'dependency=' + str(target / 'debug/deps')]
    for path in sorted(native_paths):
        command.extend(['-L', path])
    command += [str(source), '-o', str(binary)]
    original_run(command, cwd=dest, env=env, check=True, capture_output=True, text=True, timeout=120)
    return binary


def validate_fixture(env, original_run):
    private = Path(env['TMPDIR']).resolve()
    config = Path(env['KOBO_STREAM_CONFIG_DIR']).resolve()
    if not config.is_relative_to(private):
        raise RuntimeError('Fixture configuration is outside this helper temporary directory')
    stream = config / 'stream'
    required = ['ca-cert.pem', 'key.pem', 'cert.pem', 'pairing', 'hosts']
    if any(not (stream / name).is_file() for name in required):
        raise RuntimeError('Production fixture initializer did not create its complete identity')
    if (config / 'trust/stream.pem').read_bytes() != (stream / 'ca-cert.pem').read_bytes():
        raise RuntimeError('Local fixture trust does not match its generated authority')
    if (stream / 'hosts').read_text() != '127.0.0.1':
        raise RuntimeError('Fixture identity unexpectedly names a non-loopback host')
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(stream / 'cert.pem', stream / 'key.pem')
    original_run(['openssl', 'verify', '-verify_ip', '127.0.0.1', '-CAfile',
                  str(stream / 'ca-cert.pem'), str(stream / 'cert.pem')],
                 env=env, check=True, capture_output=True, text=True, timeout=15)


def report_error(error):
    print('Validation subprocess failed: ' + str(error), file=sys.stderr, flush=True)
    for label, value in [('stdout', getattr(error, 'stdout', None)), ('stderr', getattr(error, 'stderr', None))]:
        if value:
            if isinstance(value, bytes): value = value.decode(errors='replace')
            print(f'{label}: {value[-16000:]}', file=sys.stderr, flush=True)


def diagnostic_dump(command, kwargs, original_run):
    try:
        address = command[command.index('--address') + 1]
        if not re.fullmatch(r'127\.0\.0\.1:[0-9]{1,5}', address):
            raise RuntimeError('Diagnostics require the existing loopback simulator address')
        log = kwargs.get('stdout')
        def emit(message):
            if log is not None and hasattr(log, 'write'):
                log.write(message)
                log.flush()
            else:
                print(message, file=sys.stderr, flush=True)
        # Diagnostics run before the unchanged helper's finally closes its
        # simulator. No additional UI actions or screenshots are requested.
        try:
            dumped = original_run([command[0], 'drive', '--address', address, '--step', 'dump'],
                                  cwd=kwargs.get('cwd'), env=kwargs.get('env'),
                                  capture_output=True, text=True, timeout=15)
            emit('\n--- LIVE FAILED-ROUTE TEXT/HIT-TARGET DUMP ---\n' + dumped.stdout + dumped.stderr + '\n')
        except Exception as error:
            emit('\nLive CLI dump unavailable: ' + str(error) + '\n')
        for endpoint in ('layout', 'diagnostics'):
            try:
                maximum = 256 * 1024
                with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as response:
                    raw = response.read(maximum + 1)
                truncated = len(raw) > maximum
                emit(f'\n--- LIVE /{endpoint} (bounded 256 KiB; truncated={truncated}) ---\n'
                     + raw[:maximum].decode(errors='replace') + '\n')
            except Exception as error:
                emit(f'\nLive /{endpoint} unavailable: {error}\n')
    except Exception as error:
        print('Live diagnostic dump unavailable: ' + str(error), file=sys.stderr, flush=True)


def execute(helper, arguments):
    helper = Path(helper).resolve()
    root = helper.parents[2]
    if helper.name not in ALLOWED_HELPERS or helper.parent != root / 'scripts/quality':
        raise RuntimeError('Adapter only supports the six committed specialized helpers')
    revision = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
    if revision != UNION_SHA:
        raise RuntimeError('Adapter requires exact production union ' + UNION_SHA)
    status = subprocess.check_output(['git', '-C', str(root), 'status', '--porcelain', '--untracked-files=all'], text=True)
    if status.strip():
        raise RuntimeError('Adapter requires a clean production checkout')
    target = Path(os.environ['CARGO_TARGET_DIR']).resolve()
    cli = str(target / 'debug/kobo')
    original_run = subprocess.run
    with tempfile.TemporaryDirectory(prefix='cobalt-local-tls-', dir='/tmp') as temporary:
        fixture = None
        def adapted_run(command, *args, **kwargs):
            nonlocal fixture
            command_list = list(map(str, command)) if isinstance(command, (list, tuple)) else []
            if helper.name in FIXTURE_HELPERS and command_list == [cli, 'stream', 'init', '--host', '127.0.0.1']:
                env = kwargs.get('env')
                if not env or 'TMPDIR' not in env or 'KOBO_STREAM_CONFIG_DIR' not in env:
                    raise RuntimeError('Fixture init requires the unchanged helper private environment')
                private, config = Path(env['TMPDIR']).resolve(), Path(env['KOBO_STREAM_CONFIG_DIR']).resolve()
                if not config.is_relative_to(private):
                    raise RuntimeError('Fixture init would leave its private temporary directory')
                print('VALIDATION ADAPTATION: generating loopback fixture TLS via exact production '
                      'kobo_stream::init; no reader discovery, SSH, or pairing', file=sys.stderr, flush=True)
                try:
                    if fixture is None:
                        fixture = build_fixture(root, target, Path(temporary), env, original_run)
                    result = original_run([str(fixture), '--host', '127.0.0.1'], *args, **kwargs)
                    # Do not turn failed initialization into success.
                    if result.returncode != 0:
                        return result
                    validate_fixture(env, original_run)
                    return subprocess.CompletedProcess(command, result.returncode, result.stdout, result.stderr)
                except Exception as error:
                    report_error(error)
                    raise
            try:
                return original_run(command, *args, **kwargs)
            except subprocess.CalledProcessError as error:
                if command_list[:2] == [cli, 'drive'] and '--address' in command_list:
                    diagnostic_dump(command_list, kwargs, original_run)
                report_error(error)
                raise
        subprocess.run = adapted_run
        old_argv, old_path = sys.argv, list(sys.path)
        sys.argv = [str(helper)] + arguments
        sys.path.insert(0, str(helper.parent))
        try:
            runpy.run_path(str(helper), run_name='__main__')
        finally:
            subprocess.run = original_run
            sys.argv, sys.path[:] = old_argv, old_path


if __name__ == '__main__':
    if len(sys.argv) < 2:
        raise SystemExit('usage: specialized_adapter.py COMMITTED_HELPER.py [original arguments ...]')
    execute(sys.argv[1], sys.argv[2:])
