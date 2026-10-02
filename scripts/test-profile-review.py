"""No simulator or sockets: test checkout selection and per-app report handling."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
spec = importlib.util.spec_from_file_location('review', Path(__file__).parent / 'run-profile-review.py')
review = importlib.util.module_from_spec(spec)
spec.loader.exec_module(review)


class HarnessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / 'scripts').mkdir()
        self.items = [dict(app=a, number=i, head='after-' + a)
                      for i, a in enumerate(('launcher', 'settings', 'pubquiz', 'backgammon', 'birds'))]
        self.manifest = dict(baseline='before', apps=self.items,
                             repair_after_apps=['launcher', 'settings', 'pubquiz', 'backgammon'],
                             repair_after_by_profile={'firmware-profile': ['birds']})
        (self.root / 'scripts/profile-review-heads.json').write_text(json.dumps(self.manifest))
        self.commands, self.calls, self.fails = [], [], set()

    def tearDown(self): self.temp.cleanup()

    def run_command(self, command, **kwargs):
        self.commands.append((command, kwargs))
        if command[:3] == ['git', 'worktree', 'add']:
            work = Path(command[4])
            for item in self.items:
                group = 'examples' if item['app'] in ('launcher', 'settings') else 'apps'
                directory = work / group / item['app']
                (directory / 'src').mkdir(parents=True, exist_ok=True)
                if item['app'] != 'settings':
                    (directory / 'drive.kobo').write_text('expect ' + item['app'] + '\n')
        return SimpleNamespace(returncode=0)

    def run_app(self, app, kobo, dest, env, timeout, route_override):
        self.calls.append(dict(app=app, env=env, route=route_override.read_text()))
        return dict(app=app, launched=True, status='fail' if app in self.fails else 'pass')

    def execute(self, args):
        with patch.object(review, 'ROOT', self.root), \
             patch.object(review, 'load_helper', return_value=SimpleNamespace(run_app=self.run_app)), \
             patch.object(review, 'source_tree', side_effect=lambda w, p: str(p.relative_to(w))), \
             patch.object(review.subprocess, 'run', side_effect=self.run_command):
            code = review.main(args)
        report = json.loads(next((self.root / 'target/profile-review').rglob('profile-results.json')).read_text())
        return code, report

    def test_baseline_all_builtins_one_build(self):
        code, report = self.execute(['--profile', 'normal', '--phase', 'before'])
        self.assertEqual(code, 0)
        self.assertEqual(len(self.calls), 5)
        self.assertEqual(sum(c[0][0] == 'cargo' for c in self.commands), 1)
        self.assertTrue(all(r['source_sha'] == 'before' for r in report['results']))
        self.assertTrue(all(c['route'].endswith('clean\nshot profile-final\n') for c in self.calls))
        self.assertIsNone(next(r for r in report['results'] if r['app'] == 'settings')['route'])

    def test_after_repair_scope_reuses_successes(self):
        code, report = self.execute(['--profile', 'normal', '--phase', 'after', '--repair-only'])
        self.assertEqual(code, 0)
        self.assertEqual(len(report['results']), 4)
        self.assertTrue(all(r['source_sha'] == 'after-' + r['app'] for r in report['results']))
        self.assertEqual(sum(c[0][0] == 'cargo' for c in self.commands), 4)

    def test_specific_profile_includes_only_its_additional_failure(self):
        code, report = self.execute(['--profile', 'firmware-profile', '--phase', 'after', '--repair-only'])
        self.assertEqual(code, 0)
        self.assertEqual(len(report['results']), 5)
        self.assertEqual(report['results'][-1]['app'], 'birds')

    def test_route_failure_does_not_discard_other_results(self):
        self.fails.add('pubquiz')
        code, report = self.execute(['--profile', 'normal', '--phase', 'after'])
        self.assertEqual(code, 1)
        self.assertEqual(len(report['results']), 5)
        self.assertEqual([r['app'] for r in report['results'] if r['status'] != 'pass'], ['pubquiz'])

    def test_review_route_preserves_source_route_provenance(self):
        route = self.root / 'scripts/review.kobo'
        route.write_text('expect current\n')
        self.manifest['apps'][0]['review_route'] = 'scripts/review.kobo'
        (self.root / 'scripts/profile-review-heads.json').write_text(json.dumps(self.manifest))
        code, report = self.execute(['--profile', 'normal', '--phase', 'after', '--apps', 'launcher'])
        self.assertEqual(code, 0)
        result = report['results'][0]
        self.assertEqual(result['review_route'], 'scripts/review.kobo')
        self.assertNotEqual(result['route_sha256'], result['committed_route_sha256'])
        self.assertEqual(result['capture_scope'], 'review route plus final screenshot')
        self.assertTrue(self.calls[0]['route'].startswith('expect current\n'))

    def test_unknown_app_prevents_execution(self):
        with self.assertRaises(SystemExit) as raised:
            self.execute(['--profile', 'normal', '--phase', 'after', '--apps', 'unknown'])
        self.assertEqual(raised.exception.code, 2)
        self.assertEqual(self.commands, [])


if __name__ == '__main__': unittest.main()
