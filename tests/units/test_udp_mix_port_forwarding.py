# tests/units/test_udp_mix_port_forwarding.py

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
    Tests that Vane can correctly route different types of UDP traffic from a
    single listener port to different backends based on protocol detection rules.
    """
    dns_backend = None
    quic_backend = None
    try:
        # --- Test Configuration ---
        NUM_PACKETS_PER_TYPE = 10
        # This DNS query must start with \x00\x01 to match the 'prefix' rule.
        DNS_QUERY = (
            b"\x00\x01\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
            b"\x07example\x03com\x00\x00\x01\x00\x01"
        )
        # This simulated QUIC packet must start with \xc3 to match the 'magic' rule.
        QUIC_LIKE_PACKET = b"\xc3" + os.urandom(24)

        # Ensure priorities are unique to make the test deterministic.
        priorities = random.sample(range(1, 101), 2)
        dns_prio = priorities[0]
        quic_prio = priorities[1]

        # --- Port and Server Setup ---
        ports = set()
        while len(ports) < 3:
            ports.add(net_utils.find_available_udp_port())

        vane_port = ports.pop()
        backend_ports = sorted(list(ports))
        backend_port_dns = backend_ports[0]
        backend_port_quic = backend_ports[1]

        # --- Start Backend Servers ---
        dns_backend = net_utils.PacketRecorderUDPServer(
            ("127.0.0.1", backend_port_dns),
            net_utils.PacketRecorderUDPHandler,
        )
        dns_backend.start()

        quic_backend = net_utils.PacketRecorderUDPServer(
            ("127.0.0.1", backend_port_quic),
            net_utils.PacketRecorderUDPHandler,
        )
        quic_backend.start()

        # --- Vane Configuration ---
        yaml_content = f"""
protocols:
  - name: dns
    priority: {dns_prio}
    detect:
      method: prefix
      pattern: "\\x00\\x01"
    destination:
      type: forward
      forward:
        strategy: serial
        targets:
          - ip: 127.0.0.1
            port: {backend_port_dns}
  - name: quic
    priority: {quic_prio}
    detect:
      method: magic
      pattern: "0xc3"
    destination:
      type: forward
      forward:
        strategy: serial
        targets:
          - ip: 127.0.0.1
            port: {backend_port_quic}
"""
        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
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

            # --- Send Mixed Traffic ---
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                # Interleave the packets to make the test more robust.
                for _ in range(NUM_PACKETS_PER_TYPE):
                    client.sendto(DNS_QUERY, ("127.0.0.1", vane_port))
                    time.sleep(0.01)
                    client.sendto(QUIC_LIKE_PACKET, ("127.0.0.1", vane_port))
                    time.sleep(0.01)

            # Allow Vane time to process the final packets before shutdown.
            time.sleep(0.5)

        # --- Final Assertions ---
        dns_hits = dns_backend.packet_count
        quic_hits = quic_backend.packet_count

        if dns_hits != NUM_PACKETS_PER_TYPE or quic_hits != NUM_PACKETS_PER_TYPE:
            reason = (
                f"UDP mix-port forwarding failed to segregate traffic correctly.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Vane Listener: UDP {vane_port}\n"
                f"      │  ├─ DNS Backend:     UDP {backend_port_dns}\n"
                f"      │  └─ QUIC Backend:    UDP {backend_port_quic}\n"
                f"      └─ Packet Distribution\n"
                f"         ├─ DNS Backend Received:  {dns_hits} (Expected: {NUM_PACKETS_PER_TYPE})\n"
                f"         └─ QUIC Backend Received: {quic_hits} (Expected: {NUM_PACKETS_PER_TYPE})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if dns_backend:
            dns_backend.stop()
        if quic_backend:
            quic_backend.stop()

    return (True, "")
