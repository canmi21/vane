# tests/units/test_port_hot_unload.py

from typing import Tuple
from utils.template import VaneInstance
from utils.port_config_utils import TOML_CONTENT, UP_STRING, DOWN_STRING, wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane correctly unloads a listener when its config file is deleted.
    """
    try:
        env_vars = {"LOG_LEVEL": "debug"}
        vane = VaneInstance(env_vars, "", debug_mode)

        listener_dir = vane.tmpdir / "listener" / "[80]"
        listener_dir.mkdir(parents=True, exist_ok=True)
        config_file_path = listener_dir / "tcp.toml"
        config_file_path.write_text(TOML_CONTENT)

        with vane:
            if not wait_for_log(vane, UP_STRING, 10):
                return (
                    False,
                    "  └─ Details: Prerequisite failed - listener did not start initially.",
                )

            log_len_after_up = len(vane.captured_output)
            config_file_path.unlink()

            if not wait_for_log(vane, DOWN_STRING, 10, start_index=log_len_after_up):
                log_dump = "".join(vane.captured_output)
                reason = f"File deleted, but did not find '{DOWN_STRING}' log."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
