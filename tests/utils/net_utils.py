# tests/utils/net_utils.py

from __future__ import annotations
import socket
import random
import threading
import socketserver
import time
import ssl
import pathlib
from typing import cast, Dict


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


def wait_for_port(port: int, timeout: float = 2.0) -> bool:
    """
    Generic alias for waiting for a TCP port to be open.
    Used by integration tests to verify API availability.
    """
    return wait_for_tcp_port_ready(port, timeout)


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
        sock = self.request[1]
        server = cast(ResponseUDPServer, self.server)
        sock.sendto(server.response_data, self.client_address)


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


class CustomDnsHandler(socketserver.BaseRequestHandler):
    """
    A simple DNS handler that responds to A record queries for specific
    domains with pre-configured IP addresses.
    """

    def handle(self):
        data, sock = self.request
        server = cast(CustomDnsServer, self.server)

        # Extract the transaction ID from the query.
        transaction_id = data[:2]

        # A crude but effective way to find the queried domain.
        # It finds the first null byte after the header and extracts the labels.
        qname_end_idx = data.find(b"\x00", 12)
        if qname_end_idx == -1:
            return

        # Format: [len]label[len]label...
        qname_bytes = data[12 : qname_end_idx + 1]
        labels = []
        i = 0
        while i < len(qname_bytes):
            length = qname_bytes[i]
            if length == 0:
                break
            labels.append(qname_bytes[i + 1 : i + 1 + length].decode("utf-8"))
            i += 1 + length
        domain = ".".join(labels)

        # Check if we have a record for this domain.
        if domain in server.a_records:
            ip_str = server.a_records[domain]
            ip_bytes = socket.inet_aton(ip_str)

            # Construct the DNS response.
            response = (
                transaction_id
                + b"\x81\x80"  # Flags: Standard query response, no error
                + b"\x00\x01"  # Questions: 1
                + b"\x00\x01"  # Answer RRs: 1
                + b"\x00\x00"  # Authority RRs: 0
                + b"\x00\x00"  # Additional RRs: 0
                + qname_bytes  # The original question
                + b"\x00\x01"  # Type: A
                + b"\x00\x01"  # Class: IN
                + b"\xc0\x0c"  # Pointer to the question name
                + b"\x00\x01"  # Type: A
                + b"\x00\x01"  # Class: IN
                + b"\x00\x00\x00\x3c"  # TTL: 60 seconds
                + b"\x00\x04"  # Data length: 4 bytes
                + ip_bytes  # The IP address
            )
            sock.sendto(response, self.client_address)


class CustomDnsServer(socketserver.ThreadingUDPServer):
    """A configurable, multi-threaded DNS server for testing custom resolvers."""

    def __init__(self, server_address, RequestHandlerClass, a_records: Dict[str, str]):
        super().__init__(server_address, RequestHandlerClass)
        self.a_records = a_records
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
