# tests/units/test_strategy_random.py

from typing import Tuple, List
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'random' load balancing strategy by sending a large number of
    requests and verifying that all backends receive some traffic.
    """
    backend_servers: List[http_utils.StoppableHTTPServer] = []
    try:
        # --- Preparation ---
        num_backends = 3
        # Send enough requests to make it statistically improbable that any
        # single backend receives zero requests.
        num_requests = num_backends * 10

        ports = set()
        while len(ports) < num_backends + 1:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = list(ports)

        targets_str = ",\n".join(
            f'    {{ ip = "127.0.0.1", port = {p} }}' for p in backend_ports
        )

        toml_content = f"""
[[protocols]]
name = "http"
priority = 1
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "random", targets = [
{targets_str}
]}} }}
"""
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

            # --- Send a burst of requests ---
            for i in range(num_requests):
                success, _ = http_utils.send_test_requests(
                    vane_port, ["GET"], [f"/req-{i}"]
                )
                if not success:
                    return (
                        False,
                        f"  └─ Details: Request #{i + 1} sent through Vane failed.",
                    )

        # --- Final Assertions ---
        actual_counts = [len(s.received_requests) for s in backend_servers]
        total_received = sum(actual_counts)

        # 1. Check if the total number of requests is correct.
        if total_received != num_requests:
            reason = (
                f"Total request count mismatch.\n"
                f"      ├─ Expected: {num_requests}\n"
                f"      └─ Actual:   {total_received} (Distribution: {actual_counts})"
            )
            return (False, f"  └─ Details: {reason}")

        # 2. Check that every backend received at least one request.
        if any(count == 0 for count in actual_counts):
            reason = (
                f"One or more backends received zero requests (starvation).\n"
                f"      └─ Actual Counts: {actual_counts}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in backend_servers:
            server.stop()

    return (True, "")
