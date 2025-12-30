# tests/units/test_external_plugin_exec_flow.py

import os
import requests
import time
import socket
import pathlib
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the execution of an external 'exec' (command) driver plugin.
    It registers the 'test_python_template.py' script via the API,
    then sets up two listeners:
    1. One passing the correct token (expected: proxy to backend).
    2. One passing an incorrect token (expected: connection abort).
    """
    http_server = None
    try:
        # --- Setup Paths ---
        # Locate the example python script relative to this test file
        current_file = pathlib.Path(__file__)
        project_root = current_file.parent.parent.parent.resolve()
        script_path = (
            project_root / "examples/plugins/exec/test_python_template.py"
        ).resolve()

        if not script_path.exists():
            return (False, f"Could not find example script at {script_path}")

        # --- Port and Server Setup ---
        api_port = net_utils.find_available_tcp_port()
        port_success = net_utils.find_available_tcp_port()
        port_fail = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        # --- Start Backend Server ---
        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(backend_port):
            return (False, f"HTTP backend on port {backend_port} failed to start.")

        # --- Client Setup ---
        # Use a session that ignores system proxies to ensure direct localhost connection
        session = requests.Session()
        session.trust_env = False

        # --- Vane Startup ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {
            "LOG_LEVEL": log_level,
            "PORT": str(api_port),
            "SKIP_VALIDATE_CONNECTIVITY": "false",
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        with vane:
            # Wait for Management API
            if not wait_for_log(vane, f"Listening on http://localhost:{api_port}", 10):
                return (
                    False,
                    f"  └─ Details: Vane Management API failed to start on port {api_port}.",
                )

            # Inject Authorization
            session.headers.update({"Authorization": f"Bearer {vane.access_token}"})

            # --- Prepare python3 in trusted bin ---
            try:
                vane.copy_to_bin("python3")
            except Exception as e:
                return (False, f"  └─ Details: Prep failed for python3: {e}")

            # --- Step 1: Register the External Plugin ---
            plugin_name = "test_py_auth"
            register_payload = {
                "name": plugin_name,
                "role": "middleware",
                "driver": {
                    "type": "command",
                    "program": "python3", # Now it will find it in bin/
                    "args": [str(script_path)],
                    "env": {},
                },
                "params": [{"name": "auth_token", "required": True}],
            }

            try:
                res = session.post(
                    f"http://127.0.0.1:{api_port}/plugins/{plugin_name}",
                    json=register_payload,
                )
                if res.status_code != 200:
                    return (
                        False,
                        f"  └─ Details: Plugin registration failed. Code: {res.status_code}, Body: {res.text}",
                    )
            except Exception as e:
                return (False, f"  └─ Details: API Request exception: {e}")

            # --- Step 2: Configure Listener A (Success Scenario) ---
            config_success = f"""
connection:
  {plugin_name}:
    input:
      auth_token: "secret123"
    output:
      "success":
        internal.transport.proxy.transparent:
          input:
            target.ip: "127.0.0.1"
            target.port: {backend_port}
      "failure":
        internal.transport.abort:
          input: {{}}
"""
            (vane.tmpdir / "listener" / f"[{port_success}]").mkdir(
                parents=True, exist_ok=True
            )
            (vane.tmpdir / "listener" / f"[{port_success}]" / "tcp.yaml").write_text(
                config_success
            )

            # --- Step 3: Configure Listener B (Failure Scenario) ---
            config_fail = f"""
connection:
  {plugin_name}:
    input:
      auth_token: "wrong_token"
    output:
      "success":
        internal.transport.proxy.transparent:
          input:
            target.ip: "127.0.0.1"
            target.port: {backend_port}
      "failure":
        internal.transport.abort:
          input: {{}}
"""
            (vane.tmpdir / "listener" / f"[{port_fail}]").mkdir(
                parents=True, exist_ok=True
            )
            (vane.tmpdir / "listener" / f"[{port_fail}]" / "tcp.yaml").write_text(
                config_fail
            )

            # Wait for Vane to pick up the new files
            if not wait_for_log(vane, f"PORT {port_success} TCP UP", 5):
                return (
                    False,
                    f"  └─ Details: Listener A (Success Scenario) on port {port_success} failed to start.",
                )
            if not wait_for_log(vane, f"PORT {port_fail} TCP UP", 5):
                return (
                    False,
                    f"  └─ Details: Listener B (Failure Scenario) on port {port_fail} failed to start.",
                )

            # --- Step 4: Verify Success Scenario ---
            try:
                resp = session.get(
                    f"http://127.0.0.1:{port_success}/test_ok", timeout=2.0
                )
                if resp.status_code != 200:
                    return (
                        False,
                        f"  └─ Details: Success scenario request failed. Expected 200, got {resp.status_code}.",
                    )
            except Exception as e:
                return (
                    False,
                    f"  └─ Details: Success scenario request threw exception: {e}",
                )

            # --- Step 5: Verify Failure Scenario ---
            failed_correctly = False
            fail_msg = ""
            try:
                resp = session.get(
                    f"http://127.0.0.1:{port_fail}/test_fail", timeout=2.0
                )
                fail_msg = f"Received unexpected response: Status {resp.status_code}"
            except (
                requests.exceptions.ConnectionError,
                requests.exceptions.ChunkedEncodingError,
            ):
                failed_correctly = True
            except Exception as e:
                fail_msg = f"Unexpected exception type: {type(e)} {e}"

            if not failed_correctly:
                reason = (
                    f"External plugin execution flow failed.\n"
                    f"      \n"
                    f"      ├─ Test Scenario\n"
                    f"      │  ├─ Plugin: {plugin_name} (Python Script)\n"
                    f"      │  ├─ Listener A (Port {port_success}): Inputs 'secret123' -> Expect Success (Proxy)\n"
                    f"      │  └─ Listener B (Port {port_fail}):    Inputs 'wrong_token' -> Expect Failure (Abort)\n"
                    f"      └─ Result\n"
                    f"         ├─ Listener A: Success (Backend reached)\n"
                    f"         └─ Listener B: FAILED - Connection was NOT aborted.\n"
                    f"            └─ Details: {fail_msg}"
                )
                return (False, f"  └─ Details: {reason}")

        # --- Final Assertions ---
        hits = len(http_server.received_requests)
        if hits != 1:
            reason = (
                f"Backend request count mismatch.\n"
                f"      \n"
                f"      ├─ Expected\n"
                f"      │  └─ Total Hits: 1 (Only from Listener A)\n"
                f"      └─ Actual\n"
                f"         └─ Total Hits: {hits}\n"
            )
            return (False, f"  └─ Details: {reason}")

        recorded_path = http_server.received_requests[0]["path"]
        if recorded_path != "/test_ok":
            return (
                False,
                f"  └─ Details: Backend received wrong path: '{recorded_path}', expected '/test_ok'.",
            )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if http_server:
            http_server.stop()

    return (True, "")
