# tests/units/test_udp_fallback.py

import random
import socket
import time
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that a UDP fallback rule correctly captures and forwards a burst of
    UDP traffic without packet loss.
    """
    fallback_server = None
    try:
        # --- Test Configuration ---
        NUM_PACKETS_TO_SEND = 20
        TOTAL_PACKETS = NUM_PACKETS_TO_SEND

        # A standard DNS query.
        DNS_QUERY = (
            b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
            b"\x07example\x03com\x00\x00\x01\x00\x01"
        )
        priority = random.randint(1, 100)

        # --- Port and Server Setup ---
        vane_port = net_utils.find_available_udp_port()
        backend_port = net_utils.find_available_udp_port()

        # --- Start Backend Server ---
        fallback_server = net_utils.PacketRecorderUDPServer(
            ("127.0.0.1", backend_port),
            net_utils.PacketRecorderUDPHandler,
        )
        fallback_server.start()

        # --- Vane Configuration ---
        yaml_content = f"""
protocols:
  - name: all
    priority: {priority}
    detect:
      method: fallback
      pattern: any
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

            # --- Send Traffic ---
            # The only robust way to send a burst of UDP packets without triggering
            # silent drops from the OS kernel is to use a new socket for each
            # packet. This simulates distinct clients and gives each send its
            # own clean network buffer.
            for _ in range(TOTAL_PACKETS):
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                    client.sendto(DNS_QUERY, ("127.0.0.1", vane_port))

            # This sleep is still necessary to ensure the *last* packet
            # is processed by Vane before the process is terminated.
            time.sleep(0.5)

        # --- Final Assertions ---
        packets_received = fallback_server.packet_count
        if packets_received != TOTAL_PACKETS:
            reason = (
                f"Fallback backend did not receive the correct number of UDP packets.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Vane Listener: UDP {vane_port}\n"
                f"      │  └─ Backend:       UDP {backend_port}\n"
                f"      └─ Packet Count\n"
                f"         ├─ Sent:     {TOTAL_PACKETS}\n"
                f"         └─ Received: {packets_received}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if fallback_server:
            fallback_server.stop()

    return (True, "")
