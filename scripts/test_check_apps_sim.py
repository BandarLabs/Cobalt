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

    def test_a_route_names_a_site_it_reads_by_its_https_address(self):
        with tempfile.TemporaryDirectory() as temporary:
            route = Path(temporary) / 'drive-catalog.kobo'
            route.write_text('# sweep: --bare needs https://catalog.example/feed needs https://other.example/\n')
            options = runner.route_directive(route)
            self.assertEqual(options['needs'], ['https://catalog.example/feed', 'https://other.example/'])
            self.assertTrue(options['bare'])
            route.write_text('expect Home\n')
            self.assertEqual(runner.route_directive(route)['needs'], [])
            for wrong in ('needs catalog.example', 'needs http://catalog.example/', 'needs'):
                route.write_text(f'# sweep: {wrong}\n')
                with self.assertRaises(ValueError):
                    runner.route_directive(route)

    def swept(self, routes, down):
        """Sweeps a made-up app whose routes are `routes`, with `down` not answering."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app = root / 'apps/fixture'
            app.mkdir(parents=True)
            for name, text in routes.items():
                (app / name).write_text(text)
            ran = []

            def passed(app, *_, label=None, route_override=None, **__):
                ran.append(label or app)
                return dict(app=label or app, launched=True, status='pass')

            with patch.object(runner, 'ROOT', root), \
                    patch.object(runner, 'run_app', passed), \
                    patch.object(runner, 'answering', lambda address: address not in down):
                return runner.run_routes('fixture', root / 'kobo', root, {}, 3), ran

    def test_a_route_whose_site_is_down_is_left_out_by_name_and_the_others_still_run(self):
        routes = {
            'drive.kobo': '# sweep: needs https://catalog.example/feed\nexpect Popular\n',
            'drive-search.kobo': '# sweep: needs https://catalog.example/feed\nexpect Search\n',
            'drive-offline.kobo': '# sweep: --bare\nexpect On this device\n',
        }
        results, ran = self.swept(routes, down={'https://catalog.example/feed'})
        by_name = {result['app']: result for result in results}
        self.assertEqual(ran, ['fixture:drive-offline'])
        for left_out in ('fixture', 'fixture:drive-search'):
            self.assertEqual(by_name[left_out]['status'], 'skipped')
            self.assertEqual(by_name[left_out]['reason'], 'https://catalog.example/feed is not answering')
            self.assertFalse(by_name[left_out]['launched'])
        self.assertEqual(by_name['fixture:drive-offline']['status'], 'pass')
        # With the site up, every one of them runs.
        results, ran = self.swept(routes, down=set())
        self.assertEqual(ran, ['fixture', 'fixture:drive-offline', 'fixture:drive-search'])
        self.assertTrue(all(result['status'] == 'pass' for result in results))

    def test_a_main_route_may_ask_for_a_site_and_for_nothing_else(self):
        results, ran = self.swept({'drive.kobo': '# sweep: --bare\nexpect Home\n'}, down=set())
        self.assertEqual(ran, [])
        self.assertEqual(results[0]['status'], 'fail')
        self.assertIn('may only say what it needs', results[0]['error'])

    def test_a_site_counts_as_there_unless_it_cannot_be_reached_or_faults(self):
        import urllib.error

        def answer(status):
            class Answer:
                def __enter__(self):
                    return self

                def __exit__(self, *_):
                    return False
            Answer.status = status
            return Answer()

        def refusing(code):
            def raised(*_, **__):
                raise urllib.error.HTTPError('https://x.example/', code, 'said so', {}, None)
            return raised

        def unreachable(*_, **__):
            raise urllib.error.URLError('no route')

        cases = [
            (lambda *_, **__: answer(200), True),
            (refusing(403), True),
            (refusing(404), True),
            (refusing(503), False),
            (unreachable, False),
        ]
        for number, (opened, there) in enumerate(cases):
            with patch.object(runner.urllib.request, 'urlopen', opened):
                self.assertEqual(runner.answering(f'https://case{number}.example/'), there, number)
        # Asked once a run: the answer is kept, whatever the site does next.
        with patch.object(runner.urllib.request, 'urlopen', unreachable):
            self.assertTrue(runner.answering('https://case0.example/'))

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
