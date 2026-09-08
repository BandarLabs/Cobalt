import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('clara_checks', Path(__file__).with_name('clara-bw-check.py'))
CHECKS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKS)


class ClaraChecks(unittest.TestCase):
    def observation(self):
        return json.loads((CHECKS.ROOT / 'docs/quality/fixtures/clara-bw-synthetic.json').read_text())

    def test_synthetic_and_wrong_profile_never_authorize_physical_checks(self):
        observation = self.observation()
        with self.assertRaisesRegex(ValueError, 'synthetic'):
            CHECKS.require_clara_observation(json.dumps(observation).encode())
        observation['origin'] = 'cobalt-device-probe'
        observation['observed']['identity'].update(firmware_version='test-firmware', kernel_release='test-kernel')
        self.assertEqual(CHECKS.require_clara_observation(json.dumps(observation).encode()), observation)
        observation['observed']['identity']['device_code'] = 395
        with self.assertRaisesRegex(ValueError, '391'):
            CHECKS.require_clara_observation(json.dumps(observation).encode())

    def test_failed_identity_stops_before_any_panel_or_setting_operation(self):
        with tempfile.TemporaryDirectory() as private:
            output = Path(private) / 'run'
            calls = []
            def fake_command(argv, stdout, stderr, timeout=240):
                calls.append(argv)
                stdout.write_text(json.dumps(self.observation()))
                stderr.write_text('')
                return 0, 1
            argv = ['clara-bw-check.py', '--device', 'fixture.invalid', '--phase', 'display', '--out', str(output), '--execute']
            with patch.object(sys, 'argv', argv), patch.object(CHECKS, 'run_command', fake_command):
                with self.assertRaisesRegex(ValueError, 'synthetic'):
                    CHECKS.main()
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0][1], 'doctor')
            report = json.loads((output / 'run.json').read_text())
            self.assertEqual(report['status'], 'failed')
            self.assertEqual(report['physicalAssessment'], 'pending')
            self.assertTrue(all(step['status'] == 'planned' for step in report['steps'][1:]))

    def test_plan_has_no_device_effects_and_never_claims_calibration(self):
        with tempfile.TemporaryDirectory() as private:
            output = Path(private) / 'plan'
            argv = ['clara-bw-check.py', '--device', 'fixture.invalid', '--phase', 'recovery', '--out', str(output)]
            with patch.object(sys, 'argv', argv), patch.object(CHECKS, 'run_command') as run:
                CHECKS.main()
                run.assert_not_called()
            report = json.loads((output / 'run.json').read_text())
            self.assertEqual(report['status'], 'planned')
            self.assertFalse(report['calibrationApplied'])
            self.assertEqual(output.stat().st_mode & 0o777, 0o700)

    def test_settings_compare_ignores_counters_but_requires_owner_values(self):
        settings = 'wake_lock: owner\nForceWifiOn: unset\nAutoSleepMinutes: 15\nconfig_backup: absent\n'
        self.assertEqual(CHECKS.session_settings(settings + 'suspend_events: 1\n'),
                         CHECKS.session_settings(settings + 'suspend_events: 2\n'))
        self.assertNotEqual(CHECKS.session_settings(settings), CHECKS.session_settings(settings.replace('15', '90')))
        with self.assertRaises(ValueError):
            CHECKS.session_settings('wake_lock: owner\n')

    def test_touch_requires_a_named_point_and_does_not_synthesize_input(self):
        with self.assertRaises(ValueError):
            CHECKS.commands(Path('kobo'), 'fixture.invalid', 'touch', Path('/tmp/evidence'))
        steps = CHECKS.commands(Path('kobo'), 'fixture.invalid', 'touch', Path('/tmp/evidence'), 'top-left')
        self.assertTrue(any(command[1] == 'touch-probe' for _, command in steps))
        self.assertFalse(any(command[1] == 'tap' for _, command in steps))

    def test_wifi_return_alone_is_not_a_verified_resume_and_reboots_are_distinct(self):
        boot = 'boot_id: 00000000-1111-2222-3333-444444444444\n'
        before = boot + 'suspend_events: 5\nuptime_seconds: 200\n'
        resumed = CHECKS.wake_evidence(before, boot + 'suspend_events: 6\nuptime_seconds: 400\n')
        self.assertTrue(resumed['suspendObserved'] and resumed['uptimeDidNotReset'] and resumed['sameBoot'])
        later_boot = CHECKS.wake_evidence(before, before.replace('444444444444', '555555555555').replace('events: 5', 'events: 6'))
        self.assertTrue(later_boot['uptimeDidNotReset'])
        self.assertFalse(later_boot['sameBoot'])
        unchanged = CHECKS.wake_evidence(before, before + 'wifi_operstate: up\n')
        self.assertFalse(unchanged['suspendObserved'])
        rebooted = CHECKS.wake_evidence(before, boot + 'suspend_events: 0\nuptime_seconds: 10\n')
        self.assertFalse(rebooted['uptimeDidNotReset'])
        with self.assertRaises(ValueError):
            CHECKS.wake_evidence(before, 'wifi_operstate: up\n')

    def test_timeout_reaps_only_the_owned_host_command_group(self):
        with tempfile.TemporaryDirectory() as private:
            root = Path(private)
            with self.assertRaises(subprocess.TimeoutExpired):
                CHECKS.run_command([sys.executable, '-c', 'import time; time.sleep(60)'], root/'out', root/'err', timeout=0.05)


if __name__ == '__main__':
    unittest.main()
