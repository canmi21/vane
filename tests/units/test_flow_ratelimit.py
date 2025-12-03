# tests/units/test_flow_ratelimit.py

import time
import socket
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log
from .flow_configs import ratelimit_and_route


def send_single_raw_http_request(port: int) -> bool:
    """
    Sends a single, raw HTTP request using a low-level socket.
    Returns True if a response is received, False if the connection is reset.
    """
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.5)
            s.connect(("127.0.0.1", port))
            s.sendall(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n")
            data = s.recv(1024)
            return bool(data)
    except (ConnectionResetError, BrokenPipeError, socket.timeout):
        return False


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the 'internal.common.ratelimit.sec' plugin by sending a rapid
    burst of traffic and asserting that the number of successful connections
    falls within a reasonable, non-deterministic range.
    """
    http_server = None
    try:
        # --- Test Configuration ---
        RATE_LIMIT = 2
        BURST_REQUESTS = 20
        RECOVERY_REQUESTS = 2
        WAIT_SECONDS = 1.5

        # Due to the inherent race condition between the client's burst and the
        # server's internal 1-second tick, we assert a range, not a fixed number.
        EXPECTED_MIN_HITS = RATE_LIMIT
        EXPECTED_MAX_HITS = 5  # Allow up to 4 hits (e.g., 2 at the end of a
        # window, 2 at the start of the next). < 5 means max 4.

        # --- Port and Server Setup ---
        vane_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        # --- Start Backend Server ---
        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(backend_port):
            return (
                False,
                f"  └─ Details: HTTP backend on port {backend_port} failed to start.",
            )

        # --- Vane Configuration ---
        flow_yaml = ratelimit_and_route.generate_config(backend_port, RATE_LIMIT)

        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.yaml").write_text(flow_yaml)

        successful_sends_phase1 = 0
        successful_sends_phase3 = 0

        with vane:
            up_string = f"PORT {vane_port} TCP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on port {vane_port}.",
                )

            # --- PHASE 1: Send a rapid burst of traffic ---
            # No sleep is used here to maximize the chance of hitting the limit.
            for _ in range(BURST_REQUESTS):
                if send_single_raw_http_request(vane_port):
                    successful_sends_phase1 += 1

            # --- PHASE 2: Wait for Recovery ---
            time.sleep(WAIT_SECONDS)

            # --- PHASE 3: Verify Recovery ---
            # Send requests with a small delay to ensure they are processed discretely.
            for _ in range(RECOVERY_REQUESTS):
                if send_single_raw_http_request(vane_port):
                    successful_sends_phase3 += 1
                time.sleep(0.1)

        # --- Final Assertions ---
        http_hits = len(http_server.received_requests)
        total_successful_sends = successful_sends_phase1 + successful_sends_phase3

        is_phase1_ok = EXPECTED_MIN_HITS <= successful_sends_phase1 < EXPECTED_MAX_HITS
        is_phase3_ok = successful_sends_phase3 == RECOVERY_REQUESTS
        is_total_ok = http_hits == total_successful_sends

        if not (is_phase1_ok and is_phase3_ok and is_total_ok):
            reason = (
                f"Rate-limiting test with bounded assertion failed.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Rate Limit: {RATE_LIMIT} requests per second\n"
                f"      │  ├─ Phase 1: Sent {BURST_REQUESTS} raw requests in a tight loop.\n"
                f"      │  ├─ Phase 2: Waited {WAIT_SECONDS} seconds.\n"
                f"      │  └─ Phase 3: Sent {RECOVERY_REQUESTS} more discrete requests.\n"
                f"      └─ Result\n"
                f"         ├─ Successful sends in Phase 1: {successful_sends_phase1} (Expected: between {EXPECTED_MIN_HITS} and {EXPECTED_MAX_HITS - 1})\n"
                f"         ├─ Successful sends in Phase 3: {successful_sends_phase3} (Expected: {RECOVERY_REQUESTS})\n"
                f"         └─ Total Backend Hits:          {http_hits} (Expected: {total_successful_sends})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if http_server:
            http_server.stop()

    return (True, "")
