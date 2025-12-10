# tests/units/test_proxy_domain.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log
from .flow_configs import domain_proxy


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'internal.transport.proxy.domain' plugin.
    It verifies that traffic is correctly routed using a domain name (lo.ill.li -> 127.0.0.1).
    """
    http_server = None
    try:
        # --- Test Configuration ---
        NUM_REQUESTS = 5
        # This domain is a public wildcard DNS service that resolves to 127.0.0.1
        TARGET_DOMAIN = "lo.ill.li"

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
        flow_yaml = domain_proxy.generate_config(TARGET_DOMAIN, backend_port)

        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.yaml").write_text(flow_yaml)

        with vane:
            if not wait_for_log(vane, f"PORT {vane_port} TCP UP", 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            # --- Send Traffic ---
            http_utils.send_test_requests(
                vane_port, ["GET"], ["/domain-test"] * NUM_REQUESTS
            )

        # --- Final Assertions ---
        http_hits = len(http_server.received_requests)

        if http_hits != NUM_REQUESTS:
            reason = (
                f"Domain Proxy failed to route traffic.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Target Domain: {TARGET_DOMAIN}\n"
                f"      │  └─ Backend Port:  {backend_port}\n"
                f"      └─ Result\n"
                f"         └─ Backend Received: {http_hits} (Expected: {NUM_REQUESTS})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if http_server:
            http_server.stop()

    return (True, "")
