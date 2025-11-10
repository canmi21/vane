# tests/units/config_utils.py

import threading
import time
from typing import Tuple, Dict
from utils.template import VaneInstance

# --- Shared Log Strings ---
PORT_80_TCP_UP = "PORT 80 TCP UP"
PORT_80_UDP_UP = "PORT 80 UDP UP"

# --- TCP Configuration Content ---
JSON_TCP = """
{
  "protocols": [
    {
      "name": "tls", "priority": 1,
      "detect": { "method": "magic", "pattern": "0x16" },
      "session": { "keepalive": true, "timeout": 30000 },
      "destination": { "type": "resolver", "resolver": "tls" }
    },
    {
      "name": "http", "priority": 2,
      "detect": { "method": "prefix", "pattern": "GET " },
      "destination": { "type": "forward", "forward": {
          "strategy": "serial",
          "targets": [ { "ip": "127.0.0.1", "port": 8080 }, { "ip": "127.0.0.1", "port": 8081 } ],
          "fallbacks": [ { "ip": "127.0.0.1", "port": 8082 }, { "ip": "127.0.0.1", "port": 8083 } ]
      } }
    }
  ]
}
"""

TOML_TCP = """
[[protocols]]
name = "tls"
priority = 1
detect = { method = "magic", pattern = "0x16" }
session = { keepalive = true, timeout = 30000 }
destination = { type = "resolver", resolver = "tls" }

[[protocols]]
name = "http"
priority = 2
detect = { method = "prefix", pattern = "GET " }
destination = { type = "forward", forward = { strategy = "serial", targets = [
    { ip = "127.0.0.1", port = 8080 },
    { ip = "127.0.0.1", port = 8081 },
], fallbacks = [
    { ip = "127.0.0.1", port = 8082 },
    { ip = "127.0.0.1", port = 8083 },
] } }
"""

YAML_TCP = """
protocols:
  - name: tls
    priority: 1
    detect:
      method: magic
      pattern: "0x16"
    session:
      keepalive: true
      timeout: 30000
    destination:
      type: resolver
      resolver: tls
  - name: http
    priority: 2
    detect:
      method: prefix
      pattern: "GET "
    destination:
      type: forward
      forward:
        strategy: serial
        targets:
          - ip: 127.0.0.1
            port: 8080
          - ip: 127.0.0.1
            port: 8081
        fallbacks:
          - ip: 127.0.0.1
            port: 8082
          - ip: 127.0.0.1
            port: 8083
"""

# --- UDP Configuration Content ---
JSON_UDP = r"""
{
  "protocols": [
    {
      "name": "quic", "priority": 1,
      "detect": { "method": "magic", "pattern": "0xc3" },
      "destination": { "type": "resolver", "resolver": "quic" }
    },
    {
      "name": "dns", "priority": 2,
      "detect": { "method": "prefix", "pattern": "\u0000\u0001" },
      "destination": { "type": "forward", "forward": {
          "strategy": "random",
          "targets": [ { "ip": "127.0.0.1", "port": 5353 }, { "ip": "127.0.0.1", "port": 5354 } ],
          "fallbacks": [ { "ip": "127.0.0.1", "port": 5355 }, { "ip": "127.0.0.1", "port": 5356 } ]
      } }
    }
  ]
}
"""

TOML_UDP = r"""
[[protocols]]
name = "quic"
priority = 1
detect = { method = "magic", pattern = "0xc3" }
destination = { type = "resolver", resolver = "quic" }

[[protocols]]
name = "dns"
priority = 2
detect = { method = "prefix", pattern = "\u0000\u0001" }
destination = { type = "forward", forward = { strategy = "random", targets = [
    { ip = "127.0.0.1", port = 5353 },
    { ip = "127.0.0.1", port = 5354 },
], fallbacks = [
    { ip = "127.0.0.1", port = 5355 },
    { ip = "127.0.0.1", port = 5356 },
] } }
"""

YAML_UDP = r"""
protocols:
  - name: quic
    priority: 1
    detect:
      method: magic
      pattern: "0xc3"
    destination:
      type: resolver
      resolver: quic
  - name: dns
    priority: 2
    detect:
      method: prefix
      pattern: "\0\x01"
    destination:
      type: forward
      forward:
        strategy: random
        targets:
          - ip: 127.0.0.1
            port: 5353
          - ip: 127.0.0.1
            port: 5354
        fallbacks:
          - ip: 127.0.0.1
            port: 5355
          - ip: 127.0.0.1
            port: 5356
"""


# --- Generic Test Runner ---
def run_config_test(
    files_to_create: Dict[str, str], debug_mode: bool
) -> Tuple[bool, str]:
    """
    A generic test executor for configuration file tests.
    """
    try:
        env_vars = {"LOG_LEVEL": "debug"}
        vane = VaneInstance(env_vars, "", debug_mode)

        listener_dir = vane.tmpdir / "listener" / "[80]"
        listener_dir.mkdir(parents=True, exist_ok=True)

        for filename, content in files_to_create.items():
            (listener_dir / filename).write_text(content)

        with vane:
            tcp_up = threading.Event()
            udp_up = threading.Event()

            def wait_for_both():
                while not (tcp_up.is_set() and udp_up.is_set()):
                    if any(PORT_80_TCP_UP in line for line in vane.captured_output):
                        tcp_up.set()
                    if any(PORT_80_UDP_UP in line for line in vane.captured_output):
                        udp_up.set()
                    if not (tcp_up.is_set() and udp_up.is_set()):
                        time.sleep(0.2)

            wait_thread = threading.Thread(target=wait_for_both)
            wait_thread.start()
            wait_thread.join(timeout=10)

            if not tcp_up.is_set() or not udp_up.is_set():
                log_dump = "".join(vane.captured_output)
                missing = []
                if not tcp_up.is_set():
                    missing.append(f"'{PORT_80_TCP_UP}'")
                if not udp_up.is_set():
                    missing.append(f"'{PORT_80_UDP_UP}'")
                reason = (
                    f"Timeout waiting for listeners. Missing: {', '.join(missing)}."
                )
                return (
                    False,
                    f"  └─ Details: {reason}\n\n--- Captured Log ---\n{log_dump}",
                )

    except Exception as e:
        return (False, f"  └─ Details: An unexpected exception occurred: {e}")

    return (True, "")
