# tests/units/test_strategy_fastest.py

import math
from typing import Tuple, List, Union
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'fastest' load balancing strategy using a fundamentally correct
    approach: one real HTTP server as the fast target, and multiple, correctly
    simulated slow TCP servers as decoys.
    """
    # The type hint now correctly refers to the real classes being used,
    # resolving the Pylance error.
    all_servers: List[
        Union[http_utils.StoppableHTTPServer, net_utils.SlowTCPServer]
    ] = []
    try:
        # --- Test Configuration ---
        NUM_BACKENDS = 3
        # A robust warm-up phase is critical to give Vane time to experience
        # the true proxying latency of the slow servers.
        NUM_WARMUP_REQUESTS = NUM_BACKENDS * 5
        NUM_MEASUREMENT_REQUESTS = 40
        MAJORITY_THRESHOLD = 0.8
        SLOW_SERVER_DELAY_SEC = 0.2

        # --- Port and Server Setup ---
        ports = set()
        while len(ports) < NUM_BACKENDS + 1:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = sorted(list(ports))

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
        # --- Start Backend Servers (Corrected Approach) ---
        # 1. Start the one true, fast, functional HTTP server.
        fast_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", fast_server_port), http_utils.RequestRecorderHandler
        )
        fast_server.start()
        if not net_utils.wait_for_tcp_port_ready(fast_server_port):
            return (
                False,
                f"  └─ Details: Fast HTTP backend on port {fast_server_port} failed to start.",
            )
        all_servers.append(fast_server)

        # 2. Start decoy servers using the new, robust SlowTCPServer.
        for port in slow_server_ports:
            # --- THIS SECTION IS CORRECTED ---
            # Now using the correct SlowTCPServer that does not close connections.
            slow_server = net_utils.SlowTCPServer(
                server_address=("127.0.0.1", port),
                RequestHandlerClass=net_utils.SlowTCPHandler,
                delay_sec=SLOW_SERVER_DELAY_SEC,
            )
            slow_server.start()
            if not net_utils.wait_for_tcp_port_ready(port):
                return (
                    False,
                    f"  └─ Details: Slow TCP decoy on port {port} failed to start.",
                )
            all_servers.append(slow_server)

        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
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

            # --- 1. WARM-UP PHASE ---
            # Requests sent to slow servers will time out on the client side,
            # which is expected and acceptable. The goal is to make Vane
            # experience the proxying delay.
            for i in range(NUM_WARMUP_REQUESTS):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/warmup-{i}"])

            # Only the fast server can have received requests. We clear it
            # for the measurement phase.
            warmup_counts = len(fast_server.received_requests)
            fast_server.received_requests.clear()

            # --- 2. MEASUREMENT PHASE ---
            for i in range(NUM_MEASUREMENT_REQUESTS):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/measure-{i}"])

        # --- 3. Final Assertions ---
        fast_server_requests = len(fast_server.received_requests)
        expected_min_requests = math.ceil(NUM_MEASUREMENT_REQUESTS * MAJORITY_THRESHOLD)

        if fast_server_requests < expected_min_requests:
            reason = (
                f"Fastest backend did not receive the majority of traffic.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Fast Backend:  Port {fast_server_port} (HTTP Server, 0.0s Delay)\n"
                f"      │  ├─ Slow Backends: Ports {slow_server_ports} (TCP Decoys, {SLOW_SERVER_DELAY_SEC}s Delay)\n"
                f"      │  └─ Vane Port:     {vane_port}\n"
                f"      ├─ Warm-up Phase ({NUM_WARMUP_REQUESTS} requests)\n"
                f"      │  └─ Requests to Fast Server: {warmup_counts} (The rest timed out as expected)\n"
                f"      └─ Measurement Phase ({NUM_MEASUREMENT_REQUESTS} requests)\n"
                f"         ├─ Requests to Fast Server: {fast_server_requests}\n"
                f"         └─ Expected Minimum:       {expected_min_requests} ({MAJORITY_THRESHOLD:.0%})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in all_servers:
            server.stop()

    return (True, "")
