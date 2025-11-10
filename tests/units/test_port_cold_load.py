# tests/units/test_port_cold_load.py

from typing import Tuple
from utils.template import VaneInstance
from utils.port_config_utils import TOML_CONTENT, UP_STRING, wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane correctly loads an existing listener configuration on startup.
    """
    try:
        env_vars = {"LOG_LEVEL": "debug"}
        vane = VaneInstance(env_vars, "", debug_mode)

        listener_dir = vane.tmpdir / "listener" / "[80]"
        listener_dir.mkdir(parents=True, exist_ok=True)
        (listener_dir / "tcp.toml").write_text(TOML_CONTENT)

        with vane:
            if not wait_for_log(vane, UP_STRING, 10):
                log_dump = "".join(vane.captured_output)
                reason = (
                    f"Timeout waiting for initial load (did not find '{UP_STRING}')."
                )
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
