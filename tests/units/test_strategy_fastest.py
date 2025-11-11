# tests/units/test_strategy_fastest.py

import math
from typing import Tuple, List, Union
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'fastest' load balancing strategy by creating backends with
    varying handshake latencies and verifying that the majority of traffic is
    routed to the fastest backend.
    """
    backend_servers: List[
        Union[http_utils.StoppableHTTPServer, http_utils.DelayedStoppableHTTPServer]
    ] = []
    try:
        # --- Preparation ---
        NUM_BACKENDS = 3
        NUM_REQUESTS = 40  # High enough to establish a clear pattern
        MAJORITY_THRESHOLD = 0.7
        SLOW_SERVER_DELAY_SEC = 0.2

        ports = set()
        while len(ports) < NUM_BACKENDS + 1:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = list(ports)

        fast_server_port = backend_ports[0]
        slow_server_ports = backend_ports[1:]

        targets_str = ",\n".join(
            f'    {{ ip = "127.0.0.1", port = {p} }}' for p in backend_ports
        )

        toml_content = f"""
[[protocols]]
name = "http"
priority = 1
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "fastest", targets = [
{targets_str}
]}} }}
"""
        # --- Start Backend Servers ---
        # 1. Start the fast server (target)
        fast_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", fast_server_port), http_utils.RequestRecorderHandler
        )
        fast_server.start()
        backend_servers.append(fast_server)

        # 2. Start slow servers
        for port in slow_server_ports:
            slow_server = http_utils.DelayedStoppableHTTPServer(
                ("127.0.0.1", port),
                http_utils.RequestRecorderHandler,
                handshake_delay_sec=SLOW_SERVER_DELAY_SEC,
            )
            slow_server.start()
            backend_servers.append(slow_server)

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
            # Vane needs a few initial connections to measure latency.
            # The distribution will stabilize after a few requests.
            for i in range(NUM_REQUESTS):
                success, _ = http_utils.send_test_requests(
                    vane_port, ["GET"], [f"/req-{i}"]
                )
                if not success:
                    return (
                        False,
                        f"  └─ Details: Request #{i + 1} sent through Vane failed.",
                    )

        # --- Final Assertions ---
        fast_server_requests = len(fast_server.received_requests)
        total_received = sum(len(s.received_requests) for s in backend_servers)

        # 1. Check if the total number of requests is correct.
        if total_received != NUM_REQUESTS:
            counts = [len(s.received_requests) for s in backend_servers]
            reason = (
                f"Total request count mismatch.\n"
                f"      ├─ Expected: {NUM_REQUESTS}\n"
                f"      └─ Actual:   {total_received} (Distribution: {counts})"
            )
            return (False, f"  └─ Details: {reason}")

        # 2. Check that the fastest server received the vast majority of requests.
        expected_min_requests = math.ceil(NUM_REQUESTS * MAJORITY_THRESHOLD)
        if fast_server_requests < expected_min_requests:
            counts = [len(s.received_requests) for s in backend_servers]
            reason = (
                f"Fastest backend did not receive the majority of traffic.\n"
                f"      ├─ Target Backend Port: {fast_server_port}\n"
                f"      ├─ Requests Received:   {fast_server_requests}\n"
                f"      ├─ Expected Minimum:    {expected_min_requests} ({MAJORITY_THRESHOLD:.0%})\n"
                f"      └─ Full Distribution:   {counts}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in backend_servers:
            server.stop()

    return (True, "")
