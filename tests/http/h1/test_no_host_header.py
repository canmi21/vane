# tests/http/h1/test_no_host_header.py

import socket
import sys
from pathlib import Path

# --- Module Import Setup ---
project_root = Path(__file__).resolve().parents[3]
sys.path.append(str(project_root))

from tests.utils.env import get_env

# --- Configuration ---
HOST = "127.0.0.1"
port_from_env = get_env("BIND_HTTP_PORT")
PORT = int(port_from_env) if port_from_env is not False else 80

def test_protocol(version: str):
    """
    Attempts to send a request for a specific HTTP version without a Host header.
    The function returns silently on success (ConnectionResetError) and raises
    an AssertionError on any other outcome.
    """
    request_text = (
        f"GET / HTTP/{version}\r\n"
        f"User-Agent: Vane-No-Host-Test/{version}\r\n"
        f"Connection: close\r\n"
        f"\r\n"
    )

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.connect((HOST, PORT))
            s.sendall(request_text.encode('utf-8'))

            # The server should immediately reset the connection.
            # If we receive any data, it's a failure.
            data = s.recv(1024)

            if data:
                print(f"FAIL (HTTP/{version}): Server sent an unexpected response: {data.decode()}")
                raise AssertionError("Server should have reset the connection, but sent data instead.")
            else:
                # This means the server closed the connection gracefully.
                print(f"FAIL (HTTP/{version}): Server closed the connection gracefully instead of resetting it.")
                raise AssertionError("Expected ConnectionResetError, but connection was closed gracefully.")

    except ConnectionResetError:
        # This is the expected success case. The server correctly reset the connection.
        # Do nothing and return silently.
        pass
    except ConnectionRefusedError:
        print(f"FAIL (HTTP/{version}): Connection refused. Is the Vane engine running on port {PORT}?")
        raise AssertionError("Test environment is not ready.")
    except Exception as e:
        print(f"FAIL (HTTP/{version}): An unexpected error occurred: {e}")
        raise AssertionError(f"Expected ConnectionResetError, got {type(e).__name__}.")


def run():
    """
    Runs the no-host-header test for both HTTP/1.0 and HTTP/1.1.
    If any test fails, it will raise an exception and terminate the script.
    """
    test_protocol("1.0")
    test_protocol("1.1")


if __name__ == "__main__":
    run()