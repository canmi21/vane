# tests/units/test_flow_engine_tls.py

import socket
import ssl
import time
import pathlib
from typing import Tuple
from utils.template import VaneInstance
from utils import net_utils, tls_utils
from .config_utils import wait_for_log


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the Flow Engine for TLS L4+ Upgrade.
    Verifies that Vane can:
    1. Accept a raw TCP connection.
    2. Detect TLS ClientHello.
    3. Upgrade to 'tls' carrier.
    4. Parse SNI ('lo.ill.li').
    5. Route to the correct backend.
    """
    tls_backend = None
    cert_path = pathlib.Path("tests/cert.pem")
    key_path = pathlib.Path("tests/key.pem")

    try:
        # --- Generate Certs ---
        tls_utils.generate_self_signed_cert(cert_path, key_path)

        # --- Port Setup ---
        vane_port = net_utils.find_available_tcp_port()
        backend_port = net_utils.find_available_tcp_port()

        # --- Start TLS Backend Server ---
        tls_backend = net_utils.TLSConnectionRecorderServer(
            ("127.0.0.1", backend_port),
            net_utils.ConnectionRecorderHandler,
            certfile=cert_path,
            keyfile=key_path,
        )
        tls_backend.start()

        # --- Vane Configuration ---
        # L4 Listener Config (TCP)
        l4_yaml = """
connection:
  internal.protocol.detect:
    input:
      method: "tls"
      payload: "{{req.peek_buffer_hex}}"
    output:
      "true":
        internal.transport.upgrade:
          input:
            protocol: "tls"
      "false":
        internal.transport.abort:
          input: {}
"""
        # L4+ Resolver Config (TLS)
        # Note: We route based on the extracted SNI
        resolver_yaml = f"""
connection:
  internal.common.match:
    input:
      left: "{{{{tls.sni}}}}"
      right: "lo.ill.li"
      operator: "eq"
    output:
      "true":
        internal.transport.proxy:
          input:
            target.ip: "127.0.0.1"
            target.port: {backend_port}
      "false":
        internal.transport.abort:
          input: {{}}
"""

        # --- Configure and Start Vane ---
        log_level = "debug" if debug_mode else "info"
        env_vars = {"LOG_LEVEL": log_level}
        vane = VaneInstance(env_vars, "", debug_mode)

        # Create L4 Listener
        (vane.tmpdir / "listener" / f"[{vane_port}]").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "listener" / f"[{vane_port}]" / "tcp.yaml").write_text(l4_yaml)

        # Create L4+ Resolver
        (vane.tmpdir / "resolver").mkdir(parents=True, exist_ok=True)
        (vane.tmpdir / "resolver" / "tls.yaml").write_text(resolver_yaml)

        with vane:
            up_string = f"PORT {vane_port} TCP UP"
            if not wait_for_log(vane, up_string, 10):
                return (
                    False,
                    f"  └─ Details: Vane did not start listener on TCP port {vane_port}.",
                )

            # --- Send TLS Traffic ---
            # 1. Valid Request (Correct SNI)
            context = ssl.create_default_context()
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE

            try:
                with socket.create_connection(
                    ("127.0.0.1", vane_port), timeout=2
                ) as sock:
                    with context.wrap_socket(
                        sock, server_hostname="lo.ill.li"
                    ) as ssock:
                        ssock.sendall(b"PING")
            except Exception as e:
                return (False, f"  └─ Details: Failed to establish TLS connection: {e}")

            # 2. Invalid Request (Wrong SNI) - Should be aborted
            try:
                with socket.create_connection(
                    ("127.0.0.1", vane_port), timeout=2
                ) as sock:
                    with context.wrap_socket(
                        sock, server_hostname="wrong.com"
                    ) as ssock:
                        ssock.sendall(b"PING")
            except (ConnectionResetError, ssl.SSLError, socket.timeout):
                # Expected behavior: connection aborted or closed
                pass

            time.sleep(0.5)

        # --- Final Assertions ---
        # Should record exactly 1 successful connection (the valid SNI one)
        total_conns = tls_backend.connection_count

        if total_conns != 1:
            return (
                False,
                f"  └─ Details: Backend received {total_conns} connections, expected 1.",
            )

    except Exception as e:
        return (False, f"  └─ Details: Unexpected exception: {e}")
    finally:
        if tls_backend:
            tls_backend.stop()
        # Cleanup certs
        if cert_path.exists():
            cert_path.unlink()
        if key_path.exists():
            key_path.unlink()

    return (True, "")
