# tests/units/test_flow_engine_udp.py

import random
import socket
import time
import os
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the Flow Engine for UDP, verifying that it can detect a specific
    protocol (DNS) and route it to a backend, while aborting other traffic.
    """
    dns_backend = None
    try:
        # --- Test Configuration ---
        NUM_DNS_PACKETS = 10
        NUM_QUIC_PACKETS = 10  # This traffic should be dropped

        # A standard DNS query, which must match the 'dns' method's pattern.
        DNS_QUERY = (
            b"\x00\x01\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
            b"\x07example\x03com\x00\x00\x01\x00\x01"
        )
        # A simulated QUIC packet that will not match the DNS pattern.
        QUIC_LIKE_PACKET = b"\xc3" + os.urandom(24)

        # --- Port and Server Setup ---
        vane_port = net_utils.find_available_udp_port()
        backend_port = net_utils.find_available_udp_port()

        # --- Start Backend Server ---
        dns_backend = net_utils.PacketRecorderUDPServer(
            ("127.0.0.1", backend_port),
            net_utils.PacketRecorderUDPHandler,
        )
        dns_backend.start()

        # --- Vane Configuration ---
        flow_yaml = f"""
connection:
  internal.protocol.detect:
    input:
      method: "dns"
      payload: "{{{{req.peek_buffer_hex}}}}"
    output:
      "true":
        internal.transport.proxy.transparent:
          input:
            target.ip: "127.0.0.1"
            target.port: {backend_port}
      "false":
        internal.transport.abort:
          input: {{}}
"""
        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "udp.yaml").write_text(flow_yaml)

        with vane:
            up_string = f"PORT {vane_port} UDP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on UDP port {vane_port}.",
                )

            # --- Send Mixed Traffic ---
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                # Send DNS packets, which should be proxied.
                for _ in range(NUM_DNS_PACKETS):
                    client.sendto(DNS_QUERY, ("127.0.0.1", vane_port))
                    time.sleep(0.01)

                # Send QUIC packets, which should be aborted by the flow.
                for _ in range(NUM_QUIC_PACKETS):
                    client.sendto(QUIC_LIKE_PACKET, ("127.0.0.1", vane_port))
                    time.sleep(0.01)

            # Allow Vane time to process the final packets before shutdown.
            time.sleep(0.5)

        # --- Final Assertions ---
        dns_hits = dns_backend.packet_count

        # The core of the test: only DNS packets should have reached the backend.
        if dns_hits != NUM_DNS_PACKETS:
            reason = (
                f"UDP Flow engine did not correctly route traffic.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Vane Listener: Port {vane_port} (Flow Engine Mode)\n"
                f"      │  └─ DNS Backend:   Port {backend_port}\n"
                f"      ├─ Traffic Sent\n"
                f"      │  ├─ DNS Packets:  {NUM_DNS_PACKETS} (should be proxied)\n"
                f"      │  └─ QUIC Packets: {NUM_QUIC_PACKETS} (should be aborted)\n"
                f"      └─ Result\n"
                f"         └─ DNS Backend Received: {dns_hits} (Expected: {NUM_DNS_PACKETS})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if dns_backend:
            dns_backend.stop()

    return (True, "")
