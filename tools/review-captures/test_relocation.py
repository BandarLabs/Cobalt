"""Fast capture harness checks: no renderer, network, or output files required."""
import ast
import importlib.util
import json
from pathlib import Path
import unittest

BASE = Path(__file__).resolve().parent
ROOT = BASE.parent.parent


class RelocationTests(unittest.TestCase):
    def test_every_relocated_source_exists_and_compiles_as_python(self):
        manifests = list(BASE.glob('relocation-*.json'))
        self.assertTrue(manifests)
        for manifest in manifests:
            for old, new in json.loads(manifest.read_text())['original_to_relocated'].items():
                with self.subTest(path=old):
                    source = ROOT / new
                    self.assertTrue(source.is_file())
                    self.assertFalse((ROOT / old).exists(), 'Capture-only source leaked back into app/evidence tree')
                    if source.suffix == '.py':
                        ast.parse(source.read_text(), filename=new)

    def test_capture_only_modules_are_not_in_production(self):
        for group in ['apps', 'examples']:
            for main in (ROOT / group).glob('*/src/main.rs'):
                self.assertNotIn('mod review_capture;', main.read_text())
                self.assertFalse(main.with_name('review_capture.rs').exists())

    def test_runner_uses_temporary_overlay_and_lock(self):
        spec = importlib.util.spec_from_file_location('capture_runner', BASE / 'run.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.assertEqual(module.value({'features': ['runtime-settings']}), '{ features = ["runtime-settings"] }')
        self.assertEqual(module.value(True), 'true')

    def test_gutenbird_injects_into_tests_before_later_modules(self):
        path = BASE / 'examples/gutenbird/screenshots/ui-review/capture.py'
        if not path.exists():
            self.skipTest('Gutenbird is in the other consolidated PR')
        spec = importlib.util.spec_from_file_location('gutenbird_capture', path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        source = 'mod tests { const FIXTURE: &str = "}"; }\nmod later_tests {}'
        actual = module.insert_test_scenario(source, 'fn probe() {}')
        self.assertLess(actual.index('fn probe'), actual.index('mod later_tests'))
        for invalid in ['fn main() {}', 'mod tests {}\nmod tests {}']:
            with self.assertRaises(ValueError):
                module.insert_test_scenario(invalid, 'fn probe() {}')


if __name__ == '__main__':
    unittest.main()
