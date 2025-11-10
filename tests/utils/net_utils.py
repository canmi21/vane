# tests/utils/net_utils.py

from __future__ import annotations
import socket
import random
import threading
import socketserver
from typing import cast


# --- Port Finding Functions (Existing) ---
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


# --- Connection Recorder Server ---
class ConnectionRecorderHandler(socketserver.BaseRequestHandler):
    """
    A handler that notes a connection was made and then robustly waits for
    the client to close the connection before closing itself.
    """

    @property
    def _recorder_server(self) -> ConnectionRecorderTCPServer:
        return cast(ConnectionRecorderTCPServer, self.server)

    def handle(self):
        self._recorder_server.connection_count += 1
        # --- THIS IS THE ROBUST FIX ---
        # This loop will block and read any data sent by the client (Vane).
        # It will only exit when Vane closes the connection, returning b''.
        # This correctly simulates a real server and prevents reset errors.
        try:
            while True:
                data = self.request.recv(1024)
                if not data:
                    break  # Clean shutdown from client
        except (ConnectionResetError, BrokenPipeError):
            pass  # Or an unclean shutdown, which is also fine.
        finally:
            self.request.close()


class ConnectionRecorderTCPServer(socketserver.TCPServer):
    """A simple TCP server that counts incoming connections."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.connection_count = 0
        self._thread = None
        self.allow_reuse_address = True

    def start(self):
        self._thread = threading.Thread(target=self.serve_forever)
        self._thread.daemon = True
        self._thread.start()

    def stop(self):
        if self._thread:
            self.shutdown()
            self.server_close()
            self._thread.join()
