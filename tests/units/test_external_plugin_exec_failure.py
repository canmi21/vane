import os
import shutil
import pathlib
import requests
import tempfile
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the behavior when an external plugin's executable is deleted
    after registration but before execution (runtime failure).
    """
    http_server = None
    try:
        # --- 1. Setup Paths ---
        project_root = pathlib.Path(__file__).parent.parent.parent.resolve()
        source_bin = project_root / "examples/plugins/exec/test_c_template"

        if not source_bin.exists():
            return (
                True,
                f"  ⚠ SKIPPED: Source binary not found (needs compilation): {source_bin.name}",
            )

        # --- 2. Infra Setup ---
        api_port = net_utils.find_available_tcp_port()
        proxy_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(backend_port):
            return (
                False,
                f"  └─ Details: Backend failed to start on port {backend_port}.",
            )

        # --- 3. Vane Startup ---
        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        session = requests.Session()
        session.trust_env = False

        with vane:
            # --- 4. Prepare Program in Trusted Bin ---
            try:
                # Copy binary to Vane's trusted bin root
                bin_name = vane.copy_to_bin(str(source_bin), target_name="test_fragile_bin_binary")
                target_bin_path = vane.bin_dir / bin_name
            except Exception as e:
                return (False, f"  └─ Details: Prep failed: {e}")

            if not wait_for_log(vane, f"✓ Management console listening on unix:", 10):
                return (False, f"  └─ Details: Vane API failed to start.")

            # Inject Authorization
            session.headers.update({"Authorization": f"Bearer {vane.access_token}"})

            # --- 5. Register Plugin ---
            plugin_name = "test_fragile_bin"
            driver_config = {
                "type": "command",
                "program": bin_name, # Use filename only
                "args": [],
                "env": {},
            }
            reg_payload = {
                "name": plugin_name,
                "role": "middleware",
                "driver": driver_config,
                "params": [{"name": "auth_token", "required": True}],
            }

            try:
                res = session.post(
                    f"http://127.0.0.1:{api_port}/plugins/{plugin_name}",
                    json=reg_payload,
                )
                if res.status_code != 200:
                    return (False, f"  └─ Details: Registration failed: {res.text}")
            except Exception as e:
                return (False, f"  └─ Details: API Registration exception: {e}")

            if debug_mode:
                print(
                    f"    ➜ Registered plugin '{plugin_name}' pointing to {target_bin_path}"
                )

            # --- 6. Configure Listener ---
            config = f"""
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
            listener_dir = vane.tmpdir / "listener" / f"[{proxy_port}]"
            listener_dir.mkdir(parents=True, exist_ok=True)
            (listener_dir / "tcp.yaml").write_text(config)

            if not wait_for_log(vane, f"PORT {proxy_port} TCP UP", 5):
                return (False, f"  └─ Details: Listener failed to start.")

            # --- 7. Phase 1: Verify Normal Operation ---
            try:
                resp = session.get(
                    f"http://127.0.0.1:{proxy_port}/phase1_ok", timeout=2.0
                )
                if resp.status_code != 200:
                    return (
                        False,
                        f"  └─ Details: Phase 1 (Normal) failed. Status: {resp.status_code}",
                    )
            except Exception as e:
                return (False, f"  └─ Details: Phase 1 exception: {e}")

            if len(http_server.received_requests) != 1:
                return (
                    False,
                    f"  └─ Details: Phase 1: Backend did not receive request.",
                )

            # --- 8. Phase 2: Sabotage - Delete the binary ---
            if debug_mode:
                print(f"    ➜ Deleting binary: {target_bin_path}")
            os.remove(target_bin_path)

            # --- 9. Phase 3: Verify Runtime Failure ---
            # Expectation: Vane tries to spawn -> Fails -> Flow Error -> Connection Closed
            failed_correctly = False
            fail_msg = ""
            try:
                resp = session.get(
                    f"http://127.0.0.1:{proxy_port}/phase2_fail", timeout=2.0
                )
                fail_msg = f"Received Status {resp.status_code}"
            except (
                requests.exceptions.ConnectionError,
                requests.exceptions.ChunkedEncodingError,
            ):
                failed_correctly = True
            except Exception as e:
                fail_msg = f"Exception: {type(e)} {e}"

            if not failed_correctly:
                reason = (
                    f"Runtime missing binary test failed.\n"
                    f"      \n"
                    f"      ├─ Scenario\n"
                    f"      │  ├─ Action: Deleted binary '{target_bin_path}' while listener was active.\n"
                    f"      │  └─ Request: Sent traffic to listener.\n"
                    f"      └─ Result\n"
                    f"         ├─ Expected: Connection Abort (due to spawn failure)\n"
                    f"         └─ Actual: Request succeeded or failed with wrong error. {fail_msg}\n"
                )
                return (False, f"  └─ Details: {reason}")

            # Backend should NOT have received the second request
            if len(http_server.received_requests) != 1:
                return (
                    False,
                    f"  └─ Details: Phase 2: Backend received traffic despite missing binary! (Security/Logic Flaw)",
                )

    except Exception as e:
        return (False, f"  └─ Details: Unexpected exception: {e}")
    finally:
        if http_server:
            http_server.stop()

    return (True, "")
