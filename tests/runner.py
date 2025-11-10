# tests/runner.py

import sys
import time
from units import test_env_loglevel

# The master list of all tests to be executed sequentially.
TEST_SUITE = [
    ("units.test_env_loglevel", test_env_loglevel.run),
]


def run_suite(debug_mode: bool):
    """
    Runs all defined tests sequentially and reports a summary.

    Args:
        debug_mode: If True, prints sub-process logs even for successful tests.
    """
    total_tests = len(TEST_SUITE)
    passed_count = 0
    failed_count = 0
    start_time = time.monotonic()

    for i, (name, test_func) in enumerate(TEST_SUITE, 1):
        print(f"[{i}/{total_tests}] Running test: {name} ... ", end="", flush=True)

        try:
            success, details = test_func(debug_mode)
            if success:
                print("PASSED")
                passed_count += 1
            else:
                print("FAILED", file=sys.stderr)
                print(details, file=sys.stderr)
                failed_count += 1
        except Exception as e:
            print("CRASHED", file=sys.stderr)
            print(f"  └─ Unhandled Exception: {e}\n", file=sys.stderr)
            failed_count += 1

    duration = time.monotonic() - start_time
    print()
    print("--- Test Summary ---")
    print(
        f"Result: {passed_count} passed, {failed_count} failed out of {total_tests} total tests."
    )
    print(f"Total duration: {duration:.2f}s")

    if failed_count > 0:
        sys.exit(1)
