# tests/units/test_port_config.py

import time
from typing import Tuple, List
from utils.template import VaneInstance


def wait_for_log(
    vane_instance: VaneInstance,
    search_string: str,
    timeout_secs: int,
    start_index: int = 0,
) -> bool:
    """
    Polls the captured output for a specific string starting from a given index.
    """
    for _ in range(timeout_secs * 2):  # Poll every 0.5 seconds
        # Only search in the logs that have appeared since the last check
        log_slice = vane_instance.captured_output[start_index:]
        if any(search_string in line for line in log_slice):
            return True
        time.sleep(0.5)
    return False


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests initial load, hot-unload, and hot-reload of a listener config.
    """
    up_string = "PORT 80 TCP UP"
    down_string = "PORT 80 TCP DOWN"

    # The backslash in the regex pattern must be escaped for the f-string.
    toml_content = """
[[protocols]]
name = "http"
priority = 2
detect = { method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }
destination = { type = "forward", forward = { strategy = "random", targets = [
    { ip = "127.0.0.1", port = 3333 },
] } }
"""

    try:
        env_vars = {"LOG_LEVEL": "debug"}
        # We pass an empty string because we will poll the log manually.
        vane = VaneInstance(env_vars, "", debug_mode)

        # Pre-start setup: Create the configuration file before Vane launches.
        listener_dir = vane.tmpdir / "listener" / "[80]"
        listener_dir.mkdir(parents=True, exist_ok=True)
        config_file_path = listener_dir / "tcp.toml"
        config_file_path.write_text(toml_content)

        with vane:
            # Phase 1: Test initial load.
            if not wait_for_log(vane, up_string, 10):
                log_dump = "".join(vane.captured_output)
                reason = (
                    f"Timeout waiting for initial load (did not find '{up_string}')."
                )
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

            log_len_after_up1 = len(vane.captured_output)

            # Phase 2: Test hot-unload. Delete the file.
            config_file_path.unlink()
            if not wait_for_log(vane, down_string, 10, start_index=log_len_after_up1):
                log_dump = "".join(vane.captured_output)
                reason = f"File deleted, but did not find '{down_string}' log."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

            log_len_after_down = len(vane.captured_output)

            # Phase 3: Test hot-reload. Re-create the file.
            config_file_path.write_text(toml_content)
            if not wait_for_log(vane, up_string, 10, start_index=log_len_after_down):
                log_dump = "".join(vane.captured_output)
                reason = (
                    f"File re-created, but did not find the second '{up_string}' log."
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
