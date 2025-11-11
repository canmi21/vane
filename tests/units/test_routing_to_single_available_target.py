# tests/units/test_routing_to_single_available_target.py

from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that Vane correctly routes all traffic to the single available
    target when all other configured targets are down (ports are not listening).
    """
    good_server = None
    try:
        # --- Preparation ---
        NUM_TOTAL_TARGETS = 3
        NUM_REQUESTS = 5

        # Find ports for all backend targets plus the Vane listener.
        ports = set()
        while len(ports) < NUM_TOTAL_TARGETS + 1:
            ports.add(net_utils.find_available_tcp_port())

        vane_port = ports.pop()
        backend_ports = sorted(list(ports))  # Sort for deterministic behavior

        # Designate the first port as the only available target.
        good_target_port = backend_ports[0]
        bad_target_ports = backend_ports[1:]

        # --- Start the single available backend server ---
        good_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", good_target_port), http_utils.RequestRecorderHandler
        )
        good_server.start()
        if not net_utils.wait_for_tcp_port_ready(good_target_port):
            return (
                False,
                f"  └─ Details: The single available backend on port {good_target_port} failed to start.",
            )

        # The other ports (bad_target_ports) are intentionally left down.

        # --- Configure Vane to target all backends ---
        targets_str = ",\n".join(
            f'    {{ ip = "127.0.0.1", port = {p} }}' for p in backend_ports
        )

        # The 'serial' strategy is used, but Vane's health checker should
        # ensure that it skips the dead targets regardless of the strategy.
        toml_content = f"""
[[protocols]]
name = "http"
priority = 1
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
{targets_str}
]}} }}
"""
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

            # --- Send Test Requests ---
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
        received_count = len(good_server.received_requests)
        if received_count != NUM_REQUESTS:
            reason = (
                f"Incorrect number of requests routed to the only available backend.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ Available Backend:    Port {good_target_port}\n"
                f"      │  ├─ Unavailable Backends: Ports {bad_target_ports}\n"
                f"      │  └─ Vane Port:            {vane_port}\n"
                f"      └─ Result\n"
                f"         ├─ Total Requests Sent:    {NUM_REQUESTS}\n"
                f"         └─ Requests Received:      {received_count}"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if good_server:
            good_server.stop()

    return (True, "")
