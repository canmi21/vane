# tests/utils/http_utils.py

from __future__ import annotations
import threading
import requests
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import List, Dict, Any, Tuple, cast


class RequestRecorderHandler(BaseHTTPRequestHandler):
    """A custom HTTP handler that records requests to a list."""

    @property
    def _stoppable_server(self) -> StoppableHTTPServer:
        """
        A type-safe property to access the server instance with its custom attributes.
        This uses `cast` to inform the type checker of the actual server type
        without violating the inheritance contract of the `server` attribute itself.
        """
        return cast(StoppableHTTPServer, self.server)

    def _record_request(self):
        self._stoppable_server.received_requests.append(
            {
                "method": self.command,
                "path": self.path,
            }
        )
        self.send_response(200)
        self.send_header("Content-type", "text/plain")
        self.end_headers()
        self.wfile.write(b"OK")

    def do_GET(self):
        self._record_request()

    def do_POST(self):
        self._record_request()

    def do_PUT(self):
        self._record_request()

    def do_DELETE(self):
        self._record_request()

    def do_OPTIONS(self):
        self._record_request()

    def log_message(self, format: str, *args: Any) -> None:
        return


class StoppableHTTPServer(HTTPServer):
    """An HTTPServer that can be stopped from another thread."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.received_requests: List[Dict[str, str]] = []
        self._thread = None

    def start(self):
        """Starts the server in a background thread."""
        self._thread = threading.Thread(target=self.serve_forever)
        self._thread.daemon = True
        self._thread.start()

    def stop(self):
        """Stops the server and waits for the thread to terminate."""
        if self._thread:
            self.shutdown()
            self.server_close()
            self._thread.join()


# --- Latency Testing Classes ---
class DelayedRequestRecorderHandler(RequestRecorderHandler):
    """

    A specialized handler that inherits from RequestRecorderHandler and adds
    a delay at the beginning of the connection handling process.
    """

    def handle(self) -> None:
        """
        Overrides the base handle method to inject a delay, reading the
        duration from a `delay_sec` attribute on its server instance.
        """
        delay = getattr(self.server, "delay_sec", 0.0)
        time.sleep(delay)
        super().handle()


class SlowStoppableHTTPServer(StoppableHTTPServer):
    """
    A specialized server that stores a `delay_sec` value for the
    DelayedRequestRecorderHandler to use.
    """

    def __init__(
        self,
        server_address: Tuple[str, int],
        RequestHandlerClass: Any,
        delay_sec: float,
    ):
        self.delay_sec = delay_sec
        super().__init__(server_address, RequestHandlerClass)


def send_test_requests(
    port: int,
    methods: List[str],
    paths: List[str],
    timeout: float = 2.0,
) -> Tuple[bool, List[Dict[str, str]]]:
    """
    Sends a series of HTTP requests with a configurable timeout and returns a
    list of what was sent. It tolerates individual request failures.
    """
    sent_requests = []
    base_url = f"http://127.0.0.1:{port}"

    # The try-except block is now correctly placed inside the loop. This ensures
    # that a single failed request (like a timeout in the 'fastest' strategy
    # test) does not prematurely terminate the entire send sequence.
    for path in paths:
        for method in methods:
            try:
                requests.request(method, f"{base_url}{path}", timeout=timeout)
                sent_requests.append({"method": method, "path": path})
            except requests.exceptions.RequestException:
                # In tests like 'fastest', some requests are expected to fail.
                # We ignore these failures and continue with the next request.
                pass

    # The function now always returns True, as its purpose is to attempt all
    # sends, not to guarantee the success of all of them. The success is
    # verified by the backend counters in the actual test case.
    return (True, sent_requests)
