# tests/units/test_strategy_serial.py

from typing import Tuple, List
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'serial' (round-robin) load balancing strategy by sending
    multiple requests and verifying the distribution incrementally.
    """
    backend_servers: List[http_utils.StoppableHTTPServer] = []
    try:
        # --- Preparation ---
        num_backends = 3
        num_requests = 9

        ports = set()
        while len(ports) < num_backends + 1:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = list(ports)

        targets_str = ",\n".join(
            f'    {{ ip = "127.0.0.1", port = {p} }}' for p in backend_ports
        )

        # --- THIS SECTION IS CORRECTED ---
        # The closing braces for the TOML structure are now correctly escaped.
        toml_content = f"""
[[protocols]]
name = "http"
priority = 1
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
{targets_str}
]}} }}
"""
        # --- End of Correction ---

        # --- Start Backend Servers ---
        for port in backend_ports:
            server = http_utils.StoppableHTTPServer(
                ("127.0.0.1", port), http_utils.RequestRecorderHandler
            )
            server.start()
            backend_servers.append(server)

        # --- Configure and Start Vane ---
        env_vars = {"LOG_LEVEL": "debug"}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.toml").write_text(
            toml_content
        )

        with vane:
            up_string = f"PORT {vane_port} TCP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            # --- Incremental Request & Verification Loop ---
            expected_counts = [0] * num_backends
            for i in range(num_requests):
                success, _ = http_utils.send_test_requests(
                    vane_port, ["GET"], [f"/req-{i}"]
                )
                if not success:
                    return (
                        False,
                        f"  └─ Details: Request #{i + 1} sent through Vane failed.",
                    )

                target_index = i % num_backends
                expected_counts[target_index] += 1

                actual_counts = [len(s.received_requests) for s in backend_servers]

                if actual_counts != expected_counts:
                    reason = (
                        f"Distribution failed at request #{i + 1}.\n"
                        f"      ├─ Expected Counts: {expected_counts}\n"
                        f"      └─ Actual Counts:   {actual_counts}"
                    )
                    return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in backend_servers:
            server.stop()

    return (True, "")
