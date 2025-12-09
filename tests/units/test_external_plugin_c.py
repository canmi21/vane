# tests/units/test_external_plugin_c.py

import os
import pathlib
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from utils.plugin_test_utils import lifecycle_test
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """Test external plugin execution for: C (Binary)"""
    http_server = None
    try:
        # 1. Setup
        project_root = pathlib.Path(__file__).parent.parent.parent.resolve()
        bin_path = project_root / "examples/plugins/exec/test_c_template"

        if not bin_path.exists():
            return (
                True,
                f"  ⚠ SKIPPED: Binary not found (needs compilation): {bin_path.name}",
            )

        if not os.access(bin_path, os.X_OK):
            try:
                os.chmod(bin_path, 0o755)
            except:
                return (True, f"  ⚠ SKIPPED: Binary is not executable.")

        driver_config = {
            "type": "command",
            "program": str(bin_path),
            "args": [],
            "env": {},
        }

        # 2. Infra
        api_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(backend_port):
            return (False, f"  └─ Details: Backend failed to start.")

        # 3. Vane
        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        with vane:
            if not wait_for_log(vane, f"Listening on http://localhost:{api_port}", 10):
                return (False, f"  └─ Details: Vane API failed to start.")

            return lifecycle_test(
                vane, api_port, backend_port, "test_c_bin", driver_config, debug_mode
            )

    except Exception as e:
        return (False, f"  └─ Details: Unexpected exception: {e}")
    finally:
        if http_server:
            http_server.stop()
