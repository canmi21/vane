# tests/units/test_capture_all_fallback.py

import random
import socket
import ssl
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, http_utils, tls_utils
from .config_utils import wait_for_log


def send_tls_connections(port: int, count: int):
    """
    Sends a number of simple, non-HTTP TLS connections, skipping certificate
    verification. The stateful TLS handshake makes this method far more
    reliable for testing than sending raw TCP packets.
    """
    client_context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    client_context.check_hostname = False
    client_context.verify_mode = ssl.CERT_NONE

    for _ in range(count):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
                sock.settimeout(2)
                sock.connect(("127.0.0.1", port))
                with client_context.wrap_socket(
                    sock, server_hostname="localhost"
                ) as ssock:
                    ssock.sendall(b"this is generic tls traffic\n")
                    # Gracefully wait for the server to close the connection.
                    # This is crucial for a clean shutdown and prevents the
                    # "Broken pipe" errors in the proxy. A recv() call
                    # returning b'' indicates a graceful close from the peer.
                    while ssock.recv(1024):
                        pass
        except (socket.timeout, ConnectionRefusedError, ssl.SSLError):
            # We can ignore failures here as the main assertions will catch
            # routing issues.
            pass


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests that a high-priority rule is matched before a low-priority 'fallback'
    rule, ensuring traffic is correctly segregated.
    """
    http_server = None
    fallback_server = None
    try:
        # --- Test Configuration ---
        NUM_HTTP_REQUESTS = 10
        NUM_TLS_CONNECTIONS = 10
        MAX_ALLOWED_EXTRA_TCP = 5

        # --- Dynamic Priority Setup ---
        priorities = sorted(random.sample(range(1, 101), 2))
        http_prio = priorities[0]
        fallback_prio = priorities[1]

        # --- Port and Server Setup ---
        ports = set()
        while len(ports) < 2 + 1:
            ports.add(net_utils.find_available_tcp_port())
        vane_port = ports.pop()
        port_list = sorted(list(ports))
        http_backend_port = port_list[0]
        fallback_backend_port = port_list[1]

        # --- Vane Configuration & Startup ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        # Generate self-signed certs within Vane's temp directory for auto-cleanup.
        cert_path = vane.tmpdir / "cert.pem"
        key_path = vane.tmpdir / "key.pem"
        tls_utils.generate_self_signed_cert(cert_path, key_path)

        # --- Start Backend Servers ---
        # 1. The HTTP backend, which records full HTTP requests.
        http_server = http_utils.StoppableHTTPServer(
            ("127.0.0.1", http_backend_port), http_utils.RequestRecorderHandler
        )
        http_server.start()
        if not net_utils.wait_for_tcp_port_ready(http_backend_port):
            return (
                False,
                f"  └─ Details: HTTP backend on port {http_backend_port} failed to start.",
            )

        # 2. The fallback backend, now a TLS server counting TLS connections.
        fallback_server = net_utils.TLSConnectionRecorderServer(
            ("127.0.0.1", fallback_backend_port),
            net_utils.ConnectionRecorderHandler,
            cert_path,
            key_path,
        )
        fallback_server.start()
        if not net_utils.wait_for_tcp_port_ready(fallback_backend_port):
            return (
                False,
                f"  └─ Details: Fallback TLS backend on port {fallback_backend_port} failed to start.",
            )

        # --- Vane Configuration ---
        toml_content = f"""
[[protocols]]
name = "http"
priority = {http_prio}
detect = {{ method = "regex", pattern = "^[A-Z]+ /.* HTTP/1\\\\.[01]" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
    {{ ip = "127.0.0.1", port = {http_backend_port} }}
]}} }}

[[protocols]]
name = "other"
priority = {fallback_prio}
detect = {{ method = "fallback", pattern = "any" }}
destination = {{ type = "forward", forward = {{ strategy = "serial", targets = [
    {{ ip = "127.0.0.1", port = {fallback_backend_port} }}
]}} }}
"""
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

            # --- Send Mixed Traffic ---
            http_utils.send_test_requests(vane_port, ["GET"], ["/"] * NUM_HTTP_REQUESTS)
            send_tls_connections(vane_port, NUM_TLS_CONNECTIONS)

        # --- Final Assertions ---
        http_hits = len(http_server.received_requests)
        fallback_hits = fallback_server.connection_count

        is_fallback_ok = (
            NUM_TLS_CONNECTIONS
            <= fallback_hits
            <= NUM_TLS_CONNECTIONS + MAX_ALLOWED_EXTRA_TCP
        )
        if http_hits != NUM_HTTP_REQUESTS or not is_fallback_ok:
            expected_tls_range = f"between {NUM_TLS_CONNECTIONS} and {NUM_TLS_CONNECTIONS + MAX_ALLOWED_EXTRA_TCP}"
            reason = (
                f"Traffic was not correctly segregated based on priority.\n"
                f"      \n"
                f"      ├─ Test Scenario\n"
                f"      │  ├─ HTTP Rule Priority:     {http_prio} (target: {http_backend_port})\n"
                f"      │  └─ Fallback Rule Priority: {fallback_prio} (target: {fallback_backend_port})\n"
                f"      ├─ HTTP Traffic\n"
                f"      │  ├─ Sent:     {NUM_HTTP_REQUESTS}\n"
                f"      │  └─ Received: {http_hits} (Expected: {NUM_HTTP_REQUESTS})\n"
                f"      └─ Generic TLS Traffic\n"
                f"         ├─ Sent:     {NUM_TLS_CONNECTIONS}\n"
                f"         └─ Received: {fallback_hits} (Expected: {expected_tls_range})"
            )
            return (False, f"  └─ Details: {reason}")

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")
    finally:
        if http_server:
            http_server.stop()
        if fallback_server:
            fallback_server.stop()

    return (True, "")
