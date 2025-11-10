# tests/units/test_invalid_toml.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane correctly logs an error when encountering a syntactically
    invalid TOML configuration file.
    """
    success_string = "Failed to parse config file"

    # This is not valid TOML and will cause a parsing error.
    invalid_toml_content = "this is not a valid key = value pair"

    try:
        port = net_utils.find_available_tcp_port()
        env_vars = {"LOG_LEVEL": "debug"}

        vane = VaneInstance(env_vars, success_string, debug_mode)

        listener_dir = vane.tmpdir / "listener" / f"[{port}]"
        listener_dir.mkdir(parents=True, exist_ok=True)
        (listener_dir / "tcp.toml").write_text(invalid_toml_content)

        with vane:
            if not vane.found_event.wait(timeout=10):
                log_dump = "".join(vane.captured_output)
                reason = f"Timeout waiting for TOML parsing error (did not find '{success_string}')."
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
