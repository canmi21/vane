# tests/units/test_external_plugin_httpx.py

import sys
import subprocess
import time
import pathlib
import requests
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from utils.plugin_test_utils import lifecycle_test
from units.config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'http' external driver.
    It spins up a local Python HTTP server acting as the plugin logic,
    registers it with Vane, and verifies traffic flow.
    """
    http_server = None
    plugin_server_proc = None
    try:
        # 1. Setup Plugin Server
        project_root = pathlib.Path(__file__).parent.parent.parent.resolve()
        server_script = project_root / "examples/plugins/httpx/mock_server.py"

        if not server_script.exists():
            return (
                False,
                f"  └─ Details: Mock server script not found: {server_script}",
            )

        plugin_port = net_utils.find_available_tcp_port()

        # Start the external plugin server
        plugin_server_proc = subprocess.Popen(
            [sys.executable, str(server_script), str(plugin_port)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        # Wait for plugin server to be ready
        if not net_utils.wait_for_port(plugin_port, timeout=5):
            return (
                False,
                f"  └─ Details: External HTTP plugin server failed to start on port {plugin_port}.",
            )

        # 2. Infra Setup (Backend)
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
        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        with vane:
            if not wait_for_log(vane, f"Listening on http://localhost:{api_port}", 10):
                return (False, f"  └─ Details: Vane API failed to start.")

            # 4. Configure & Test
            driver_config = {
                "type": "http",
                "url": f"http://127.0.0.1:{plugin_port}",
            }

            return lifecycle_test(
                vane,
                api_port,
                backend_port,
                "test_http_driver",
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
            plugin_server_proc.wait()

    return (True, "")
