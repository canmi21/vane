# tests/runner.py

import sys
import time
from units import (
    test_env_loglevel,
    test_socket_dir,
    test_console,
    test_port_cold_load,
    test_port_hot_unload,
    test_port_hot_reload,
    test_config_formats_toml_yaml,
    test_config_formats_json_toml,
    test_config_formats_yaml_json,
    test_dynamic_port,
    test_multi_port_binding,
    test_tcp_proxy,
    test_tcp_filtering,
    test_mix_port_forwarding,
    test_protocol_priority,
    test_duplicate_configs,
    test_invalid_json,
    test_invalid_toml,
    test_invalid_yaml,
    test_strategy_serial,
    test_strategy_random,
    test_strategy_fastest,
    test_no_available_targets,
    test_routing_to_single_available_target,
    test_serial_strategy_with_runtime_failure,
    test_backend_auto_recovery,
    test_fallback_routing,
    test_fallback_auto_recovery,
    test_capture_all_fallback,
    test_udp_proxy,
    test_udp_fallback,  # Import the new UDP fallback test case
)

# The master list of all tests to be executed sequentially.
TEST_SUITE = [
    ("units.test_env_loglevel", test_env_loglevel.run),
    ("units.test_socket_dir", test_socket_dir.run),
    ("units.test_console", test_console.run),
    ("units.test_port_cold_load", test_port_cold_load.run),
    ("units.test_port_hot_unload", test_port_hot_unload.run),
    ("units.test_port_hot_reload", test_port_hot_reload.run),
    ("units.test_config_formats_toml_yaml", test_config_formats_toml_yaml.run),
    ("units.test_config_formats_json_toml", test_config_formats_json_toml.run),
    ("units.test_config_formats_yaml_json", test_config_formats_yaml_json.run),
    ("units.test_dynamic_port", test_dynamic_port.run),
    ("units.test_multi_port_binding", test_multi_port_binding.run),
    ("units.test_tcp_proxy", test_tcp_proxy.run),
    ("units.test_tcp_filtering", test_tcp_filtering.run),
    ("units.test_mix_port_forwarding", test_mix_port_forwarding.run),
    ("units.test_protocol_priority", test_protocol_priority.run),
    ("units.test_duplicate_configs", test_duplicate_configs.run),
    ("units.test_invalid_json", test_invalid_json.run),
    ("units.test_invalid_toml", test_invalid_toml.run),
    ("units.test_invalid_yaml", test_invalid_yaml.run),
    ("units.test_strategy_serial", test_strategy_serial.run),
    ("units.test_strategy_random", test_strategy_random.run),
    ("units.test_strategy_fastest", test_strategy_fastest.run),
    ("units.test_no_available_targets", test_no_available_targets.run),
    (
        "units.test_routing_to_single_available_target",
        test_routing_to_single_available_target.run,
    ),
    (
        "units.test_serial_strategy_with_runtime_failure",
        test_serial_strategy_with_runtime_failure.run,
    ),
    ("units.test_backend_auto_recovery", test_backend_auto_recovery.run),
    ("units.test_fallback_routing", test_fallback_routing.run),
    ("units.test_fallback_auto_recovery", test_fallback_auto_recovery.run),
    ("units.test_capture_all_fallback", test_capture_all_fallback.run),
    ("units.test_udp_proxy", test_udp_proxy.run),
    # Add the new test case to the very end of the suite.
    ("units.test_udp_fallback", test_udp_fallback.run),
]


def run_suite(debug_mode: bool, args: list):
    """
    Runs a filtered and ordered test suite based on command-line arguments.
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
    ns, _ = parser.parse_known_args(args)

    total_in_master_suite = len(TEST_SUITE)
    width_absolute = len(str(total_in_master_suite))

    all_tests: List[Tuple[int, str, Callable[[bool], Tuple[bool, str]]]] = [
        (i, name, func) for i, (name, func) in enumerate(TEST_SUITE, 1)
    ]
    test_suite = all_tests
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
                        if start < end:
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

    total_to_run = len(test_suite)
    if total_to_run == 0:
        print("No tests to run after filtering.")
        return
    passed_count, failed_count = 0, 0
    start_time = time.monotonic()
    print(f"Running {total_to_run} tests...")

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
    print()
    print("+ Test Summary")
    print(
        f"Result: {passed_count} passed, {failed_count} failed out of {total_to_run} total tests."
    )
    print(f"Total duration: {duration:.2f}s")
    if failed_count > 0:
        sys.exit(1)
