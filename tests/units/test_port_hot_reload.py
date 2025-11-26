# tests/units/test_port_hot_reload.py

from typing import Tuple
from utils.template import VaneInstance
from .config_utils import (
    TOML_HTTP_SIMPLE,
    PORT_80_TCP_UP,
    STABLE_STARTUP_STRING,
    wait_for_log,
)


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane can hot-reload a new listener configuration created post-startup.
    """
    try:
        env_vars = {"LOG_LEVEL": "debug"}
        with VaneInstance(env_vars, "", debug_mode) as vane:
            if not wait_for_log(vane, STABLE_STARTUP_STRING, 10):
                return (
                    False,
                    "  └─ Details: Prerequisite failed - Vane did not initialize.",
                )

            log_len_after_startup = len(vane.captured_output)

            listener_dir = vane.tmpdir / "listener" / "[80]"
            listener_dir.mkdir(parents=True, exist_ok=True)
            # Use the corrected constant name
            (listener_dir / "tcp.toml").write_text(TOML_HTTP_SIMPLE)

            # Use the corrected constant name
            if not wait_for_log(
                vane, PORT_80_TCP_UP, 10, start_index=log_len_after_startup
            ):
                log_dump = "".join(vane.captured_output)
                reason = (
                    f"Config file created, but did not find '{PORT_80_TCP_UP}' log."
                )
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
