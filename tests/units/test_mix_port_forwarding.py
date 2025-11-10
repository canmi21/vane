# tests/units/test_mix_port_forwarding.py

import random
import socket
import time
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane can forward different protocols (HTTP, TLS) from a single
    source port to different backend destinations.
    """
    http_server = None
    tls_server = None
    try:
        vane_port = net_utils.find_available_tcp_port()
        http_backend_port = net_utils.find_available_tcp_port()
        tls_backend_port = net_utils.find_available_tcp_port()

        while len({vane_port, http_backend_port, tls_backend_port}) != 3:
            http_backend_port = net_utils.find_available_tcp_port()
            tls_backend_port = net_utils.find_available_tcp_port()

        p1, p2 = random.sample(range(1, 101), 2)

        toml_content = f"""
[[protocols]]
name = "tls"
priority = {p1}
detect = {{ method = "magic", pattern = "0x16" }}
destination = {{ type = "forward", forward = {{ strategy = "random", targets = [
    {{ ip = "127.0.0.1", port = {tls_backend_port} }},
] }} }}
[[protocols]]
name = "http"
priority = {p2}
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "random", targets = [
    {{ ip = "127.0.0.1", port = {http_backend_port} }},
] }} }}
"""
        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", http_backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()

        tls_server = net_utils.ConnectionRecorderTCPServer(
            ("127.0.0.1", tls_backend_port), net_utils.ConnectionRecorderHandler
        )
        tls_server.start()

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
                vane_port, ["GET"], ["/http_test"]
            )
            if not success:
                return (
                    False,
                    "  └─ Details: Sending HTTP request through Vane failed.",
                )

            tls_payload = b"\x16\x03\x01\x00\x55\x01\x00\x00\x51\x03\x03"
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.connect(("127.0.0.1", vane_port))
                s.sendall(tls_payload)

            time.sleep(1)

        # --- FINAL ASSERTIONS (Corrected) ---

        # 1. The HTTP backend MUST receive exactly one HTTP request.
        expected_http_requests = 1
        actual_http_requests = len(http_server.received_requests)
        if actual_http_requests != expected_http_requests:
            reason = (
                f"HTTP backend request count mismatch.\n"
                f"      ├─ Expected: {expected_http_requests}\n"
                f"      └─ Actual:   {actual_http_requests} (Received: {http_server.received_requests})"
            )
            return (False, f"  └─ Details: {reason}")

        # 2. The TLS backend MUST receive AT LEAST one connection.
        #    We check for > 0 to account for Vane's potential retry logic.
        expected_tls_connections_min = 1
        actual_tls_connections = tls_server.connection_count
        if actual_tls_connections < expected_tls_connections_min:
            reason = (
                f"TLS backend did not receive any connections.\n"
                f"      ├─ Expected: >= {expected_tls_connections_min}\n"
                f"      └─ Actual:   {actual_tls_connections}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if http_server:
            http_server.stop()
        if tls_server:
            tls_server.stop()

    return (True, "")
