# tests/units/test_udp_strategy_serial.py

import random
import socket
import time
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the UDP 'serial' forwarding strategy. It verifies that Vane correctly
    maps distinct client sessions (source IP:port tuples) to distinct backends
    in a sequential and sticky manner.
    """
    backend_server_A = None
    backend_server_B = None
    try:
        # --- Test Configuration ---
        NUM_PACKETS_PER_CLIENT = 10
        DNS_QUERY = (
            b"\xab\xcd\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
            b"\x04vane\x05proxy\x00\x00\x01\x00\x01"
        )
        priority = random.randint(1, 100)

        # --- Port and Server Setup ---
        # We need three ports: one for Vane, two for the backends.
        ports = set()
        while len(ports) < 3:
            ports.add(net_utils.find_available_udp_port())

        vane_port = ports.pop()
        backend_ports = sorted(list(ports))
        backend_port_A = backend_ports[0]
        backend_port_B = backend_ports[1]

        # --- Start Backend Servers ---
        backend_server_A = net_utils.PacketRecorderUDPServer(
            ("127.0.0.1", backend_port_A),
            net_utils.PacketRecorderUDPHandler,
        )
        backend_server_A.start()

        backend_server_B = net_utils.PacketRecorderUDPServer(
            ("127.0.0.1", backend_port_B),
            net_utils.PacketRecorderUDPHandler,
        )
        backend_server_B.start()

        # --- Vane Configuration ---
        yaml_content = f"""
protocols:
  - name: any
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
            port: {backend_port_A}
          - ip: 127.0.0.1
            port: {backend_port_B}
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

            # --- Send Traffic from Two Distinct Clients ---
            # To test the serial strategy for UDP, we must use two separate
            # sockets, as Vane defines a session by the source IP:port tuple.

            # Client A sends its burst of packets.
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client_A:
                for _ in range(NUM_PACKETS_PER_CLIENT):
                    client_A.sendto(DNS_QUERY, ("127.0.0.1", vane_port))
                    time.sleep(0.01)  # Pace sends to avoid OS buffer drops.

            # Client B sends its burst of packets.
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client_B:
                for _ in range(NUM_PACKETS_PER_CLIENT):
                    client_B.sendto(DNS_QUERY, ("127.0.0.1", vane_port))
                    time.sleep(0.01)

            # Allow Vane time to process the final packets before shutdown.
            time.sleep(0.5)

        # --- Final Assertions ---
        hits_on_A = backend_server_A.packet_count
        hits_on_B = backend_server_B.packet_count

        if hits_on_A != NUM_PACKETS_PER_CLIENT or hits_on_B != NUM_PACKETS_PER_CLIENT:
            reason = (
                f"UDP 'serial' strategy failed to distribute traffic correctly.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Vane Listener: UDP {vane_port}\n"
                f"      │  ├─ Backend A:     UDP {backend_port_A} (Expected for Client A)\n"
                f"      │  └─ Backend B:     UDP {backend_port_B} (Expected for Client B)\n"
                f"      └─ Packet Distribution\n"
                f"         ├─ Backend A Received: {hits_on_A} (Expected: {NUM_PACKETS_PER_CLIENT})\n"
                f"         └─ Backend B Received: {hits_on_B} (Expected: {NUM_PACKETS_PER_CLIENT})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if backend_server_A:
            backend_server_A.stop()
        if backend_server_B:
            backend_server_B.stop()

    return (True, "")
