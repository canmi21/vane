# tests/units/test_backend_auto_recovery.py

import time
from typing import Tuple, List
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests Vane's ability to automatically bring back recovered backends into
    the routing pool, governed by the HEALTH_TCP_INTERVAL_SECS env var.
    """
    backend_servers: List[http_utils.StoppableHTTPServer] = []
    stopped_server_info: List[Tuple[int, int]] = []  # (port, original_index)

    try:
        # --- Test Configuration ---
        NUM_BACKENDS = 3
        NUM_FAILOVER_REQUESTS = 10
        NUM_RECOVERY_REQUESTS = 30
        HEALTH_CHECK_INTERVAL = 2
        RECOVERY_WAIT_TIME = HEALTH_CHECK_INTERVAL + 1

        # --- Port and Server Setup ---
        ports = set()
        while len(ports) < NUM_BACKENDS + 1:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = sorted(list(ports))

        targets_str = ",\n".join(
            f'    {{ ip = "127.0.0.1", port = {p} }}' for p in backend_ports
        )

        toml_content = f"""
[[protocols]]
name = "http"
priority = 1
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
{targets_str}
]}} }}
"""
        # --- Start all initial backend servers ---
        for port in backend_ports:
            server = http_utils.StoppableHTTPServer(
                ("127.0.0.1", port), http_utils.RequestRecorderHandler
            )
            server.start()
            if not net_utils.wait_for_tcp_port_ready(port):
                return (
                    False,
                    f"  └─ Details: Initial backend on port {port} failed to start.",
                )
            backend_servers.append(server)

        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {
            "HEALTH_TCP_INTERVAL_SECS": str(HEALTH_CHECK_INTERVAL),
            "LOG_LEVEL": log_level,
        }
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

            # --- Phase 1: Verify Initial State ---
            for i in range(NUM_BACKENDS):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/initial-{i}"])
            initial_counts = [len(s.received_requests) for s in backend_servers]
            if initial_counts != [1, 1, 1]:
                return (
                    False,
                    f"  └─ Details: Initial round-robin failed. Expected [1, 1, 1], got {initial_counts}.",
                )

            # --- Phase 2: Induce Failure and Verify Failover ---
            servers_to_stop = backend_servers[:2]
            num_stopped = len(servers_to_stop)
            for i, server in enumerate(servers_to_stop):
                stopped_server_info.append((backend_ports[i], i))
                server.stop()

            for server in backend_servers:
                server.received_requests.clear()

            # --- THIS SECTION IS CORRECTED ---
            # We no longer wait, and we send all requests immediately. We expect
            # a number of requests equal to the number of failed backends
            # to fail at the client level, as Vane proactively discovers them.
            for i in range(NUM_FAILOVER_REQUESTS):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/failover-{i}"])

            failover_counts = [len(s.received_requests) for s in backend_servers]
            # The correct expectation is that the survivor receives all requests
            # except for the ones that were used to discover the failures.
            expected_requests_to_survivor = NUM_FAILOVER_REQUESTS - num_stopped
            expected_failover_counts = [0, 0, expected_requests_to_survivor]

            if failover_counts != expected_failover_counts:
                reason = (
                    f"Failover routing was incorrect.\n"
                    f"      \n"
                    f"      ├─ Test Scenario\n"
                    f"      │  ├─ Action: {num_stopped} of {NUM_BACKENDS} backends were stopped.\n"
                    f"      │  └─ Vane's Behavior: Vane should proactively detect failures on first contact.\n"
                    f"      └─ Result\n"
                    f"         ├─ Expected Distribution: {expected_failover_counts}\n"
                    f"         └─ Actual Distribution:   {failover_counts}"
                )
                return (False, f"  └─ Details: {reason}")

            # --- Phase 3: Recover Backends and Verify Auto-Recovery ---
            for port, index in stopped_server_info:
                restarted_server = http_utils.StoppableHTTPServer(
                    ("127.0.0.1", port), http_utils.RequestRecorderHandler
                )
                restarted_server.start()
                if not net_utils.wait_for_tcp_port_ready(port):
                    return (
                        False,
                        f"  └─ Details: Restarted backend on port {port} failed to start.",
                    )
                backend_servers[index] = restarted_server

            time.sleep(RECOVERY_WAIT_TIME)

            for server in backend_servers:
                server.received_requests.clear()

            for i in range(NUM_RECOVERY_REQUESTS):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/recovery-{i}"])

            recovery_counts = [len(s.received_requests) for s in backend_servers]
            if any(count == 0 for count in recovery_counts):
                reason = (
                    f"One or more backends were not brought back into service after recovery.\n"
                    f"      \n"
                    f"      ├─ Test Scenario\n"
                    f"      │  ├─ Health Check Interval: {HEALTH_CHECK_INTERVAL}s\n"
                    f"      │  └─ Wait Time After Recovery: {RECOVERY_WAIT_TIME}s\n"
                    f"      └─ Result ({NUM_RECOVERY_REQUESTS} requests sent)\n"
                    f"         ├─ Expected: All backends to receive > 0 requests\n"
                    f"         └─ Actual Distribution: {recovery_counts}"
                )
                return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in backend_servers:
            server.stop()

    return (True, "")
