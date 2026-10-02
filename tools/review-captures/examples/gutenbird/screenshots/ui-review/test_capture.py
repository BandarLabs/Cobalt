"""Scope regression for the standalone native capture harness."""
import unittest
from capture import insert_test_scenario


class ScopeTests(unittest.TestCase):
    def test_later_modules_and_braces_do_not_change_the_target(self):
        source = 'mod tests {\n const FIXTURE: &str = "}";\n}\nmod large_text_tests {\n}\n'
        result = insert_test_scenario(source, 'fn probe() { assert_eq!(FIXTURE, "}"); }')
        self.assertTrue(result.startswith('mod tests {\nfn probe()'))
        self.assertTrue(result.endswith('mod large_text_tests {\n}\n'))

    def test_missing_or_ambiguous_scope_is_rejected(self):
        for source in ['', 'mod tests {}\nmod tests {}']:
            with self.assertRaises(ValueError):
                insert_test_scenario(source, 'fn probe() {}')


if __name__ == '__main__':
    unittest.main()
