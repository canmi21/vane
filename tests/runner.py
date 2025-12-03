# tests/runner.py

import sys
import time

# The runner now only needs to import the master list from the registry file.
from test_suite import TEST_SUITE


def run_suite(debug_mode: bool, args: list):
    """
    Runs a filtered and ordered test suite based on command-line arguments.
    The test cases themselves are defined in `test_suite.py`.
    """
    import argparse
    from typing import List, Tuple, Callable

    parser = argparse.ArgumentParser(description="Vane Test Runner")
    parser.add_argument(
        "--start", type=int, help="Start tests from this number (inclusive)."
    )
    parser.add_argument(
        "--skip", type=str, help="Skip specific tests. E.g., '3', '1-2', '1,5,6'."
    )
    # The runner can also accept filter strings to run a subset of tests.
    parser.add_argument(
        "filters", nargs="*", help="Run only tests whose names contain these strings."
    )
    ns, _ = parser.parse_known_args(args)

    total_in_master_suite = len(TEST_SUITE)
    width_absolute = len(str(total_in_master_suite))

    all_tests: List[Tuple[int, str, Callable[[bool], Tuple[bool, str]]]] = [
        (i, name, func) for i, (name, func) in enumerate(TEST_SUITE, 1)
    ]
    test_suite = all_tests

    # --- Filtering Logic ---
    if ns.filters:
        test_suite = [t for t in test_suite if any(f in t[1] for f in ns.filters)]
    if ns.start:
        test_suite = [t for t in test_suite if t[0] >= ns.start]
    if ns.skip:

        def _parse_skip_string(skip_str: str) -> set[int]:
            indices_to_skip = set()
            parts = skip_str.split(",")
            for part in parts:
                if "-" in part:
                    try:
                        start, end = map(int, part.split("-"))
                        if start <= end:
                            indices_to_skip.update(range(start, end + 1))
                    except ValueError:
                        continue
                else:
                    try:
                        indices_to_skip.add(int(part))
                    except ValueError:
                        continue
            return indices_to_skip

        indices_to_skip = _parse_skip_string(ns.skip)
        test_suite = [t for t in test_suite if t[0] not in indices_to_skip]

    # --- Execution Logic ---
    total_to_run = len(test_suite)
    if total_to_run == 0:
        print("No tests to run after filtering.")
        return

    passed_count, failed_count = 0, 0
    start_time = time.monotonic()
    print(f"Running {total_to_run} test(s)...")

    width_running = len(str(total_to_run))

    for i, (test_num, name, test_func) in enumerate(test_suite, 1):
        print(
            f"[{i:0{width_running}}/{total_to_run}] #{test_num:0{width_absolute}} Running: {name} ... ",
            end="",
            flush=True,
        )
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

    # --- Summary ---
    print()
    print("+ Test Summary")
    print(
        f"Result: {passed_count} passed, {failed_count} failed out of {total_to_run} total tests."
    )
    print(f"Total duration: {duration:.2f}s")
    if failed_count > 0:
        sys.exit(1)
