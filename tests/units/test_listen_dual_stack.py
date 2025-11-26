# tests/units/test_listen_dual_stack.py

import random
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the LISTEN_IPV6 environment variable to ensure Vane binds to the
    correct IP stacks (IPv4-only vs. dual-stack).
    """
    # --- SCENARIO 1: DUAL-STACK ENABLED (LISTEN_IPV6=true) ---
    try:
        port1 = net_utils.find_available_udp_port()
        backend_port1 = net_utils.find_available_udp_port()
        priority1 = random.randint(1, 100)

        yaml_content1 = f"""
protocols:
  - name: dns
    priority: {priority1}
    detect:
      method: prefix
      pattern: "\\x00\\x01"
    destination:
      type: forward
      forward:
        strategy: serial
        targets:
          - ip: 127.0.0.1
            port: {backend_port1}
"""
        log_level = "debug" if debug_mode else "info"
        env_vars1 = {"LOG_LEVEL": log_level, "LISTEN_IPV6": "true"}

        vane1 = VaneInstance(env_vars1, "", debug_mode)
        (vane1.tmpdir / "listener" / f"[{port1}]").mkdir(parents=True, exist_ok=True)
        (vane1.tmpdir / "listener" / f"[{port1}]" / "udp.yaml").write_text(
            yaml_content1
        )

        with vane1:
            expected_log = f"IPv4 + IPv6 PORT {port1} UDP UP"
            if not wait_for_log(vane1, expected_log, 10):
                return (
                    False,
                    f"  └─ Details: Failed dual-stack test. Vane did not log '{expected_log}' when LISTEN_IPV6=true.",
                )
    except Exception as e:
        return (
            False,
            f"  └─ Details: An unexpected exception occurred during the dual-stack test: {e}",
        )

    # --- SCENARIO 2: IPV4-ONLY (DEFAULT, LISTEN_IPV6 not set) ---
    try:
        port2 = net_utils.find_available_udp_port()
        backend_port2 = net_utils.find_available_udp_port()
        priority2 = random.randint(1, 100)

        yaml_content2 = f"""
protocols:
  - name: dns
    priority: {priority2}
    detect:
      method: prefix
      pattern: "\\x00\\x01"
    destination:
      type: forward
      forward:
        strategy: serial
        targets:
          - ip: 127.0.0.1
            port: {backend_port2}
"""
        log_level = "debug" if debug_mode else "info"
        # LISTEN_IPV6 is intentionally not set to test the default behavior.
        env_vars2 = {"LOG_LEVEL": log_level}

        vane2 = VaneInstance(env_vars2, "", debug_mode)
        (vane2.tmpdir / "listener" / f"[{port2}]").mkdir(parents=True, exist_ok=True)
        (vane2.tmpdir / "listener" / f"[{port2}]" / "udp.yaml").write_text(
            yaml_content2
        )

        with vane2:
            expected_log = f"IPv4 PORT {port2} UDP UP"
            if not wait_for_log(vane2, expected_log, 10):
                return (
                    False,
                    f"  └─ Details: Failed IPv4-only test. Vane did not log '{expected_log}' when LISTEN_IPV6 was not set.",
                )

            # Perform a negative assertion to be absolutely sure.
            found_line = ""
            for line in vane2.captured_output:
                if expected_log in line:
                    found_line = line
                    break

            if "IPv6" in found_line:
                return (
                    False,
                    f"  └─ Details: Failed IPv4-only test. Vane logged '{found_line.strip()}', which incorrectly contains 'IPv6', when LISTEN_IPV6 was not set.",
                )
    except Exception as e:
        return (
            False,
            f"  └─ Details: An unexpected exception occurred during the IPv4-only test: {e}",
        )

    return (True, "")
