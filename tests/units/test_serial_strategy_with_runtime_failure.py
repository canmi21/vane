# tests/units/test_serial_strategy_with_runtime_failure.py

import random
from typing import Tuple, List
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that the 'serial' strategy correctly handles runtime backend
    failures. It verifies initial round-robin distribution, then randomly
    shuts down most backends and ensures the vast majority of subsequent
    traffic is routed to the single survivor.
    """
    backend_servers: List[http_utils.StoppableHTTPServer] = []
    try:
        # --- Test Configuration ---
        NUM_BACKENDS = 3
        NUM_POST_FAILURE_REQUESTS = 30
        MIN_SUCCESS_THRESHOLD = 20

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
        # --- Start all backend servers ---
        for port in backend_ports:
            server = http_utils.StoppableHTTPServer(
                ("127.0.0.1", port), http_utils.RequestRecorderHandler
            )
            server.start()
            if not net_utils.wait_for_tcp_port_ready(port):
                return (
                    False,
                    f"  └─ Details: Backend server on port {port} failed to start.",
                )
            backend_servers.append(server)

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

            # --- Phase 1: Verify Initial Round-Robin ---
            for i in range(NUM_BACKENDS):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/initial-{i}"])

            initial_counts = [len(s.received_requests) for s in backend_servers]
            if initial_counts != [1, 1, 1]:
                return (
                    False,
                    f"  └─ Details: Initial round-robin distribution was incorrect. Expected [1, 1, 1], got {initial_counts}",
                )

            # --- Phase 2: Induce Random Runtime Failure ---
            all_indices = list(range(NUM_BACKENDS))
            survivor_index = random.choice(all_indices)

            the_survivor = backend_servers[survivor_index]
            survivor_port = backend_ports[survivor_index]

            stopped_ports = []
            for i in all_indices:
                if i != survivor_index:
                    backend_servers[i].stop()
                    stopped_ports.append(backend_ports[i])

            # Clear the survivor's log for the next assertion.
            the_survivor.received_requests.clear()

            # --- Phase 3: Verify Routing After Failure ---
            for i in range(NUM_POST_FAILURE_REQUESTS):
                # It's expected that some of these requests might fail as Vane's
                # health checker updates, so we ignore the success status.
                http_utils.send_test_requests(
                    vane_port, ["GET"], [f"/post-failure-{i}"]
                )

            final_count = len(the_survivor.received_requests)
            if final_count < MIN_SUCCESS_THRESHOLD:
                reason = (
                    f"Majority of requests were not routed to the single surviving backend after failure.\n"
                    f"      \n"
                    f"      ├─ Test Scenario\n"
                    f"      │  ├─ Randomly Stopped Backends: {stopped_ports}\n"
                    f"      │  └─ Surviving Backend:         {survivor_port}\n"
                    f"      └─ Result ({NUM_POST_FAILURE_REQUESTS} requests sent)\n"
                    f"         ├─ Expected Requests to Survivor: >= {MIN_SUCCESS_THRESHOLD}\n"
                    f"         └─ Actual Requests to Survivor:   {final_count}"
                )
                return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in backend_servers:
            server.stop()

    return (True, "")
