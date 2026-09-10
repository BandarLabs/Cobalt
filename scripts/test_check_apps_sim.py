"""The simulator sweep must fail on a bad route and clean only its own state."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('check_apps_sim', Path(__file__).with_name('check-apps-sim.py'))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class SimulatorRunnerTests(unittest.TestCase):
    def run_fixture(self, startup_failure=False):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app = root / 'apps/fixture'
            app.mkdir(parents=True)
            (app / 'drive.kobo').write_text('expect Home\n')
            out = root / 'output'
            out.mkdir()
            marker = root / 'state.json'
            fake = root / 'kobo'
            fake.write_text(f'''#!{sys.executable}
import json, os, sys, time
from pathlib import Path
if sys.argv[1] == 'dev':
    Path(os.environ['TEST_STATE_FILE']).write_text(json.dumps([os.environ['TMPDIR'], os.getpid()]))
    if os.environ.get('TEST_STARTUP_FAILURE') == '1':
        raise SystemExit(9)
    print('Kobo app simulator: http://127.0.0.1:12345', flush=True)
    time.sleep(60)
elif '--script' in sys.argv:
    raise SystemExit(7)
else:
    print('text ["Home"]')
''')
            fake.chmod(0o755)
            env = dict(os.environ, TEST_STATE_FILE=str(marker), TEST_STARTUP_FAILURE=str(int(startup_failure)))
            with patch.object(runner, 'ROOT', root):
                result = runner.run_app('fixture', fake, out, env, 3)
            self.assertEqual(result['status'], 'fail')
            self.assertEqual(result['launched'], not startup_failure)
            state, pid = json.loads(marker.read_text())
            self.assertFalse(Path(state).exists(), 'temporary app data leaked')
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            return result

    def test_failed_route_is_not_reported_as_success(self):
        result = self.run_fixture()
        self.assertEqual(result['exit_code'], 7)

    def test_early_simulator_exit_cleans_temporary_store(self):
        result = self.run_fixture(startup_failure=True)
        self.assertIn('simulator exited with 9', result['error'])


if __name__ == '__main__':
    unittest.main()
