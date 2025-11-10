# tests/units/test_multi_port_binding.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import TOML_HTTP_SIMPLE, YAML_UDP, wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests dynamic multi-port binding for TCP and UDP, including hot-reload.
    """
    try:
        # --- Phase 1: Cold Start Preparation ---
        tcp_port1 = net_utils.find_available_tcp_port()
        udp_port1 = net_utils.find_available_udp_port()

        if net_utils.is_tcp_port_taken(tcp_port1) or net_utils.is_udp_port_taken(
            udp_port1
        ):
            return (False, "  └─ Details: Port selected for test was already taken.")

        env_vars = {"LOG_LEVEL": "debug"}
        vane = VaneInstance(env_vars, "", debug_mode)

        # Create initial config files before starting Vane
        (vane.tmpdir / "listener" / f"[{tcp_port1}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{tcp_port1}]" / "tcp.toml").write_text(
            TOML_HTTP_SIMPLE
        )

        (vane.tmpdir / "listener" / f"[{udp_port1}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{udp_port1}]" / "udp.yaml").write_text(YAML_UDP)

        with vane:
            # --- Phase 1: Cold Start Verification ---
            tcp_up_string1 = f"PORT {tcp_port1} TCP UP"
            udp_up_string1 = f"PORT {udp_port1} UDP UP"

            if not wait_for_log(vane, tcp_up_string1, 10):
                return (
                    False,
                    f"  └─ Details: Cold start failed for TCP port {tcp_port1}.",
                )
            if not wait_for_log(vane, udp_up_string1, 5):
                return (
                    False,
                    f"  └─ Details: Cold start failed for UDP port {udp_port1}.",
                )

            if not net_utils.is_tcp_port_taken(
                tcp_port1
            ) or not net_utils.is_udp_port_taken(udp_port1):
                return (
                    False,
                    "  └─ Details: Vane logged ports as UP, but they are not occupied.",
                )

            log_len_after_cold_load = len(vane.captured_output)

            # --- Phase 2: Hot Reload Preparation ---
            port2 = net_utils.find_available_tcp_port()
            while port2 in [tcp_port1, udp_port1]:  # Ensure it's a new port
                port2 = net_utils.find_available_tcp_port()

            if net_utils.is_tcp_port_taken(port2) or net_utils.is_udp_port_taken(port2):
                return (
                    False,
                    f"  └─ Details: Port {port2} selected for hot-reload was already taken.",
                )

            (vane.tmpdir / "listener" / f"[{port2}]").mkdir(parents=True, exist_ok=True)
            (vane.tmpdir / "listener" / f"[{port2}]" / "tcp.toml").write_text(
                TOML_HTTP_SIMPLE
            )
            (vane.tmpdir / "listener" / f"[{port2}]" / "udp.yaml").write_text(YAML_UDP)

            # --- Phase 2: Hot Reload Verification ---
            tcp_up_string2 = f"PORT {port2} TCP UP"
            udp_up_string2 = f"PORT {port2} UDP UP"

            if not wait_for_log(
                vane, tcp_up_string2, 10, start_index=log_len_after_cold_load
            ):
                return (False, f"  └─ Details: Hot-reload failed for TCP port {port2}.")
            if not wait_for_log(
                vane, udp_up_string2, 5, start_index=log_len_after_cold_load
            ):
                return (False, f"  └─ Details: Hot-reload failed for UDP port {port2}.")

            if not net_utils.is_tcp_port_taken(
                port2
            ) or not net_utils.is_udp_port_taken(port2):
                return (
                    False,
                    f"  └─ Details: Vane logged hot-reloaded ports as UP, but they are not occupied.",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
