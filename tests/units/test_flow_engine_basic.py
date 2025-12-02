# tests/units/test_flow_engine_basic.py

import socket
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log
from .flow_configs import http_detect_and_route


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the basic functionality of the new Flow Engine by using a 'connection'
    format configuration to route HTTP and non-HTTP traffic.
    """
    http_server = None
    try:
        # --- Test Configuration ---
        NUM_HTTP_REQUESTS = 10
        NUM_TLS_CONNECTIONS = 10  # This traffic should be rejected

        # --- Port and Server Setup ---
        vane_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        # --- Start Backend Server ---
        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(backend_port):
            return (
                False,
                f"  └─ Details: HTTP backend on port {backend_port} failed to start.",
            )

        # --- Vane Configuration ---
        flow_yaml = http_detect_and_route.generate_config(backend_port)

        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.yaml").write_text(flow_yaml)

        with vane:
            up_string = f"PORT {vane_port} TCP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            # --- Send Mixed Traffic ---
            # 1. Send HTTP requests, which should be accepted and forwarded.
            http_utils.send_test_requests(vane_port, ["GET"], ["/"] * NUM_HTTP_REQUESTS)

            # 2. Send generic TLS connections, which should be aborted by the flow.
            try:
                from units.test_capture_all_fallback import send_tls_connections

                send_tls_connections(vane_port, NUM_TLS_CONNECTIONS)
            except (ConnectionResetError, socket.error) as e:
                # This is the expected outcome. The 'internal.transport.abort'
                # plugin intentionally resets the connection, which manifests
                # as a 'Connection reset by peer' error on the client.
                # We catch and ignore it, as it signifies the 'false' branch
                # of the flow worked correctly.
                # We also catch socket.error for broader compatibility.
                if "Connection reset by peer" not in str(e) and e.errno != 54:
                    raise  # Re-raise if it's a different, unexpected error.
                pass

        # --- Final Assertions ---
        http_hits = len(http_server.received_requests)

        # The core of the test: only HTTP requests should have reached the backend.
        if http_hits != NUM_HTTP_REQUESTS:
            reason = (
                f"Flow engine did not correctly route traffic.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Vane Listener: Port {vane_port} (Flow Engine Mode)\n"
                f"      │  └─ HTTP Backend:  Port {backend_port}\n"
                f"      ├─ Traffic Sent\n"
                f"      │  ├─ HTTP Requests: {NUM_HTTP_REQUESTS} (should be proxied)\n"
                f"      │  └─ TLS Attempts:  {NUM_TLS_CONNECTIONS} (should be aborted)\n"
                f"      └─ Result\n"
                f"         └─ HTTP Backend Received: {http_hits} (Expected: {NUM_HTTP_REQUESTS})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if http_server:
            http_server.stop()

    return (True, "")
