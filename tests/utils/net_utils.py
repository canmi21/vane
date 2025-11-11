# tests/utils/net_utils.py

from __future__ import annotations
import socket
import random
import threading
import socketserver
import time
import ssl
import pathlib
from typing import cast


# --- Port Finding Functions ---
def is_udp_port_taken(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        try:
            s.bind(("127.0.0.1", port))
        except OSError:
            return True
    return False


def find_available_udp_port() -> int:
    for _ in range(100):
        port = random.randint(49152, 65535)
        if not is_udp_port_taken(port):
            return port
    raise RuntimeError("Could not find an available UDP port.")


def is_tcp_port_taken(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        try:
            s.bind(("127.0.0.1", port))
        except OSError:
            return True
    return False


def find_available_tcp_port() -> int:
    for _ in range(100):
        port = random.randint(49152, 65535)
        if not is_tcp_port_taken(port):
            return port
    raise RuntimeError("Could not find an available TCP port.")


def wait_for_tcp_port_ready(port: int, timeout: float = 2.0) -> bool:
    """
    Waits for a TCP port to become ready and accept a connection. This is a
    reliable way to ensure a server has started before proceeding.
    """
    start_time = time.monotonic()
    while time.monotonic() - start_time < timeout:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                return True
        except (socket.timeout, ConnectionRefusedError):
            time.sleep(0.1)
    return False


# --- Connection Recorder Servers ---
class ConnectionRecorderHandler(socketserver.BaseRequestHandler):
    @property
    def _recorder_server(self) -> ConnectionRecorderTCPServer:
        return cast(ConnectionRecorderTCPServer, self.server)

    def handle(self):
        # The lock ensures that in a concurrent environment, the counter
        # increment is atomic, preventing race conditions.
        with self._recorder_server.count_lock:
            self._recorder_server.connection_count += 1
        try:
            # The handler's responsibility is now to simply accept, count,
            # consume any incoming data, and then immediately close the
            # connection. This creates a predictable lifecycle.
            self.request.recv(1024)
        except (ConnectionResetError, BrokenPipeError, ssl.SSLError):
            # Client disconnected abruptly or a TLS error occurred.
            pass
        finally:
            # By design, the server now always closes the connection first.
            self.request.close()


class ConnectionRecorderTCPServer(socketserver.ThreadingTCPServer):
    """
    A multi-threaded TCP server that records the total number of connections.
    It uses ThreadingTCPServer to handle multiple simultaneous connections
    robustly, which is essential for accurate testing under load.
    """

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.connection_count = 0
        self._thread = None
        self.allow_reuse_address = True
        # A lock is crucial to protect the counter during concurrent access
        # from multiple handler threads.
        self.count_lock = threading.Lock()

    def start(self):
        self._thread = threading.Thread(target=self.serve_forever)
        self._thread.daemon = True
        self._thread.start()

    def stop(self):
        if self._thread:
            self.shutdown()
            self.server_close()
            self._thread.join()


class TLSConnectionRecorderServer(ConnectionRecorderTCPServer):
    """
    A multi-threaded TLS server that records the total number of connections.
    It wraps incoming connections in a TLS context using a provided
    self-signed certificate.
    """

    def __init__(
        self,
        server_address,
        RequestHandlerClass,
        certfile: pathlib.Path,
        keyfile: pathlib.Path,
    ):
        super().__init__(server_address, RequestHandlerClass)
        self.ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.ssl_context.load_cert_chain(certfile, keyfile)

    def get_request(self):
        """Wraps the accepted socket with the TLS context."""
        sock, addr = self.socket.accept()
        return self.ssl_context.wrap_socket(sock, server_side=True), addr
