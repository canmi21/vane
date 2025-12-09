#!/usr/bin/env python3

import http.server
import socketserver
import sys
import json

# Read port from args
if len(sys.argv) < 2:
    print("Usage: mock_server.py <port>", file=sys.stderr)
    sys.exit(1)

PORT = int(sys.argv[1])


class PluginHandler(http.server.BaseHTTPRequestHandler):
    def do_OPTIONS(self):
        """Handle OPTIONS request for connectivity validation."""
        self.send_response(200)
        self.end_headers()

    def do_POST(self):
        content_length = int(self.headers.get("Content-Length", 0))
        post_data = self.rfile.read(content_length)

        try:
            inputs = json.loads(post_data.decode("utf-8"))
            auth_token = inputs.get("auth_token")

            response = {}
            if auth_token == "secret123":
                # Matches Vane's ExternalApiResponse contract
                response = {
                    "status": "success",
                    "data": {"branch": "success", "store": {"user_role": "admin"}},
                }
            else:
                response = {
                    "status": "success",
                    "data": {"branch": "failure", "store": {"error": "bad_token"}},
                }

            resp_bytes = json.dumps(response).encode("utf-8")

            self.send_response(200)
            self.send_header("Content-type", "application/json")
            self.send_header("Content-Length", str(len(resp_bytes)))
            self.end_headers()
            self.wfile.write(resp_bytes)

        except Exception as e:
            self.send_response(500)
            self.end_headers()
            print(f"Error: {e}", file=sys.stderr)

    def log_message(self, format, *args):
        # Silence default logs
        pass


with socketserver.TCPServer(("127.0.0.1", PORT), PluginHandler) as httpd:
    print(f"Listening on {PORT}")
    sys.stdout.flush()
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
