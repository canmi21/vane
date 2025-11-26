# tests/units/test_no_available_targets.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that Vane logs "No available targets" when a request is sent but
    the only configured backend target is down (port is not listening).
    """
    try:
        # --- Preparation ---
        # Find two available ports. One for Vane's listener and one for the
        # non-existent backend. By not starting a server on the backend port,
        # we guarantee it's unavailable.
        vane_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        EXPECTED_LOG_STRING = "No available targets"

        toml_content = f"""
[[protocols]]
name = "http"
priority = 1
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
    {{ ip = "127.0.0.1", port = {backend_port} }}
]}} }}
"""
        # --- Configure and Start Vane ---
        # We must set the log level to at least 'info' to ensure the
        # target log message is captured in non-debug runs.
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.toml").write_text(
            toml_content
        )

        with vane:
            # 1. First, wait for Vane's listener to be up and running.
            up_string = f"PORT {vane_port} TCP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            # 2. Send a request to trigger the connection attempt to the dead backend.
            # The request itself is expected to fail from the client's perspective,
            # so we can ignore the success status.
            http_utils.send_test_requests(vane_port, ["GET"], ["/trigger"])

            # 3. Now, wait for the specific log message we expect Vane to produce.
            if not wait_for_log(vane, EXPECTED_LOG_STRING, 10):
                reason = (
                    "Vane did not log the expected message after failing to connect.\n"
                    f'      └─ Expected Log: "{EXPECTED_LOG_STRING}"'
                )
                return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
