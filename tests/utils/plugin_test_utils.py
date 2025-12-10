# tests/utils/plugin_test_utils.py

import time
import shutil
import requests
from typing import Tuple, Dict, Any
from utils import net_utils
from units.config_utils import wait_for_log


def lifecycle_test(
    vane_instance: Any,
    api_port: int,
    backend_port: int,
    plugin_name: str,
    driver_config: Dict[str, Any],
    debug_mode: bool,
) -> Tuple[bool, str]:
    """
    Executes the standard lifecycle test for an external plugin:
    1. Register via API.
    2. Configure Listener A (Success Case) -> Verify Proxy.
    3. Configure Listener B (Failure Case) -> Verify Abort.
    4. Delete Plugin via API.
    """
    session = requests.Session()
    session.trust_env = False

    # --- 1. Register Plugin ---
    register_payload = {
        "name": plugin_name,
        "role": "middleware",
        "driver": driver_config,
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
                f"  └─ Details: Registration failed: {res.status_code} {res.text}",
            )
    except Exception as e:
        return (False, f"  └─ Details: API Registration exception: {e}")

    if debug_mode:
        print(f"    ➜ Registered plugin: {plugin_name}")

    # --- 2. Test Success Case ---
    port_success = net_utils.find_available_tcp_port()
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
    dir_success = vane_instance.tmpdir / "listener" / f"[{port_success}]"
    dir_success.mkdir(parents=True, exist_ok=True)
    (dir_success / "tcp.yaml").write_text(config_success)

    if not wait_for_log(vane_instance, f"PORT {port_success} TCP UP", 5):
        return (
            False,
            f"  └─ Details: Listener failed to start on port {port_success}.",
        )

    try:
        resp = session.get(f"http://127.0.0.1:{port_success}/test_ok", timeout=2.0)
        if resp.status_code != 200:
            reason = (
                f"Success case failed.\n"
                f"      \n"
                f"      ├─ Scenario: Input 'secret123' -> Expected 200 OK\n"
                f"      └─ Result: Status {resp.status_code}\n"
            )
            return (False, f"  └─ Details: {reason}")
    except Exception as e:
        return (False, f"  └─ Details: Success case exception: {e}")

    # Cleanup Success Listener
    shutil.rmtree(dir_success)
    time.sleep(0.2)  # Allow slight buffer for FS watcher

    # --- 3. Test Failure Case (New Port) ---
    port_fail = net_utils.find_available_tcp_port()
    config_fail = config_success.replace("secret123", "wrong_token")

    dir_fail = vane_instance.tmpdir / "listener" / f"[{port_fail}]"
    dir_fail.mkdir(parents=True, exist_ok=True)
    (dir_fail / "tcp.yaml").write_text(config_fail)

    if not wait_for_log(vane_instance, f"PORT {port_fail} TCP UP", 5):
        return (
            False,
            f"  └─ Details: Failure listener failed to start on port {port_fail}.",
        )

    failed_correctly = False
    fail_msg = ""
    try:
        resp = session.get(f"http://127.0.0.1:{port_fail}/test_fail", timeout=2.0)
        fail_msg = f"Received Status {resp.status_code}"
    except (
        requests.exceptions.ConnectionError,
        requests.exceptions.ChunkedEncodingError,
    ):
        failed_correctly = True
    except Exception as e:
        fail_msg = f"Exception: {e}"

    if not failed_correctly:
        reason = (
            f"Failure case failed.\n"
            f"      \n"
            f"      ├─ Scenario: Input 'wrong_token' -> Expected Abort\n"
            f"      └─ Result: Did not abort. {fail_msg}\n"
        )
        return (False, f"  └─ Details: {reason}")

    # Cleanup Failure Listener
    shutil.rmtree(dir_fail)
    time.sleep(0.2)

    # --- 4. Delete Plugin ---
    try:
        res = session.delete(f"http://127.0.0.1:{api_port}/plugins/{plugin_name}")
        if res.status_code != 200:
            return (
                False,
                f"  └─ Details: Delete plugin failed: {res.status_code}",
            )
    except Exception as e:
        return (False, f"  └─ Details: Delete API exception: {e}")

    return (True, "")
