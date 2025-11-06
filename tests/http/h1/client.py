# tests/http/h1/client.py

import sys
from pathlib import Path

# --- Module Import Setup ---
# Add the project root directory to the Python path to find the utils module.
project_root = Path(__file__).resolve().parents[3]
sys.path.append(str(project_root))

from tests.utils.env import get_env
from tests.http.h1.h1 import send_request as send_h1_request
from tests.http.h1.h1_1 import send_request as send_h1_1_request

# --- Configuration ---
HOST = "127.0.0.1"

# Manually get the port from .env and fall back to 80 if it's not set.
port_from_env = get_env("BIND_HTTP_PORT")
if port_from_env is False:
    PORT = 80
else:
    PORT = int(port_from_env)

def main():
    """
    Runs a sequence of HTTP/1 tests against the Vane engine.
    """
    # --- Test 1: HTTP/1.0 ---
    send_h1_request(HOST, PORT)

    # --- Test 2: HTTP/1.1 ---
    send_h1_1_request(HOST, PORT)


if __name__ == "__main__":
    main()