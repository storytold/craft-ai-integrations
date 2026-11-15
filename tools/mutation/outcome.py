"""Decide when a passing mutation run proves nothing about the mutant.

A test that skips itself prints SKIP_MARK, so its pass says nothing about the code.

On Windows, tests marked `#[cfg(unix)]` are not compiled, so cargo runs zero tests and exits 0.
That pass also says nothing, and counting it as NOT CAUGHT would fail a Windows run for a reason
unrelated to the mutant. On Linux and macOS a zero-test run is not a skip: there it means the
test filter matched nothing, and that miss must stay visible.
"""

import re

# Printed by a test that skips itself because the machine cannot do what it needs.
SKIP_MARK = 'skipped: this machine does not allow directory links'

# Cargo prints one of these per test binary it runs: "running 0 tests", "running 1 test".
_RUNNING = re.compile(r'running (\d+) tests?\b')


def proves_nothing(log, windows):
    """True when the run neither caught the mutant nor showed it survives."""
    if SKIP_MARK in log:
        return True
    if not windows:
        return False
    counts = [int(n) for n in _RUNNING.findall(log)]
    return bool(counts) and not any(counts)
