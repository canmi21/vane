# tests/units/test_proxy_node.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log
from .flow_configs import node_proxy
import secrets


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'internal.transport.proxy.node' plugin.
    It verifies that traffic is correctly routed to a backend defined in nodes.yaml.
    """
    http_server = None
    try:
        # --- Test Configuration ---
        NUM_REQUESTS = 5
        NODE_NAME = "backend-node-01"

        # --- Port and Server Setup ---
        vane_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()
        access_token = secrets.token_hex(16)

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
        # 1. Generate the Nodes configuration (nodes.yml)
        nodes_yaml = f"""
nodes:
  - name: "{NODE_NAME}"
    ips:
      - address: "127.0.0.1"
        ports: [{backend_port}]
        type: ipv4
"""
        # 2. Generate the Flow configuration
        flow_yaml = node_proxy.generate_config(NODE_NAME, backend_port)

        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {
            "LOG_LEVEL": log_level,
            "ACCESS_TOKEN": access_token,
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        # Write nodes.yml to the root config directory (vane.tmpdir)
        (vane.tmpdir / "nodes.yml").write_text(nodes_yaml)

        # Write the listener config
        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.yaml").write_text(flow_yaml)

        with vane:
            # Wait for Vane to load nodes and start the port
            if not wait_for_log(vane, f"PORT {vane_port} TCP UP", 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            # --- Send Traffic ---
            http_utils.send_test_requests(
                vane_port, ["GET"], ["/node-test"] * NUM_REQUESTS
            )

        # --- Final Assertions ---
        http_hits = len(http_server.received_requests)

        if http_hits != NUM_REQUESTS:
            reason = (
                f"Node Proxy failed to route traffic.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Node Name:    {NODE_NAME}\n"
                f"      │  ├─ Resolved IP:  127.0.0.1\n"
                f"      │  └─ Backend Port: {backend_port}\n"
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
