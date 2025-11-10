# tests/units/test_protocol_priority.py

import random
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane correctly forwards traffic based on protocol priority,
    where a lower number means higher priority.
    """
    high_prio_server = None
    low_prio_server = None
    try:
        # --- Preparation ---
        vane_port = net_utils.find_available_tcp_port()
        high_prio_backend_port = net_utils.find_available_tcp_port()
        low_prio_backend_port = net_utils.find_available_tcp_port()

        while len({vane_port, high_prio_backend_port, low_prio_backend_port}) != 3:
            high_prio_backend_port = net_utils.find_available_tcp_port()
            low_prio_backend_port = net_utils.find_available_tcp_port()

        p1, p2 = random.sample(range(1, 101), 2)
        high_prio, low_prio = min(p1, p2), max(p1, p2)

        # --- THIS SECTION IS CORRECTED ---
        # The regex pattern was fixed from "[A_Z]" to "[A-Z]".
        toml_content = f"""
[[protocols]]
name = "http1"
priority = {low_prio}
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "random", targets = [
    {{ ip = "127.0.0.1", port = {low_prio_backend_port} }},
] }} }}

[[protocols]]
name = "http2"
priority = {high_prio}
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "random", targets = [
    {{ ip = "127.0.0.1", port = {high_prio_backend_port} }},
] }} }}
"""
        # --- End of Correction ---

        # --- Start Backend Servers ---
        high_prio_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", high_prio_backend_port), http_utils.RequestRecorderHandler
        )
        high_prio_server.start()

        low_prio_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", low_prio_backend_port), http_utils.RequestRecorderHandler
        )
        low_prio_server.start()

        # --- Configure and Start Vane ---
        env_vars = {"LOG_LEVEL": "debug"}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.toml").write_text(
            toml_content
        )

        with vane:
            up_string = f"PORT {vane_port} TCP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            success, _ = http_utils.send_test_requests(
                vane_port, ["GET"], ["/priority_check"]
            )
            if not success:
                return (
                    False,
                    "  └─ Details: Sending HTTP request through Vane failed.",
                )

        # --- Final Assertions ---
        high_prio_received = len(high_prio_server.received_requests)
        low_prio_received = len(low_prio_server.received_requests)

        if high_prio_received != 1 or low_prio_received != 0:
            reason = (
                f"Request distribution by priority was incorrect.\n"
                f"      ├─ High Prio Server (p={high_prio}, port={high_prio_backend_port}): Expected 1, Got {high_prio_received}\n"
                f"      └─ Low Prio Server  (p={low_prio}, port={low_prio_backend_port}): Expected 0, Got {low_prio_received}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if high_prio_server:
            high_prio_server.stop()
        if low_prio_server:
            low_prio_server.stop()

    return (True, "")
