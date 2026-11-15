import unittest

from outcome import SKIP_MARK, proves_nothing

ZERO = 'running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out\n'
ONE = 'running 1 test\ntest a ... ok\n\ntest result: ok. 1 passed; 0 failed\n'


class OutcomeTests(unittest.TestCase):
    """A pass counts as a survivor only when a test really ran against the mutant."""

    def test_zero_tests_on_windows_proves_nothing(self):
        """Unix-only tests are compiled out on Windows, so nothing ran."""
        self.assertTrue(proves_nothing(ZERO, windows=True))

    def test_zero_tests_elsewhere_is_still_a_miss(self):
        """On Linux and macOS a zero-test run means the filter matched nothing."""
        self.assertFalse(proves_nothing(ZERO, windows=False))

    def test_a_test_that_ran_is_evidence(self):
        """A real pass is a survivor on every platform."""
        for windows in (True, False):
            with self.subTest(windows=windows):
                self.assertFalse(proves_nothing(ONE, windows=windows))

    def test_one_binary_ran_tests_so_the_run_counts(self):
        """Zero in one binary does not hide tests that ran in another."""
        self.assertFalse(proves_nothing(ZERO + ONE, windows=True))

    def test_no_cargo_output_is_not_a_skip(self):
        """A run that printed no test counts at all is not quietly skipped."""
        self.assertFalse(proves_nothing('error: could not compile', windows=True))

    def test_the_skip_mark_proves_nothing_everywhere(self):
        """A test that skipped itself passes whatever the code does."""
        for windows in (True, False):
            with self.subTest(windows=windows):
                self.assertTrue(proves_nothing(ONE + SKIP_MARK, windows=windows))


if __name__ == '__main__':
    unittest.main()
