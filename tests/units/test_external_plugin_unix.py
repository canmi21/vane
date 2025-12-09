# tests/units/test_external_plugin_unix.py

import sys
import subprocess
import time
import shutil
import pathlib
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from utils.plugin_test_utils import lifecycle_test
from units.config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'unix' external driver.
    It spins up a local Lua server listening on a Unix Domain Socket,
    registers it with Vane, and verifies traffic flow.
    """
    http_server = None
    plugin_server_proc = None
    try:
        # 1. Setup Paths
        project_root = pathlib.Path(__file__).parent.parent.parent.resolve()
        script_path = project_root / "examples/plugins/unixsocket/mock_unix_server.lua"

        interpreter = "lua"
        if not shutil.which(interpreter):
            return (True, f"  ⚠ SKIPPED: '{interpreter}' not found.")

        if not script_path.exists():
            return (False, f"  └─ Details: Script not found: {script_path}")

        # 2. Infra Setup
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

        # 3. Vane Startup
        # We need a temp dir for the socket file to ensure write permissions and cleanup
        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        with vane:
            if not wait_for_log(vane, f"Listening on http://localhost:{api_port}", 10):
                return (False, f"  └─ Details: Vane API failed to start.")

            # 4. Start Lua UDS Server
            socket_path = vane.tmpdir / "plugin_mock.sock"

            plugin_server_proc = subprocess.Popen(
                [interpreter, str(script_path), str(socket_path)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )

            # Wait for socket file to appear (Server Ready)
            start_time = time.monotonic()
            socket_ready = False
            while time.monotonic() - start_time < 5:
                if socket_path.exists():
                    socket_ready = True
                    break
                time.sleep(0.1)
                # Check if process died
                if plugin_server_proc.poll() is not None:
                    _, stderr = plugin_server_proc.communicate()
                    return (
                        True,
                        f"  ⚠ SKIPPED: Lua script failed to start (missing socket.unix?): {stderr.strip()}",
                    )

            if not socket_ready:
                return (
                    False,
                    "  └─ Details: Timeout waiting for Unix socket file to be created.",
                )

            # 5. Configure & Test
            driver_config = {
                "type": "unix",
                "path": str(socket_path),
            }

            return lifecycle_test(
                vane,
                api_port,
                backend_port,
                "test_unix_driver",
                driver_config,
                debug_mode,
            )

    except Exception as e:
        return (False, f"  └─ Details: Unexpected exception: {e}")
    finally:
        if http_server:
            http_server.stop()
        if plugin_server_proc:
            plugin_server_proc.terminate()
            try:
                plugin_server_proc.wait(timeout=1)
            except subprocess.TimeoutExpired:
                plugin_server_proc.kill()

    return (True, "")
