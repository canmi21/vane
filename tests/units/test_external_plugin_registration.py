# tests/units/test_external_plugin_persistence.py

import os
import time
import requests
import subprocess
import tempfile
import shutil
from typing import Tuple
from utils import net_utils


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that external plugins persist across Vane restarts.
    """
    test_config_dir = tempfile.mkdtemp(prefix="vane_persist_test_")
    try:
        api_port = net_utils.find_available_tcp_port()

        # Use a simple existing command like "ls" or "echo" if script not found,
        # just to satisfy the registration (since we skip validation).
        plugin_script = "dummy_script.py"

        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
            "CONFIG_DIR": test_config_dir,
            "SOCKET_DIR": test_config_dir,  # Ensure socket goes here too
            "SKIP_VALIDATE_CONNECTIVITY": "true",  # Important!
        }

        # Ensure we run the 'vane' binary from the project target/debug or release
        # Assuming 'cargo run' or binary is in PATH. If using 'vane', make sure it's compiled.
        # Better to rely on what VaneInstance does, but here we invoke manually.
        vane_bin = "vane"  # Assumes vane is in PATH or alias

        session = requests.Session()
        session.trust_env = False

        # --- Phase 1: Start, Register, Stop ---
        proc1 = subprocess.Popen(
            [vane_bin],
            env={**os.environ, **env_vars},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

        try:
            # Wait for port to be open
            if not net_utils.wait_for_port(api_port, timeout=10):
                return (False, "Phase 1: Vane failed to start (port not open).")

            plugin_name = "persist_plugin"
            payload = {
                "name": plugin_name,
                "role": "middleware",
                "driver": {
                    "type": "command",
                    "program": "python3",
                    "args": [plugin_script],
                    "env": {},
                },
                "params": [],
            }

            res = session.post(
                f"http://127.0.0.1:{api_port}/plugins/{plugin_name}", json=payload
            )
            if res.status_code != 200:
                return (False, f"Phase 1: Registration failed: {res.text}")

        finally:
            proc1.terminate()
            proc1.wait()

        # --- Phase 2: Start again, Check ---
        proc2 = subprocess.Popen(
            [vane_bin],
            env={**os.environ, **env_vars},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

        try:
            if not net_utils.wait_for_port(api_port, timeout=10):
                return (False, "Phase 2: Vane failed to restart.")

            res = session.get(f"http://127.0.0.1:{api_port}/plugins")
            if res.status_code != 200:
                return (False, f"Phase 2: List API failed: {res.text}")

            data = res.json().get("data", {})
            plugins = data.get("plugins", [])

            if plugin_name not in plugins:
                return (False, "Phase 2: Plugin did not persist.")

        finally:
            proc2.terminate()
            proc2.wait()

    except Exception as e:
        return (False, f"Exception: {e}")
    finally:
        if os.path.exists(test_config_dir):
            shutil.rmtree(test_config_dir)

    return (True, "")
