# tests/units/test_node_forwarding.py

import random
import socket
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that Vane can correctly forward traffic to a backend defined in a
    separate `nodes.yml` file, using the `node:` key in a listener config.
    """
    dns_server = None
    try:
        # --- Test Configuration ---
        DNS_QUERY = (
            b"\x00\x01\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
            b"\x07example\x03com\x00\x00\x01\x00\x01"
        )
        DNS_RESPONSE = (
            b"\x00\x01\x81\x80\x00\x01\x00\x01\x00\x00\x00\x00"
            b"\x07example\x03com\x00\x00\x01\x00\x01"
            b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\xff\x00\x04\x5d\xb8\xd8\x22"
        )
        priority = random.randint(1, 100)

        # --- Port and Server Setup ---
        # A random port is used for the backend to ensure test reliability.
        vane_port = net_utils.find_available_udp_port()
        backend_port = net_utils.find_available_udp_port()

        # --- Start Backend Server ---
        dns_server = net_utils.ResponseUDPServer(
            ("127.0.0.1", backend_port),
            net_utils.PredefinedResponseUDPHandler,
            response_data=DNS_RESPONSE,
        )
        dns_server.start()

        # --- Vane Configuration ---
        # 1. Listener configuration that references a logical node name.
        listener_yaml = f"""
protocols:
  - name: dns
    priority: {priority}
    detect:
      method: prefix
      pattern: "\\x00\\x01"
    destination:
      type: forward
      forward:
        strategy: serial
        targets:
          - node: local
            port: {backend_port}
"""
        # 2. Node definition file that maps the logical name to a real IP.
        # This must be placed in Vane's main CONFIG_DIR.
        nodes_yaml = f"""
nodes:
  - name: local
    ips:
      - address: 127.0.0.1
        ports: [80, 443, {backend_port}]
        type: ipv4
      - address: "2001:db8::5"
        ports: [80, 443]
        type: ipv6
"""
        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        # Write the nodes.yml to the root of the config directory.
        (vane.tmpdir / "nodes.yml").write_text(nodes_yaml)

        # Write the listener config to its specific folder.
        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "udp.yaml").write_text(
            listener_yaml
        )

        with vane:
            up_string = f"PORT {vane_port} UDP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on UDP port {vane_port}.",
                )

            # --- Send Test Traffic ---
            received_data = None
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client_socket:
                client_socket.settimeout(5)
                client_socket.sendto(DNS_QUERY, ("127.0.0.1", vane_port))
                try:
                    received_data, _ = client_socket.recvfrom(1024)
                except socket.timeout:
                    pass

        # --- Final Assertions ---
        if received_data != DNS_RESPONSE:
            reason = (
                f"Failed to forward UDP packet using 'node:' destination.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Vane Listener: UDP {vane_port}\n"
                f"      │  ├─ Target Node:   'local' on port {backend_port}\n"
                f"      │  └─ Resolved IP:   127.0.0.1 (from nodes.yml)\n"
                f"      └─ Result\n"
                f"         ├─ Expected Response: {DNS_RESPONSE.hex()}\n"
                f"         └─ Actual Response:   {received_data.hex() if received_data else 'None'}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if dns_server:
            dns_server.stop()

    return (True, "")
