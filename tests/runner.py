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
]


def run_suite(debug_mode: bool):
    """
    Runs all defined tests sequentially and reports a summary.
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
    print("+ Test Summary")
    print(
        f"Result: {passed_count} passed, {failed_count} failed out of {total_tests} total tests."
    )
    print(f"Total duration: {duration:.2f}s")

    if failed_count > 0:
        sys.exit(1)
