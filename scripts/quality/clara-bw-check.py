#!/usr/bin/env python3
"""Prepare or run the combined Clara BW checks after the three quality PRs are ready."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
MAX_TRANSCRIPT = 8 * 1024 * 1024
PHASES = ('baseline', 'display', 'touch', 'recovery', 'after-wake')
CORNERS = ('top-left', 'top-right', 'bottom-left', 'bottom-right', 'centre')
LIGHT_SNAPSHOT = '\n'.join([
    'for control in /sys/class/backlight/* /sys/class/leds/*; do',
    '  for field in brightness max_brightness color max_color; do',
    '    if [ -r "$control/$field" ]; then',
    '      printf "%s=" "$control/$field"; cat "$control/$field"',
    '    fi',
    '  done',
    'done',
])


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def commands(binary, device, phase, output, corner=None):
    """Only existing fixed CLI operations. Device values are separate argv items."""
    def call(verb, *args):
        return [str(binary), verb, '--device', device, *args]
    steps = [('identity', call('doctor', '--json')),
             ('session-before', call('session', '--status')),
             ('light-before', call('shell', LIGHT_SNAPSHOT))]
    if phase == 'display':
        for label, confirmation in (
            ('clean-refresh', 'REVERSIBLE_PIXELS_GC16'),
            ('whole-screen-restore', 'SCREEN_SNAPSHOT_RESTORE'),
            ('fast-refresh', 'REVERSIBLE_PIXELS_DU'),
            ('wait-timing', 'WAIT_TIMING_GC16_DU'),
        ):
            steps.append((label, call('smoke-display', '--confirm', confirmation)))
    elif phase == 'touch':
        if corner not in CORNERS:
            raise ValueError('Choose the physical point being observed with --corner.')
        steps.append(('touch-' + corner, call('touch-probe', '--seconds', '30')))
    elif phase == 'recovery':
        steps.append(('failed-child-restoration', call('guard-test', '--confirm', 'GUARD_RESTORE_AFTER_FAILURE')))
    steps.extend([
        ('session-after', call('session', '--status')),
        ('light-after', call('shell', LIGHT_SNAPSHOT)),
        ('trace', call('logs', '--dump', '--lines', '200')),
        ('screen', call('shot', '--out', str(output / 'screen.png'))),
    ])
    return steps


def require_clara_observation(source):
    """A coarse harness gate; the device HAL still enforces its exact write gate."""
    if len(source) > 32 * 1024:
        raise ValueError('Doctor observation exceeds 32 KiB.')
    value = json.loads(source)
    if value.get('schema') != 'cobalt.hardware-observation' or value.get('version') != 1:
        raise ValueError('The CLI did not return the hardware observation schema.')
    if value.get('origin') != 'cobalt-device-probe':
        raise ValueError('A physical run requires a device probe, not a synthetic fixture.')
    observed = value.get('observed', {})
    identity, frame = observed.get('identity', {}), observed.get('framebuffer', {})
    if identity.get('device_code') != 391 or (frame.get('width'), frame.get('height')) != (1072, 1448):
        raise ValueError('This run is for the owner\'s Clara BW 391 (1072 × 1448).')
    if not identity.get('firmware_version') or not identity.get('kernel_release'):
        raise ValueError('Firmware and kernel observations are required for compatibility evidence.')
    return value


def session_settings(text):
    """Compare owner settings, not counters or transient Wi-Fi state."""
    result = {}
    for line in text.splitlines():
        key, separator, value = line.partition(':')
        if separator and key in ('wake_lock', 'ForceWifiOn', 'AutoSleepMinutes', 'config_backup'):
            result[key] = value.strip()
    if set(result) != {'wake_lock', 'ForceWifiOn', 'AutoSleepMinutes', 'config_backup'}:
        raise ValueError('The session report is missing owner settings; do not assume absent values are defaults.')
    return result


def wake_evidence(before, after):
    def boot_id(text):
        matches = re.findall(r'^boot_id: ([0-9a-fA-F-]{36})$', text, flags=re.MULTILINE)
        if len(matches) != 1 or not re.fullmatch(r'[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}', matches[0]):
            raise ValueError('A valid device boot identity is required to distinguish wake from reboot.')
        return matches[0].lower()
    def counters(text):
        values = {}
        for line in text.splitlines():
            key, separator, value = line.partition(':')
            if separator and key in ('suspend_events', 'uptime_seconds'):
                number = value.strip()
                if not re.fullmatch(r'[0-9]+(?:\.[0-9]+)?', number):
                    raise ValueError('Invalid suspend/uptime observation.')
                values[key] = float(number)
        if set(values) != {'suspend_events', 'uptime_seconds'}:
            raise ValueError('Suspend count and uptime are required to distinguish wake from reboot.')
        return values
    first, last = counters(before), counters(after)
    return {'before': first, 'after': last,
            'suspendObserved': last['suspend_events'] > first['suspend_events'],
            'uptimeDidNotReset': last['uptime_seconds'] >= first['uptime_seconds'],
            'sameBoot': boot_id(before) == boot_id(after),
            'basis': 'device boot identity, suspend counter and uptime; not inferred from Wi-Fi availability'}


def run_command(argv, stdout, stderr, timeout=240):
    # Owned host process group only: timed-out SSH/build children must not
    # survive the harness. Device operations retain their existing guards.
    started = time.monotonic()
    with stdout.open('wb') as out, stderr.open('wb') as err:
        child = subprocess.Popen(argv, cwd=ROOT, stdout=out, stderr=err, start_new_session=True)
        try:
            code = child.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
            raise
    if stdout.stat().st_size > MAX_TRANSCRIPT or stderr.stat().st_size > MAX_TRANSCRIPT:
        raise ValueError('Command transcript exceeded the evidence limit; inspect the local files.')
    return code, round((time.monotonic() - started) * 1000)


def file_evidence(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return {'file': path.name, 'bytes': path.stat().st_size, 'sha256': digest.hexdigest()}



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device', required=True, help='The reader address discovered for this run')
    parser.add_argument('--phase', choices=PHASES, default='baseline')
    parser.add_argument('--corner', choices=CORNERS, help='Physical point for the touch phase')
    parser.add_argument('--out', required=True, type=Path, help='New private evidence directory')
    parser.add_argument('--cli', type=Path, default=ROOT / 'target/debug/kobo')
    parser.add_argument('--execute', action='store_true', help='Run the printed phase on the attached reader')
    parser.add_argument('--baseline', type=Path, help='Earlier session-before.stdout for after-wake comparison')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.:-]{0,252}', args.device):
        parser.error('Use a device hostname or address without shell characters.')
    if args.phase == 'after-wake' and args.baseline is None:
        parser.error('after-wake requires --baseline from the earlier run.')
    if args.baseline and (not args.baseline.is_file() or args.baseline.stat().st_size > MAX_TRANSCRIPT):
        parser.error('The baseline must be a bounded local session report.')
    output = args.out.resolve()
    try:
        steps = commands(args.cli.resolve(), args.device, args.phase, output, args.corner)
    except ValueError as error:
        parser.error(str(error))
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    revision = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()
    dirty = bool(subprocess.run(['git', 'status', '--porcelain', '--untracked-files=normal'], cwd=ROOT,
                                check=True, capture_output=True, text=True).stdout)
    report = {'schema': 'cobalt.hardware-quality-run', 'version': 1, 'status': 'planned',
              'source': {'revision': revision, 'dirty': dirty}, 'profile': 'clara-bw-391',
              'phase': args.phase, 'corner': args.corner, 'steps': [],
              'cli': file_evidence(args.cli) if args.cli.is_file() else None,
              'physicalAssessment': 'pending', 'calibrationApplied': False,
              'timingBasis': 'host command duration includes builds, transport and device work'}
    for name, argv in steps:
        report['steps'].append({'id': name, 'argv': argv, 'status': 'planned'})
    write_json(output / 'run.json', report)
    print(f"{args.phase}: {len(steps)} checks; evidence in {output}")
    if not args.execute:
        print('Plan created. No device command was run. Use --execute with a new output directory when the three PRs are ready.')
        return
    if args.phase == 'touch':
        print(f'Physical touch check: {args.corner}. Follow the point placement in docs/quality/clara-bw-validation.md. Tap repeatedly during the observation window.', flush=True)
    report['status'] = 'running'
    write_json(output / 'run.json', report)
    try:
        for step in report['steps']:
            print(step['id'], flush=True)
            stdout, stderr = (output / (step['id'] + '.stdout'), output / (step['id'] + '.stderr'))
            step['status'] = 'running'
            write_json(output / 'run.json', report)
            code, elapsed = run_command(step['argv'], stdout, stderr)
            step.update(exitCode=code, hostElapsedMillis=elapsed,
                        stdout=file_evidence(stdout), stderr=file_evidence(stderr),
                        status='completed' if code == 0 else 'failed')
            write_json(output / 'run.json', report)
            if code != 0:
                raise ValueError(f"{step['id']} failed with exit {code}; later checks were not run.")
            if step['id'] == 'identity':
                observation = require_clara_observation(stdout.read_bytes())
                report['compatibility'] = {'observed': observation['observed'],
                                           'availableBackends': observation['available_backends']}
        before = session_settings((output / 'session-before.stdout').read_text())
        after = session_settings((output / 'session-after.stdout').read_text())
        light_before = (output / 'light-before.stdout').read_text()
        light_after = (output / 'light-after.stdout').read_text()
        report['lightComparison'] = {'observed': bool(light_before.strip()), 'unchanged': light_before == light_after}
        if light_before != light_after:
            raise ValueError('Frontlight or warmth changed during this phase; review the captured controls.')
        report['settingsComparison'] = {'before': before, 'after': after, 'unchanged': before == after}
        if args.baseline:
            baseline_text = args.baseline.read_text()
            baseline = session_settings(baseline_text)
            report['wakeEvidence'] = wake_evidence(baseline_text, (output / 'session-after.stdout').read_text())
            if not all(report['wakeEvidence'][key] for key in ('suspendObserved', 'uptimeDidNotReset', 'sameBoot')):
                raise ValueError('No verified suspend/wake cycle, or uptime reset; review device evidence.')
            report['wakeSettingsComparison'] = {'baseline': baseline, 'after': after, 'unchanged': baseline == after}
            baseline_light = args.baseline.parent / 'light-before.stdout'
            if not baseline_light.is_file() or baseline_light.stat().st_size > MAX_TRANSCRIPT:
                raise ValueError('The pre-sleep frontlight baseline is missing or excessive.')
            report['wakeLightUnchanged'] = baseline_light.read_text() == light_after
            if not report['wakeLightUnchanged']:
                raise ValueError('Frontlight or warmth differs from the pre-sleep baseline.')
            if baseline != after:
                raise ValueError('Owner settings differ from the pre-sleep baseline; review the recorded values.')
        if before != after:
            raise ValueError('Owner settings changed during this phase; review the recorded values.')
        report['screen'] = file_evidence(output / 'screen.png')
        report['status'] = 'command-checks-completed'
    except (Exception, KeyboardInterrupt) as error:
        report['status'] = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        report['error'] = str(error) or 'Interrupted by the owner.'
        raise
    finally:
        write_json(output / 'run.json', report)
    print('Command checks completed. Physical assessment and calibration remain pending.')


if __name__ == '__main__':
    main()
