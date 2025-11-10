# tests/units/test_dynamic_port.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import YAML_UDP, JSON_UDP, TOML_UDP
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests dynamic port loading with multiple hot-swaps between config formats.
    """
    try:
        port = net_utils.find_available_udp_port()
        up_string = f"PORT {port} UDP UP"
        down_string = f"PORT {port} UDP DOWN"

        env_vars = {"LOG_LEVEL": "debug"}
        vane = VaneInstance(env_vars, "", debug_mode)

        listener_dir = vane.tmpdir / "listener" / f"[{port}]"
        listener_dir.mkdir(parents=True, exist_ok=True)

        yaml_path = listener_dir / "udp.yaml"
        json_path = listener_dir / "udp.json"
        toml_path = listener_dir / "udp.toml"

        yaml_path.write_text(YAML_UDP)

        with vane:
            # Phase 1: Test cold load with YAML
            if not wait_for_log(vane, up_string, 10):
                log_dump = "".join(vane.captured_output)
                reason = f"Timeout on cold load (YAML). Did not find '{up_string}'."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

            log_len_after_up1 = len(vane.captured_output)

            # Phase 2: Hot-swap from YAML to JSON
            yaml_path.unlink()
            if not wait_for_log(vane, down_string, 10, start_index=log_len_after_up1):
                return (
                    False,
                    f"  └─ Details: Failed to unload YAML config. Did not find '{down_string}'.",
                )

            log_len_after_down1 = len(vane.captured_output)
            json_path.write_text(JSON_UDP)
            if not wait_for_log(vane, up_string, 10, start_index=log_len_after_down1):
                return (
                    False,
                    f"  └─ Details: Failed to hot-reload with JSON. Did not find '{up_string}'.",
                )

            log_len_after_up2 = len(vane.captured_output)

            # Phase 3: Hot-swap from JSON to TOML
            json_path.unlink()
            if not wait_for_log(vane, down_string, 10, start_index=log_len_after_up2):
                return (
                    False,
                    f"  └─ Details: Failed to unload JSON config. Did not find '{down_string}'.",
                )

            log_len_after_down2 = len(vane.captured_output)
            toml_path.write_text(TOML_UDP)
            if not wait_for_log(vane, up_string, 10, start_index=log_len_after_down2):
                return (
                    False,
                    f"  └─ Details: Failed to hot-reload with TOML. Did not find '{up_string}'.",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
