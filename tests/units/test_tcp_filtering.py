# tests/units/test_tcp_filtering.py

import random
import socket
import time
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane's TCP proxy correctly filters traffic based on detection rules,
    forwarding HTTP while dropping other traffic like TLS.
    """
    backend_server = None
    try:
        # --- Preparation: Find free ports for Vane and the backend server ---
        vane_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()
        while vane_port == backend_port:
            backend_port = net_utils.find_available_tcp_port()

        # --- Define Traffic Payloads ---
        # A valid HTTP request that SHOULD be forwarded
        http_payload = (
            b"GET /test HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
        )
        # A fake TLS Client Hello packet that SHOULD be dropped (starts with 0x16)
        tls_payload = b"\x16\x03\x01\x00\x55\x01\x00\x00\x51\x03\x03"

        # --- Start Backend Server ---
        backend_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        backend_server.start()

        # --- Configure and Start Vane ---
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

            # --- Send Traffic to Vane ---
            # Send the TLS packet first
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.connect(("127.0.0.1", vane_port))
                s.sendall(tls_payload)

            # Brief pause to ensure connections are handled separately
            time.sleep(0.5)

            # Send the HTTP packet second
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.connect(("127.0.0.1", vane_port))
                s.sendall(http_payload)

            # Wait for any potential proxied traffic to be processed
            time.sleep(1)

        # --- Final Assertion ---
        received = backend_server.received_requests
        if len(received) != 1:
            reason = f"Expected backend to receive 1 request, but it received {len(received)}."
            return (False, f"  └─ Details: {reason}\n      └─ Received: {received}")

        if received[0]["method"] != "GET" or received[0]["path"] != "/test":
            reason = "The request received by the backend did not match the HTTP request sent."
            return (False, f"  └─ Details: {reason}\n      └─ Received: {received[0]}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if backend_server:
            backend_server.stop()

    return (True, "")
