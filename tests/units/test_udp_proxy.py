# tests/units/test_udp_proxy.py

import random
import socket
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that Vane can correctly proxy UDP traffic based on a prefix match,
    using a YAML configuration file. A simple DNS query is used as the test
    protocol.
    """
    dns_server = None
    try:
        # --- Test Configuration ---
        # A minimal DNS query for "example.com" with transaction ID 0x0001.
        # This query must start with \x00\x01 to match the Vane detection rule.
        DNS_QUERY = (
            b"\x00\x01\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
            b"\x07example\x03com\x00\x00\x01\x00\x01"
        )
        # A corresponding DNS response for "example.com" -> 93.184.216.34
        DNS_RESPONSE = (
            b"\x00\x01\x81\x80\x00\x01\x00\x01\x00\x00\x00\x00"
            b"\x07example\x03com\x00\x00\x01\x00\x01"
            b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\xff\x00\x04\x5d\xb8\xd8\x22"
        )
        priority = random.randint(1, 100)

        # --- Port and Server Setup ---
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
        # This test intentionally uses YAML to ensure its parsing of byte
        # escape sequences is also robust.
        yaml_content = f"""
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
          - ip: 127.0.0.1
            port: {backend_port}
"""
        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        # Write the configuration to a .yaml file as per the test requirement.
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "udp.yaml").write_text(
            yaml_content
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
                    # If a timeout occurs, received_data will remain None.
                    pass

        # --- Final Assertions ---
        if received_data is None:
            return (False, "  └─ Details: Did not receive any response from Vane.")

        if received_data != DNS_RESPONSE:
            reason = (
                f"Received UDP payload did not match the expected payload.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Vane Listener: UDP {vane_port}\n"
                f"      │  └─ Backend DNS:   UDP {backend_port}\n"
                f"      ├─ Expected Payload (hex)\n"
                f"      │  └─ {DNS_RESPONSE.hex()}\n"
                f"      └─ Received Payload (hex)\n"
                f"         └─ {received_data.hex()}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if dns_server:
            dns_server.stop()

    return (True, "")
