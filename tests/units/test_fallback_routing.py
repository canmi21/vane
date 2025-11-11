# tests/units/test_fallback_routing.py

from typing import Tuple, List
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests Vane's fallback routing logic through a multi-stage failure scenario.
    It verifies that traffic fails over within the primary `targets` group
    first, then switches to the `fallbacks` group, and finally fails over
    within the `fallbacks` group as well.
    """
    backend_servers: List[http_utils.StoppableHTTPServer] = []
    try:
        # --- Test Configuration ---
        NUM_PRIMARY = 2
        NUM_FALLBACK = 2
        TOTAL_BACKENDS = NUM_PRIMARY + NUM_FALLBACK

        # Phase 1: All Healthy
        NUM_REQ_P1 = 20
        EXPECTED_P1_DIST = [10, 10, 0, 0]

        # Phase 2: One Primary Fails
        NUM_REQ_P2 = 20
        EXPECTED_P2_MIN_HITS = 18

        # Phase 3: All Primaries Fail
        NUM_REQ_P3 = 20
        EXPECTED_P3_MIN_HITS = 16

        # Phase 4: One Fallback Fails
        NUM_REQ_P4 = 20
        EXPECTED_P4_MIN_HITS = 16

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

            # --- Phase 2: One Primary Fails ---
            backend_servers[0].stop()  # Stop the first primary
            for i in range(NUM_REQ_P2):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/phase2-{i}"])

            counts_p2 = [len(s.received_requests) for s in backend_servers]
            hits_on_remaining_primary = counts_p2[1]
            hits_on_fallbacks = sum(counts_p2[2:])
            if (
                hits_on_remaining_primary < EXPECTED_P2_MIN_HITS
                or hits_on_fallbacks > 0
            ):
                return (
                    False,
                    f"  └─ Details: Phase 2 (One Primary Failed) failed. Hits on remaining primary: {hits_on_remaining_primary} (min {EXPECTED_P2_MIN_HITS} expected). Hits on fallbacks: {hits_on_fallbacks} (0 expected).",
                )
            clear_all_counts()

            # --- Phase 3: All Primaries Fail ---
            backend_servers[1].stop()  # Stop the second primary
            for i in range(NUM_REQ_P3):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/phase3-{i}"])

            counts_p3 = [len(s.received_requests) for s in backend_servers]
            hits_on_fallbacks_p3 = sum(counts_p3[2:])
            if hits_on_fallbacks_p3 < EXPECTED_P3_MIN_HITS:
                return (
                    False,
                    f"  └─ Details: Phase 3 (All Primaries Failed) failed. Total hits on fallbacks: {hits_on_fallbacks_p3} (min {EXPECTED_P3_MIN_HITS} expected).",
                )
            clear_all_counts()

            # --- Phase 4: One Fallback Fails ---
            backend_servers[2].stop()  # Stop the first fallback
            for i in range(NUM_REQ_P4):
                http_utils.send_test_requests(vane_port, ["GET"], [f"/phase4-{i}"])

            counts_p4 = [len(s.received_requests) for s in backend_servers]
            hits_on_last_survivor = counts_p4[3]
            if hits_on_last_survivor < EXPECTED_P4_MIN_HITS:
                return (
                    False,
                    f"  └─ Details: Phase 4 (One Fallback Failed) failed. Hits on last survivor: {hits_on_last_survivor} (min {EXPECTED_P4_MIN_HITS} expected).",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        for server in backend_servers:
            server.stop()

    return (True, "")
