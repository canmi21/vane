# tests/units/test_invalid_json.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane correctly logs an error when encountering a syntactically
    invalid (empty) JSON configuration file.
    """
    # The expected error log line when parsing fails.
    success_string = "Failed to parse config file"

    try:
        port = net_utils.find_available_tcp_port()
        env_vars = {"LOG_LEVEL": "debug"}

        # Instantiate the manager, passing the error string we expect to find.
        vane = VaneInstance(env_vars, success_string, debug_mode)

        # Before starting Vane, create the invalid file structure.
        listener_dir = vane.tmpdir / "listener" / f"[{port}]"
        listener_dir.mkdir(parents=True, exist_ok=True)

        # Create an empty, and therefore invalid, JSON file.
        (listener_dir / "tcp.json").touch()

        # Start Vane, which will attempt to load the invalid file on startup.
        with vane:
            if not vane.found_event.wait(timeout=10):
                log_dump = "".join(vane.captured_output)
                reason = f"Timeout waiting for parsing error (did not find '{success_string}')."
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
