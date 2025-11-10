# tests/units/port_config_utils.py

import time
from typing import List
from utils.template import VaneInstance

# Shared TOML content for all port config tests.
# The backslash in the regex pattern is correctly escaped for f-strings/TOML.
TOML_CONTENT = """
[[protocols]]
name = "http"
priority = 2
detect = { method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }
destination = { type = "forward", forward = { strategy = "random", targets = [
    { ip = "127.0.0.1", port = 3333 },
] } }
"""

UP_STRING = "PORT 80 TCP UP"
DOWN_STRING = "PORT 80 TCP DOWN"
STABLE_STARTUP_STRING = "Initializing listeners from existing config..."


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
        log_slice = vane_instance.captured_output[start_index:]
        if any(search_string in line for line in log_slice):
            return True
        time.sleep(0.5)
    return False
