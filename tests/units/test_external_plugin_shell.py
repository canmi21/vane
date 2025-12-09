# tests/units/test_external_plugin_shell.py

import shutil
import pathlib
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from utils.plugin_test_utils import lifecycle_test
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """Test external plugin execution for: Shell (Bash)"""
    http_server = None
    try:
        project_root = pathlib.Path(__file__).parent.parent.parent.resolve()
        script_path = project_root / "examples/plugins/exec/test_shell_template.sh"
        interpreter = "bash"

        if not shutil.which(interpreter):
            return (True, f"  ⚠ SKIPPED: '{interpreter}' not found.")
        if not script_path.exists():
            return (False, f"  └─ Details: Script not found: {script_path}")

        driver_config = {
            "type": "command",
            "program": interpreter,
            "args": [str(script_path)],
            "env": {},
        }

        api_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()
        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(backend_port):
            return (False, "Backend failed.")

        env_vars = {
            "LOG_LEVEL": "debug" if debug_mode else "info",
            "PORT": str(api_port),
        }
        vane = VaneInstance(env_vars, "", debug_mode)

        with vane:
            if not wait_for_log(vane, f"Listening on http://localhost:{api_port}", 10):
                return (False, "Vane API failed.")
            return lifecycle_test(
                vane, api_port, backend_port, "test_shell", driver_config, debug_mode
            )
    except Exception as e:
        return (False, f"Exception: {e}")
    finally:
        if http_server:
            http_server.stop()
