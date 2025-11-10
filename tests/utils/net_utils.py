# tests/utils/net_utils.py

import socket
import random


def is_udp_port_taken(port: int) -> bool:
    """Checks if a given UDP port is currently in use."""
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        try:
            s.bind(("127.0.0.1", port))
        except OSError:
            return True
    return False


def find_available_udp_port() -> int:
    """Finds a random, available UDP port in the ephemeral range."""
    for _ in range(100):
        port = random.randint(49152, 65535)
        if not is_udp_port_taken(port):
            return port
    raise RuntimeError("Could not find an available UDP port.")


def is_tcp_port_taken(port: int) -> bool:
    """Checks if a given TCP port is currently in use."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        try:
            # Bind tells us if the port is available to listen on
            s.bind(("127.0.0.1", port))
        except OSError:
            return True
    return False


def find_available_tcp_port() -> int:
    """Finds a random, available TCP port in the ephemeral range."""
    for _ in range(100):
        port = random.randint(49152, 65535)
        if not is_tcp_port_taken(port):
            return port
    raise RuntimeError("Could not find an available TCP port.")
