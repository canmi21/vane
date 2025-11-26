# tests/units/test_tcp_proxy.py

import random
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the TCP transparent proxy functionality.
    """
    backend_server = None
    try:
        # --- Preparation ---
        vane_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()
        while vane_port == backend_port:
            backend_port = net_utils.find_available_tcp_port()

        priority = random.randint(1, 100)

        toml_content = f"""
[[protocols]]
name = "http"
priority = {priority}
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "random", targets = [
    {{ ip = "127.0.0.1", port = {backend_port} }},
] }} }}
"""
        # --- Backend Server Sanity Check ---
        backend_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        backend_server.start()

        methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
        paths = ["/", f"/{random.randint(1000, 9999)}"]

        # Verify backend server works directly
        success, sent_requests = http_utils.send_test_requests(
            backend_port, methods, paths
        )
        if not success or sent_requests != backend_server.received_requests:
            return (
                False,
                "  └─ Details: Prerequisite failed - backend HTTP server is not responding correctly.",
            )

        # Reset for the main test
        backend_server.received_requests.clear()

        # --- Main Test: Vane as Proxy ---
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

            # Send requests through Vane
            success, sent_requests = http_utils.send_test_requests(
                vane_port, methods, paths
            )
            if not success:
                return (False, "  └─ Details: Requests sent through Vane failed.")

        # --- Final Assertion ---
        if sent_requests != backend_server.received_requests:
            reason = (
                "Request mismatch between client and server.\n"
                f"      ├─ Client Sent: {sent_requests}\n"
                f"      └─ Server Rcvd: {backend_server.received_requests}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if backend_server:
            backend_server.stop()

    return (True, "")
