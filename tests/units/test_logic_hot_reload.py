# tests/units/test_logic_hot_reload.py

import random
import time
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils, tls_utils
from .config_utils import wait_for_log


def generate_toml_config(http_target_port: int, tls_target_port: int) -> str:
    """Generates a TOML config routing HTTP and TLS to specific ports."""
    http_prio, tls_prio = random.sample(range(1, 101), 2)
    return f"""
[[protocols]]
name = "http"
priority = {http_prio}
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
    {{ ip = "127.0.0.1", port = {http_target_port} }}
]}} }}

[[protocols]]
name = "tls"
priority = {tls_prio}
detect = {{ method = "magic", pattern = "0x16" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
    {{ ip = "127.0.0.1", port = {tls_target_port} }}
]}} }}
"""


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that Vane can hot-reload a listener's configuration file and apply
    new routing logic without restarting the port.
    """
    backend_A_server = None
    backend_B_server = None
    try:
        # --- Test Configuration ---
        NUM_REQUESTS_PER_PHASE = 5
        HOT_RELOAD_DELAY_SECS = 3

        # --- Port and Server Setup ---
        ports = set()
        while len(ports) < 3:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = sorted(list(ports))
        backend_port_A = backend_ports[0]
        backend_port_B = backend_ports[1]

        vane = VaneInstance({}, "", debug_mode)
        cert_path = vane.tmpdir / "cert.pem"
        key_path = vane.tmpdir / "key.pem"
        tls_utils.generate_self_signed_cert(cert_path, key_path)

        # --- Start Backend Servers ---
        # Backend A is a pure HTTP server.
        backend_A_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port_A), http_utils.RequestRecorderHandler
        )
        backend_A_server.start()
        # Backend B is a pure TLS server.
        backend_B_server = net_utils.TLSConnectionRecorderServer(
            ("127.0.0.1", backend_port_B),
            net_utils.ConnectionRecorderHandler,
            cert_path,
            key_path,
        )
        backend_B_server.start()
        if not (
            net_utils.wait_for_tcp_port_ready(backend_port_A)
            and net_utils.wait_for_tcp_port_ready(backend_port_B)
        ):
            return (False, "  └─ Details: One or more backend servers failed to start.")

        # --- Vane Configuration and Startup ---
        config_v1 = generate_toml_config(
            http_target_port=backend_port_A, tls_target_port=backend_port_B
        )
        config_v2 = generate_toml_config(
            http_target_port=backend_port_B, tls_target_port=backend_port_A
        )

        listener_dir = vane.tmpdir / "listener" / f"[{vane_port}]"
        listener_dir.mkdir(parents=True, exist_ok=True)
        config_path = listener_dir / "tcp.toml"
        config_path.write_text(config_v1)

        with vane:
            # --- PHASE 1: Verify Initial State ---
            if not wait_for_log(vane, f"PORT {vane_port} TCP UP", 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            http_utils.send_test_requests(
                vane_port, ["GET"], ["/"] * NUM_REQUESTS_PER_PHASE
            )
            from units.test_capture_all_fallback import send_tls_connections

            send_tls_connections(vane_port, NUM_REQUESTS_PER_PHASE)
            time.sleep(0.5)

            # --- PHASE 2: Hot-Reload ---
            config_path.write_text(config_v2)
            time.sleep(HOT_RELOAD_DELAY_SECS)

            # --- PHASE 3: Verify New State ---
            # This traffic is expected to fail at the transport layer, as it's
            # being routed to incompatible servers. We wrap it in a try-except
            # block to prevent these expected errors from crashing the test.
            try:
                http_utils.send_test_requests(
                    vane_port, ["GET"], ["/"] * NUM_REQUESTS_PER_PHASE
                )
                send_tls_connections(vane_port, NUM_REQUESTS_PER_PHASE)
            except Exception:
                # Errors are expected here and can be ignored.
                pass
            time.sleep(0.5)

        # --- Final Assertions ---
        # The test's success is determined by the state after Phase 1. The fact
        # that the counters did *not* increase in Phase 3 proves that the routing
        # logic was successfully reloaded and traffic was sent elsewhere.
        hits_on_A = len(backend_A_server.received_requests)
        hits_on_B = backend_B_server.connection_count

        if hits_on_A != NUM_REQUESTS_PER_PHASE or hits_on_B != NUM_REQUESTS_PER_PHASE:
            reason = (
                f"Logic hot-reload test failed.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Phase 1: HTTP -> Backend A, TLS -> Backend B (Should succeed)\n"
                f"      │  └─ Phase 2: HTTP -> Backend B, TLS -> Backend A (Should be rerouted and fail)\n"
                f"      └─ Final Packet Count (Should only reflect successful Phase 1 traffic)\n"
                f"         ├─ Backend A (HTTP) Received: {hits_on_A} (Expected: {NUM_REQUESTS_PER_PHASE})\n"
                f"         └─ Backend B (TLS)  Received: {hits_on_B} (Expected: {NUM_REQUESTS_PER_PHASE})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if backend_A_server:
            backend_A_server.stop()
        if backend_B_server:
            backend_B_server.stop()

    return (True, "")
