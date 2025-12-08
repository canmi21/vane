# tests/units/test_external_plugin_persistence.py

import os
import time
import requests
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that external plugins persist across Vane restarts.
    """
    try:
        api_port = net_utils.find_available_tcp_port()
        project_dir = os.environ.get("DEV_PROJECT_DIR")
        if not project_dir:
            return (False, "DEV_PROJECT_DIR not set.")
        project_dir = os.path.expanduser(project_dir)
        plugin_script = os.path.join(
            project_dir, "examples/plugins/exec/test_python_template.py"
        )

        log_level = "debug" if debug_mode else "info"
        env_vars = {
            "LOG_LEVEL": log_level,
            "PORT": str(api_port),
        }

        # We reuse the same config dir (via VaneInstance temp dir logic? No, VaneInstance creates a new tmp dir each time)
        # To test persistence, we need to control the config directory.
        # VaneInstance manages its own temp dir. We need to manually manage it to reuse it.

        import tempfile
        import shutil

        # Create a persistent temp dir for this test
        test_config_dir = tempfile.mkdtemp(prefix="vane_persist_test_")

        # Override VaneInstance behavior by passing CONFIG_DIR in env_vars
        # But VaneInstance __enter__ overwrites CONFIG_DIR.
        # We need to manually run Vane here or modify VaneInstance to accept an external temp dir.
        # For simplicity, let's just hack VaneInstance to use our dir if provided.
        # Wait, VaneInstance creates a tempdir in __init__.
        # We can't easily reuse it across two 'with' blocks because __exit__ cleans it up.

        # Strategy: Run Vane, register, stop Vane (manually kill process but keep dir), start Vane again.
        # But VaneInstance context manager handles process lifecycle tightly.

        # Let's modify VaneInstance usage. We will start it, do stuff, then KILL it inside the block,
        # then START a new process using the SAME directory manually.

        vane = VaneInstance(env_vars, "", debug_mode)

        # 1. Start Vane (Phase 1)
        with vane:
            # wait for start
            if not wait_for_log(vane, f"Listening on", 10):
                return (False, "Phase 1: Vane did not start.")

            # Register plugin
            plugin_name = "persist_test_plugin"
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
            requests.post(
                f"http://127.0.0.1:{api_port}/plugins/{plugin_name}", json=payload
            )

            # Verify it's there
            resp = requests.get(f"http://127.0.0.1:{api_port}/plugins").json()
            if plugin_name not in resp["data"]["plugins"]:
                return (False, "Phase 1: Plugin registration failed.")

            # Verify plugins.json was created/updated on disk
            plugins_json_path = list(vane.tmpdir.glob("**/plugins.json"))
            # It might be in the root of config dir
            if not (vane.tmpdir / "plugins.json").exists():
                return (False, "Phase 1: plugins.json not found on disk.")

        # Context manager exits -> Process killed, but dir is cleaned up!
        # CAUTION: VaneInstance cleans up tmpdir on exit.
        # We cannot use VaneInstance for persistence testing across contexts unless we modify it.

        # Alternative: Test persistence by checking if the file is written.
        # If the file is written correctly, we trust the loader (tested via unit tests? no unit tests for loader yet).
        # We really should test the reload.

        # WORKAROUND: We will implement the restart logic INSIDE the single VaneInstance block.
        # But VaneInstance doesn't support restarting the process.

        # Let's Skip full restart test for now and rely on file existence check,
        # OR write a custom test runner for this specific case without VaneInstance.

        # Let's go with Custom Runner for Persistence.

        import subprocess

        # 1. Setup Dir
        env_vars["CONFIG_DIR"] = test_config_dir
        env_vars["SOCKET_DIR"] = test_config_dir

        # 2. Start Vane 1
        proc1 = subprocess.Popen(
            ["vane"],
            env={**os.environ, **env_vars},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        time.sleep(2)  # Give it time to start

        try:
            # Register
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
            requests.post(
                f"http://127.0.0.1:{api_port}/plugins/{plugin_name}", json=payload
            )
        finally:
            proc1.terminate()
            proc1.wait()

        # 3. Start Vane 2 (Same Config Dir)
        proc2 = subprocess.Popen(
            ["vane"],
            env={**os.environ, **env_vars},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        time.sleep(2)

        try:
            # Check
            resp = requests.get(f"http://127.0.0.1:{api_port}/plugins").json()
            if plugin_name not in resp["data"]["plugins"]:
                shutil.rmtree(test_config_dir)
                return (False, "Phase 2: Plugin did not persist after restart.")
        finally:
            proc2.terminate()
            proc2.wait()
            shutil.rmtree(test_config_dir)

    except Exception as e:
        return (False, f"Exception: {e}")

    return (True, "")
