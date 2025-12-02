# tests/units/flow_configs/http_detect_and_route.py


def generate_config(backend_port: int) -> str:
    """
    Generates a YAML configuration for a TCP listener using the new 'connection'
    (Flow Engine) format.

    This flow performs the following logic:
    1.  Uses the 'internal.protocol.detect' middleware to check if the incoming
        traffic matches the HTTP protocol.
    2.  If it matches (output branch "true"), the flow terminates by proxying
        the connection to the specified backend port.
    3.  If it does not match (output branch "false"), the flow terminates by
        immediately aborting the connection.
    """
    return f"""
connection:
  # Entry point of the flow: the protocol detection middleware.
  internal.protocol.detect:
    input:
      # The detection method is set to "http", which is a built-in alias for
      # the standard HTTP regex pattern.
      method: "http"
      # The 'payload' is templated to use the initial data peeked from the
      # connection, which Vane automatically places in the KvStore.
      payload: "{{{{req.peek_buffer_hex}}}}"
    output:
      # Defines the routing logic based on the middleware's result.
      "true":
        # If the protocol is HTTP, use the transparent proxy terminator.
        internal.transport.proxy.transparent:
          input:
            # The target IP and port for the proxy.
            target.ip: "127.0.0.1"
            target.port: {backend_port}
      "false":
        # If the protocol is not HTTP, use the abort terminator.
        internal.transport.abort:
          input: {{}}
"""
