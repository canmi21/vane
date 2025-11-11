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


# --- Connection Recorder Servers (TCP) ---
class ConnectionRecorderHandler(socketserver.BaseRequestHandler):
    @property
    def _recorder_server(self) -> ConnectionRecorderTCPServer:
        return cast(ConnectionRecorderTCPServer, self.server)

    def handle(self):
        with self._recorder_server.count_lock:
            self._recorder_server.connection_count += 1
        try:
            self.request.recv(1024)
        except (ConnectionResetError, BrokenPipeError, ssl.SSLError):
            pass
        finally:
            self.request.close()


class ConnectionRecorderTCPServer(socketserver.ThreadingTCPServer):
    """A multi-threaded TCP server that records the total number of connections."""

    def __init__(self, server_address, RequestHandlerClass):
        # Using an explicit signature is more robust than generic *args, **kwargs
        # for ensuring the handler is correctly registered by the base class.
        super().__init__(server_address, RequestHandlerClass)
        self.connection_count = 0
        self._thread = None
        self.allow_reuse_address = True
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
    """A multi-threaded TLS server that records the total number of connections."""

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
        sock, addr = self.socket.accept()
        return self.ssl_context.wrap_socket(sock, server_side=True), addr


# --- UDP Test Servers ---
class PredefinedResponseUDPHandler(socketserver.BaseRequestHandler):
    """A UDP handler that sends a single, predefined response to any request."""

    def handle(self):
        socket = self.request[1]
        server = cast(ResponseUDPServer, self.server)
        socket.sendto(server.response_data, self.client_address)


class ResponseUDPServer(socketserver.ThreadingUDPServer):
    """A multi-threaded UDP server that gives a fixed response to any query."""

    def __init__(self, server_address, RequestHandlerClass, response_data: bytes):
        super().__init__(server_address, RequestHandlerClass)
        self.response_data = response_data
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


class PacketRecorderUDPHandler(socketserver.BaseRequestHandler):
    """A UDP handler that simply counts received packets thread-safely."""

    def handle(self):
        server = cast(PacketRecorderUDPServer, self.server)
        with server.count_lock:
            server.packet_count += 1


class PacketRecorderUDPServer(socketserver.ThreadingUDPServer):
    """A multi-threaded UDP server that counts the total number of packets received."""

    def __init__(self, server_address, RequestHandlerClass):
        # This explicit __init__ signature is crucial. The generic *args, **kwargs
        # can cause the RequestHandlerClass to not be properly registered by the
        # underlying socketserver, turning the server into a black hole.
        super().__init__(server_address, RequestHandlerClass)
        self.packet_count = 0
        self._thread = None
        self.allow_reuse_address = True
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


# --- Slow TCP Server for Latency Simulation ---
class SlowTCPHandler(socketserver.BaseRequestHandler):
    """A handler that simulates a slow server by introducing a delay."""

    @property
    def _slow_server(self) -> SlowTCPServer:
        return cast(SlowTCPServer, self.server)

    def handle(self):
        time.sleep(self._slow_server.delay_sec)
        try:
            while self.request.recv(1024):
                pass
        except (ConnectionResetError, BrokenPipeError, ConnectionAbortedError):
            pass
        finally:
            self.request.close()


class SlowTCPServer(socketserver.ThreadingTCPServer):
    """A multi-threaded TCP server that uses SlowTCPHandler to simulate latency."""

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
