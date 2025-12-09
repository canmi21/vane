# tests/units/test_external_plugin_exec_env.py

import sys
import shutil
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from utils.plugin_test_utils import lifecycle_test
from units.config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that environment variables defined in the plugin configuration
    are correctly passed to the external executable.
    """
    http_server = None
    try:
        # 1. Infra Setup
        api_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(backend_port):
            return (
                False,
                f"  └─ Details: Backend failed to start on port {backend_port}",
            )

        # 2. Vane Startup
        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        with vane:
            if not wait_for_log(vane, f"Listening on http://localhost:{api_port}", 10):
                return (False, f"  └─ Details: Vane API failed to start.")

            # 3. Create a custom script that verifies ENV injection
            # Logic: If ENV is missing/wrong -> Fail. If ENV is right -> Check Token (Standard logic).
            script_content = r"""
import os, sys, json

def read_stdin():
    lines = []
    for line in sys.stdin:
        lines.append(line.rstrip('\n'))
    return '\n'.join(lines)

# 1. Check ENV
target_val = os.environ.get("VANE_TEST_ENV")
if target_val != "injected_successfully":
    print(f"⚙ [Script] ENV CHECK FAILED. Got: {target_val}", file=sys.stderr)
    # Return failure branch immediately if ENV is wrong
    print('{"branch":"failure","store":{"reason":"env_mismatch"}}')
    sys.exit(0)

print("⚙ [Script] ENV CHECK PASSED.", file=sys.stderr)

# 2. Standard Token Check (to satisfy lifecycle_test success/fail scenarios)
try:
    data = json.loads(read_stdin())
except:
    sys.exit(1)

if data.get("auth_token") == "secret123":
    print('{"branch":"success","store":{}}')
else:
    print('{"branch":"failure","store":{}}')
"""
            script_path = vane.tmpdir / "env_check_script.py"
            script_path.write_text(script_content)

            # 4. Define Plugin Config with ENV
            driver_config = {
                "type": "command",
                "program": "python3",
                "args": [str(script_path)],
                # THIS IS THE CORE TEST: We inject this specific variable
                "env": {"VANE_TEST_ENV": "injected_successfully"},
            }

            # 5. Run Lifecycle Test
            # This will verify that:
            # - Success Case: (Env Correct + Token Correct) -> Proxy (200 OK)
            # - Failure Case: (Env Correct + Token Wrong) -> Abort (Conn Reset)
            # If ENV injection fails, the Success Case will fail (script returns "failure").
            return lifecycle_test(
                vane,
                api_port,
                backend_port,
                "test_env_injection",
                driver_config,
                debug_mode,
            )

    except Exception as e:
        return (False, f"  └─ Details: Unexpected exception: {e}")
    finally:
        if http_server:
            http_server.stop()
