# tests/units/test_fallback_auto_recovery.py

import time
from typing import Tuple, List
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that Vane fails over to fallback targets when all primaries are
    down, and then correctly recovers when the primary targets come back online.
    """
    backend_servers: List[http_utils.StoppableHTTPServer] = []
    stopped_primary_info: List[Tuple[int, int]] = []  # (port, original_index)

    try:
        # --- Test Configuration ---
        NUM_PRIMARY = 2
        NUM_FALLBACK = 1
        TOTAL_BACKENDS = NUM_PRIMARY + NUM_FALLBACK
        HEALTH_CHECK_INTERVAL = 2

        # Phase 1: All Healthy
        NUM_REQ_P1 = 20
        EXPECTED_P1_DIST = [10, 10, 0]

        # Phase 2: Failover to Fallback
        NUM_REQ_P2 = 20
        EXPECTED_P2_MIN_HITS = 16

        # Phase 3: Auto-Recovery
        NUM_REQ_P3 = 30
        EXPECTED_P3_MIN_HITS_EACH = 10
        EXPECTED_P3_MIN_HITS_TOTAL = 20

        # --- Port and Server Setup ---
        ports = set()
        while len(ports) < TOTAL_BACKENDS + 1:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = sorted(list(ports))

        primary_ports = backend_ports[:NUM_PRIMARY]
        fallback_ports = backend_ports[NUM_PRIMARY:]

        primary_targets_str = ",\n".join(
            f'    {{ ip = "127.0.0.1", port = {p} }}' for p in primary_ports
        )
        fallback_targets_str = ",\n".join(
            f'    {{ ip = "127.0.0.1", port = {p} }}' for p in fallback_ports
        )

        toml_content = f"""
[[protocols]]
name = "http"
priority = 1
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
{primary_targets_str}
], fallbacks = [
{fallback_targets_str}
]}} }}
"""
        # --- Start all backend servers ---
        for port in backend_ports:
            server = http_utils.StoppableHTTPServer(
                ("127.0.0.1", port), http_utils.RequestRecorderHandler
            )
            server.start()
            if not net_utils.wait_for_tcp_port_ready(port):
                return (False, f"  └─ Details: Backend on port {port} failed to start.")
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

            def clear_all_counts():
                for s in backend_servers:
                    s.received_requests.clear()

            # --- Phase 1: All Healthy ---
            for i in range(NUM_REQ_P1):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/phase1-{i}"])
            counts_p1 = [len(s.received_requests) for s in backend_servers]
            if counts_p1 != EXPECTED_P1_DIST:
                return (
                    False,
                    f"  └─ Details: Phase 1 (All Healthy) failed. Expected {EXPECTED_P1_DIST}, got {counts_p1}.",
                )
            clear_all_counts()

            # --- Phase 2: Failover to Fallback ---
            for i in range(NUM_PRIMARY):
                stopped_primary_info.append((primary_ports[i], i))
                backend_servers[i].stop()
            for i in range(NUM_REQ_P2):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/phase2-{i}"])
            counts_p2 = [len(s.received_requests) for s in backend_servers]
            hits_on_fallback = counts_p2[NUM_PRIMARY]
            if hits_on_fallback < EXPECTED_P2_MIN_HITS:
                return (
                    False,
                    f"  └─ Details: Phase 2 (Failover) failed. Hits on fallback: {hits_on_fallback} (min {EXPECTED_P2_MIN_HITS} expected).",
                )
            clear_all_counts()

            # --- Phase 3: Auto-Recovery of Primaries ---
            for port, index in stopped_primary_info:
                restarted_server = http_utils.StoppableHTTPServer(
                    ("127.0.0.1", port), http_utils.RequestRecorderHandler
                )
                restarted_server.start()
                if not net_utils.wait_for_tcp_port_ready(port):
                    return (
                        False,
                        f"  └─ Details: Restarted primary on port {port} failed.",
                    )
                backend_servers[index] = restarted_server

            time.sleep(HEALTH_CHECK_INTERVAL + 1)

            for i in range(NUM_REQ_P3):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/phase3-{i}"])

            counts_p3 = [len(s.received_requests) for s in backend_servers]
            primary1_hits = counts_p3[0]
            primary2_hits = counts_p3[1]
            total_primary_hits = primary1_hits + primary2_hits

            if (
                primary1_hits < EXPECTED_P3_MIN_HITS_EACH
                or primary2_hits < EXPECTED_P3_MIN_HITS_EACH
                or total_primary_hits < EXPECTED_P3_MIN_HITS_TOTAL
            ):
                reason = (
                    f"Primary backends did not auto-recover correctly.\n"
                    f"      \n"
                    f"      ├─ Test Scenario\n"
                    f"      │  ├─ Action: Restarted primary backends {primary_ports}.\n"
                    f"      │  └─ Wait Time: {HEALTH_CHECK_INTERVAL + 1}s (Interval: {HEALTH_CHECK_INTERVAL}s)\n"
                    f"      └─ Result ({NUM_REQ_P3} requests sent)\n"
                    f"         ├─ Primary #1 Hits: {primary1_hits} (min {EXPECTED_P3_MIN_HITS_EACH} expected)\n"
                    f"         ├─ Primary #2 Hits: {primary2_hits} (min {EXPECTED_P3_MIN_HITS_EACH} expected)\n"
                    f"         └─ Total Primary Hits: {total_primary_hits} (min {EXPECTED_P3_MIN_HITS_TOTAL} expected)"
                )
                return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in backend_servers:
            server.stop()

    return (True, "")
