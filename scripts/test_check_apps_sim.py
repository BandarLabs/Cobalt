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
    def test_shards_split_the_apps_evenly_and_together_cover_every_app_once(self):
        apps = [f'app{n}' for n in range(10)]
        shards = [runner.shard_of(apps, f'{k}/3') for k in (1, 2, 3)]
        self.assertEqual(shards[0], ['app0', 'app3', 'app6', 'app9'])
        self.assertEqual(sorted(sum(shards, [])), sorted(apps))
        self.assertEqual(runner.shard_of(apps, None), apps)

    def test_a_shard_outside_its_count_or_badly_written_is_refused(self):
        for shard in ('0/3', '4/3', '2', 'a/b'):
            with self.assertRaises(ValueError):
                runner.shard_of(['app'], shard)

    def test_a_route_asks_for_articles_a_pairing_or_a_computer_on_its_first_line(self):
        with tempfile.TemporaryDirectory() as temporary:
            route = Path(temporary) / 'drive-articles.kobo'
            route.write_text('# sweep: --bare paired articles\nexpect Books\n')
            options = runner.route_directive(route)
            self.assertTrue(options['bare'] and options['paired'] and options['articles'])
            self.assertFalse(options['computer'])
            route.write_text('# sweep: computer\n')
            self.assertTrue(runner.route_directive(route)['computer'])

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
