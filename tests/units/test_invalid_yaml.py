# tests/units/test_invalid_yaml.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests if Vane correctly logs an error when encountering a syntactically
    invalid YAML configuration file.
    """
    success_string = "Failed to parse config file"

    # A single tab character is invalid indentation in YAML.
    invalid_yaml_content = "\t- invalid"

    try:
        port = net_utils.find_available_udp_port()
        env_vars = {"LOG_LEVEL": "debug"}

        vane = VaneInstance(env_vars, success_string, debug_mode)

        listener_dir = vane.tmpdir / "listener" / f"[{port}]"
        listener_dir.mkdir(parents=True, exist_ok=True)
        (listener_dir / "udp.yaml").write_text(invalid_yaml_content)

        with vane:
            if not vane.found_event.wait(timeout=10):
                log_dump = "".join(vane.captured_output)
                reason = f"Timeout waiting for YAML parsing error (did not find '{success_string}')."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

    except Exception as e:
        return (
            False,
            f"  └─ Details: An unexpected exception occurred during the test: {e}",
        )

    return (True, "")
