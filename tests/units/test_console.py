# tests/units/test_console.py

import re
import requests
import socket
import json
from typing import Tuple
from utils.template import VaneInstance


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests the management console via both HTTP and Unix Domain Socket.
    """
    try:
        # --- Scenario 1: Test via HTTP endpoint ---
        http_ready_string = "✓ TCP console bound to"
        with VaneInstance({}, http_ready_string, debug_mode) as vane:
            event_was_set = vane.found_event.wait(timeout=10)
            if not event_was_set:
                log_dump = "".join(vane.captured_output)
                reason = "Timeout waiting for HTTP server to start."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

            try:
                # Add Authorization header
                headers = {"Authorization": f"Bearer {vane.access_token}"}
                response = requests.get(
                    "http://localhost:3333/", headers=headers, timeout=5
                )
                response.raise_for_status()
                data = response.json()
                if data.get("status") != "success":
                    reason = (
                        f"Expected status 'success', but got '{data.get('status')}'."
                    )
                    return (
                        False,
                        f"  └─ Details: {reason}\n\n--- HTTP Response ---\n{response.text}",
                    )
            except requests.exceptions.RequestException as e:
                return (False, f"  └─ Details: HTTP request failed: {e}")

        # --- Scenario 2: Test via Unix Domain Socket ---
        uds_ready_string = "✓ Management console listening on unix:"
        with VaneInstance({}, uds_ready_string, debug_mode) as vane:
            event_was_set = vane.found_event.wait(timeout=10)
            if not event_was_set:
                log_dump = "".join(vane.captured_output)
                reason = "Timeout waiting for Unix Domain Socket to be ready."
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

            uds_path = None
            for line in vane.captured_output:
                match = re.search(r"unix:(/.+)", line)
                if match:
                    uds_path = match.group(1).strip()
                    break

            if not uds_path:
                return (
                    False,
                    "  └─ Details: Could not parse the Unix Domain Socket path from logs.",
                )

            try:
                sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                sock.connect(uds_path)

                # Construct raw HTTP request with Authorization header
                request_str = (
                    f"GET / HTTP/1.1\r\n"
                    f"Host: localhost\r\n"
                    f"Connection: close\r\n"
                    f"Authorization: Bearer {vane.access_token}\r\n"
                    f"\r\n"
                )
                sock.sendall(request_str.encode("utf-8"))

                chunks = []
                while True:
                    chunk = sock.recv(4096)
                    if not chunk:
                        break
                    chunks.append(chunk)

                sock.close()
                response_bytes = b"".join(chunks)
                response_raw = response_bytes.decode("utf-8", errors="ignore")

                # Robustly parse the JSON body instead of string matching
                try:
                    _header, json_body = response_raw.split("\r\n\r\n", 1)
                    data = json.loads(json_body)
                    if data.get("status") != "success":
                        reason = f"Expected status 'success', but got '{data.get('status')}'."
                        return (
                            False,
                            f"  └─ Details: {reason}\n\n--- UDS Raw Response ---\n{response_raw}",
                        )
                except (ValueError, json.JSONDecodeError) as e:
                    reason = f"Failed to parse JSON from UDS response: {e}"
                    return (
                        False,
                        f"  └─ Details: {reason}\n\n--- UDS Raw Response ---\n{response_raw}",
                    )

            except Exception as e:
                return (
                    False,
                    f"  └─ Details: Unix Domain Socket communication failed: {e}",
                )

    except Exception as e:
        return (
            False,
            f"  └─ Details: An unexpected exception occurred during the test: {e}",
        )

    return (True, "")
