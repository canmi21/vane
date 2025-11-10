# tests/units/test_env_loglevel.py

import time
from typing import Tuple
from utils.template import VaneInstance


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Runs a multi-stage test for LOG_LEVEL behavior using a test template.
    """
    target_string = "Anynet completed before timeout."

    try:
        # First scenario: LOG_LEVEL=debug, must find the string
        env_debug = {"LOG_LEVEL": "debug"}
        with VaneInstance(env_debug, target_string, debug_mode) as vane:
            event_was_set = vane.found_event.wait(timeout=10)
            if not event_was_set:
                log_dump = "".join(vane.captured_output)
                reason = f"Timeout after 10s waiting for '{target_string}'."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

        # Second scenario: LOG_LEVEL=info, must NOT find the string
        env_info = {"LOG_LEVEL": "info"}
        with VaneInstance(env_info, target_string, debug_mode) as vane:
            time.sleep(0.5)
            if vane.found_event.is_set():
                log_dump = "".join(vane.captured_output)
                reason = f"Unexpectedly found '{target_string}'."
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
