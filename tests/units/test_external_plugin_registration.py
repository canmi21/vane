# tests/units/test_external_plugin_registration.py

import os
import requests
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the basic registration and deletion lifecycle of an external plugin via the API.
    """
    try:
        api_port = net_utils.find_available_tcp_port()

        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        session = requests.Session()
        session.trust_env = False

        with vane:
            # Wait for Management API
            if not wait_for_log(vane, f"Listening on http://localhost:{api_port}", 10):
                return (
                    False,
                    f"  └─ Details: Vane Management API failed to start on port {api_port}.",
                )

            # Inject Authorization
            session.headers.update({"Authorization": f"Bearer {vane.access_token}"})

            # --- 1. Register Plugin ---
            plugin_name = "reg_test_plugin"
            payload = {
                "name": plugin_name,
                "role": "middleware",
                "driver": {
                    "type": "command",
                    "program": "echo",  # Use a safe dummy command
                    "args": ["hello"],
                    "env": {},
                },
                "params": [],
            }

            try:
                res = session.post(
                    f"http://127.0.0.1:{api_port}/plugins/{plugin_name}",
                    json=payload,
                )
                if res.status_code != 200:
                    return (
                        False,
                        f"  └─ Details: Registration failed: {res.status_code} {res.text}",
                    )
            except Exception as e:
                return (False, f"  └─ Details: API Request exception: {e}")

            if debug_mode:
                print(f"    ➜ Registered plugin: {plugin_name}")

            # --- 2. Verify in List API ---
            res = session.get(f"http://127.0.0.1:{api_port}/plugins")
            if res.status_code != 200:
                return (False, f"  └─ Details: List API failed: {res.status_code}")

            data = res.json().get("data", {})
            plugins = data.get("plugins", [])
            if plugin_name not in plugins:
                return (
                    False,
                    f"  └─ Details: Plugin {plugin_name} not found in list API.",
                )

            # --- 3. Verify on Disk ---
            # VaneInstance puts config in a temp dir
            plugins_json_path = vane.tmpdir / "plugins.json"
            if not plugins_json_path.exists():
                return (False, f"  └─ Details: plugins.json not found on disk.")

            content = plugins_json_path.read_text()
            if plugin_name not in content:
                return (
                    False,
                    f"  └─ Details: Plugin name not found in plugins.json content.",
                )

            # --- 4. Delete Plugin ---
            res = session.delete(f"http://127.0.0.1:{api_port}/plugins/{plugin_name}")
            if res.status_code != 200:
                return (False, f"  └─ Details: Delete failed: {res.status_code}")

            # --- 5. Verify Deletion ---
            res = session.get(f"http://127.0.0.1:{api_port}/plugins")
            data = res.json().get("data", {})
            plugins = data.get("plugins", [])
            if plugin_name in plugins:
                return (
                    False,
                    f"  └─ Details: Plugin {plugin_name} still exists after deletion.",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
