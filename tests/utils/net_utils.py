# tests/utils/net_utils.py

from __future__ import annotations
import socket
import random
import threading
import socketserver
import time
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


# --- Connection Recorder Server (Existing) ---
class ConnectionRecorderHandler(socketserver.BaseRequestHandler):
    @property
    def _recorder_server(self) -> ConnectionRecorderTCPServer:
        return cast(ConnectionRecorderTCPServer, self.server)

    def handle(self):
        self._recorder_server.connection_count += 1
        try:
            while True:
                data = self.request.recv(1024)
                if not data:
                    break
        except (ConnectionResetError, BrokenPipeError):
            pass
        finally:
            self.request.close()


class ConnectionRecorderTCPServer(socketserver.TCPServer):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.connection_count, self._thread = 0, None
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


# --- Slow TCP Server ---
class SlowConnectionHandler(socketserver.BaseRequestHandler):
    """A handler that introduces a delay then waits for the client to close."""

    @property
    def _slow_server(self) -> SlowTCPServer:
        return cast(SlowTCPServer, self.server)

    def handle(self):
        self._slow_server.connection_count += 1
        time.sleep(self._slow_server.delay_sec)
        # Robustly wait for the client to finish, preventing reset errors.
        try:
            while True:
                data = self.request.recv(1024)
                if not data:
                    break
        except (ConnectionResetError, BrokenPipeError):
            pass
        finally:
            self.request.close()


class SlowTCPServer(socketserver.TCPServer):
    """A TCP server that intentionally delays responding."""

    def __init__(self, server_address, RequestHandlerClass, delay_sec: float = 0.1):
        super().__init__(server_address, RequestHandlerClass)
        self.delay_sec, self.connection_count, self._thread = delay_sec, 0, None
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
