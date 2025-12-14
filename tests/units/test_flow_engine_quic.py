# tests/units/test_flow_engine_quic.py

import socket
import time
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, quic_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the Flow Engine for QUIC L4+ Upgrade.
    Verifies that Vane can:
    1. Accept UDP packets.
    2. Upgrade to 'quic' carrier.
    3. Parse QUIC Headers (DCID, SCID).
    4. Route based on QUIC metadata.
    """
    quic_backend = None

    try:
        # --- Port Setup ---
        vane_port = net_utils.find_available_udp_port()
        backend_port = net_utils.find_available_udp_port()

        # --- Start UDP Backend Server ---
        quic_backend = net_utils.PacketRecorderUDPServer(
            ("127.0.0.1", backend_port),
            net_utils.PacketRecorderUDPHandler,
        )
        quic_backend.start()

        # --- Construct QUIC Packets ---
        # Packet 1: Valid DCID (should be routed)
        valid_dcid = bytes.fromhex("1122334455667788")  # Matches config
        valid_packet = quic_utils.create_quic_initial_packet(valid_dcid, b"src", b"")

        # Packet 2: Invalid DCID (should be aborted)
        invalid_dcid = bytes.fromhex("aabbccddeeff0011")
        invalid_packet = quic_utils.create_quic_initial_packet(
            invalid_dcid, b"src", b""
        )

        # --- Vane Configuration ---
        # L4 Listener: Upgrade all UDP to QUIC (for this test)
        l4_yaml = """
connection:
  internal.transport.upgrade:
    input:
      protocol: "quic"
"""
        # L4+ Resolver: Route based on DCID
        # In the future, this will be {{quic.sni}}
        resolver_yaml = f"""
connection:
  internal.common.match:
    input:
      left: "{{{{quic.dcid}}}}"
      right: "1122334455667788"
      operator: "eq"
    output:
      "true":
        internal.transport.proxy:
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
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "udp.yaml").write_text(l4_yaml)

        (vane.tmpdir / "resolver").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "resolver" / "quic.yaml").write_text(resolver_yaml)

        with vane:
            up_string = f"PORT {vane_port} UDP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on UDP port {vane_port}.",
                )

            # --- Send Traffic ---
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                # 1. Send Valid Packet (x5)
                for _ in range(5):
                    client.sendto(valid_packet, ("127.0.0.1", vane_port))
                    time.sleep(0.02)

                # 2. Send Invalid Packet (x5)
                for _ in range(5):
                    client.sendto(invalid_packet, ("127.0.0.1", vane_port))
                    time.sleep(0.02)

            time.sleep(0.5)

        # --- Final Assertions ---
        packets_received = quic_backend.packet_count

        if packets_received != 5:
            return (
                False,
                f"  └─ Details: Backend received {packets_received} packets, expected 5 (Valid DCID only).",
            )

    except Exception as e:
        return (False, f"  └─ Details: Unexpected exception: {e}")
    finally:
        if quic_backend:
            quic_backend.stop()

    return (True, "")
