# tests/units/test_socket_dir.py

from typing import Tuple
from utils.template import VaneInstance


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if the SOCKET_DIR environment variable is respected.
    """
    string_to_find = "Management console listening on unix:"

    try:
        # We don't need to pass any specific env_vars here, as the template
        # will set SOCKET_DIR to its own temporary directory by default.
        # We just need to capture that directory's path for our assertion.
        with VaneInstance({}, string_to_find, debug_mode) as vane:
            event_was_set = vane.found_event.wait(timeout=10)
            if not event_was_set:
                log_dump = "".join(vane.captured_output)
                reason = f"Timeout after 10s waiting for socket log line."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

            # Construct the expected path
            expected_socket_path = vane.tmpdir / "console.sock"

            # Find the actual path from the logs
            actual_socket_path = None
            for line in vane.captured_output:
                if string_to_find in line:
                    # Extract the path part, e.g., "unix:/path/to/socket"
                    path_str = line.split("unix:")[1].strip()
                    actual_socket_path = path_str
                    break

            if actual_socket_path is None:
                log_dump = "".join(vane.captured_output)
                reason = "Log line was found, but socket path could not be parsed."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

            # Assert that the paths match
            if str(expected_socket_path) != actual_socket_path:
                log_dump = "".join(vane.captured_output)
                reason = (
                    f"Socket path mismatch.\n"
                    f"      ├─ Expected: {expected_socket_path}\n"
                    f"      └─ Actual:   {actual_socket_path}"
                )
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

    except Exception as e:
        return (
            False,
            f"  └─ Details: An unexpected exception occurred during the test: {e}",
        )

    return (True, "")
