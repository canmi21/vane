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
            time.sleep(0.1)  # Wait a bit before retrying
    return False


# --- Connection Recorder Server ---
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


# --- FINAL, CORRECT SLOW TCP SERVER IMPLEMENTATION ---


class SlowTCPHandler(socketserver.BaseRequestHandler):
    """
    A handler that correctly simulates a slow server. It introduces a delay
    IMMEDIATELY upon connection acceptance, then waits passively for the client
    to send data or close the connection. IT DOES NOT CLOSE THE CONNECTION ITSELF.
    """

    @property
    def _slow_server(self) -> SlowTCPServer:
        return cast(SlowTCPServer, self.server)

    def handle(self):
        # 1. Inject delay immediately. This is the crucial part. Any attempt by
        #    the client to write data will be blocked for this duration.
        time.sleep(self._slow_server.delay_sec)

        # 2. Passively wait for data or client-side close. This prevents the
        #    "Connection reset by peer" error and correctly mimics a slow,
        #    but not broken, server.
        try:
            while True:
                data = self.request.recv(1024)
                if not data:
                    # Client closed the connection gracefully.
                    break
        except (ConnectionResetError, BrokenPipeError, ConnectionAbortedError):
            # Client closed the connection abruptly.
            pass
        finally:
            self.request.close()


class SlowTCPServer(socketserver.TCPServer):
    """
    A TCP server that uses the SlowTCPHandler to correctly simulate latency
    at the application layer, making it perceptible to proxy servers.
    """

    def __init__(self, server_address, RequestHandlerClass, delay_sec: float = 0.1):
        super().__init__(server_address, RequestHandlerClass)
        self.delay_sec = delay_sec
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
