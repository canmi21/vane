# tests/units/test_custom_resolver.py

import random
import socket
import time
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that Vane can use a custom DNS resolver (specified via environment
    variables) to resolve a `domain:` target.
    """
    custom_resolver = None
    final_backend = None
    try:
        # --- Test Configuration ---
        TARGET_DOMAIN = "example.com"
        RESOLVED_IP = "127.0.0.1"
        # This is the payload for the *final service*, not a DNS query.
        # It just needs to match the Vane listener's detection rule.
        SERVICE_QUERY = b"\x00\x01" + b"some service data"
        priority = random.randint(1, 100)

        # --- Port and Server Setup ---
        vane_port = net_utils.find_available_udp_port()
        backend_port = net_utils.find_available_udp_port()
        resolver_port = net_utils.find_available_udp_port()

        # --- Start Backend Servers ---
        # 1. The custom DNS resolver. Its job is to resolve TARGET_DOMAIN.
        custom_resolver = net_utils.CustomDnsServer(
            ("127.0.0.1", resolver_port),
            net_utils.CustomDnsHandler,
            a_records={TARGET_DOMAIN: RESOLVED_IP},
        )
        custom_resolver.start()

        # 2. The final destination server that the domain resolves to.
        final_backend = net_utils.PacketRecorderUDPServer(
            (RESOLVED_IP, backend_port),
            net_utils.PacketRecorderUDPHandler,
        )
        final_backend.start()

        # --- Vane Configuration ---
        # The listener targets a domain name.
        listener_yaml = f"""
protocols:
  - name: service
    priority: {priority}
    detect:
      method: prefix
      pattern: "\\x00\\x01"
    destination:
      type: forward
      forward:
        strategy: serial
        targets:
          - domain: {TARGET_DOMAIN}
            port: {backend_port}
"""
        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        # Vane is told to use our custom DNS server instead of the system's.
        env_vars = {
            "LOG_LEVEL": log_level,
            "NAMESERVER1": "127.0.0.1",
            "NAMESERVER1_PORT": str(resolver_port),
        }

        vane = VaneInstance(env_vars, "", debug_mode)
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

            # A longer sleep to allow Vane to perform its initial DNS resolution.
            time.sleep(1)

            # --- Send Test Traffic to the Service ---
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                client.sendto(SERVICE_QUERY, ("127.0.0.1", vane_port))

            # Allow time for the forwarded packet to be processed.
            time.sleep(0.5)

        # --- Final Assertions ---
        packets_received = final_backend.packet_count
        if packets_received != 1:
            reason = (
                f"Failed to forward packet using custom DNS resolver.\n"
                f"      \n"
                f"      ├─ Test Flow\n"
                f"      │  ├─ Vane Listener:      UDP {vane_port}\n"
                f"      │  ├─ Custom DNS Resolver:  UDP {resolver_port} (for '{TARGET_DOMAIN}')\n"
                f"      │  └─ Final Backend:        UDP {RESOLVED_IP}:{backend_port}\n"
                f"      └─ Result\n"
                f"         ├─ Packets Sent:     1\n"
                f"         └─ Packets Received: {packets_received}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if custom_resolver:
            custom_resolver.stop()
        if final_backend:
            final_backend.stop()

    return (True, "")
