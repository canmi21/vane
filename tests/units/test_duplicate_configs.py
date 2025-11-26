# tests/units/test_duplicate_configs.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane correctly detects and warns about multiple configuration
    files for the same port and protocol.
    """
    success_string = "Found multiple config files"

    try:
        port = net_utils.find_available_tcp_port()
        env_vars = {"LOG_LEVEL": "debug"}

        # Instantiate the manager, passing the string we expect to find.
        vane = VaneInstance(env_vars, success_string, debug_mode)

        # Before starting Vane, create the conflicting file setup.
        listener_dir = vane.tmpdir / "listener" / f"[{port}]"
        listener_dir.mkdir(parents=True, exist_ok=True)

        # Create two empty config files for the same port/protocol.
        (listener_dir / "tcp.toml").touch()
        (listener_dir / "tcp.json").touch()

        # Start Vane, which will discover the duplicate files on startup.
        with vane:
            if not vane.found_event.wait(timeout=10):
                log_dump = "".join(vane.captured_output)
                reason = f"Timeout waiting for duplicate config warning (did not find '{success_string}')."
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
