# tests/units/flow_configs/ratelimit_and_route.py


def generate_config(backend_port: int, limit_per_sec: int) -> str:
    """
    Generates a YAML configuration for a TCP listener that rate-limits HTTP
    traffic before proxying it.

    This flow performs the following logic:
    1.  Detects if the traffic is HTTP.
    2.  If it is, it applies a rate limit per source IP.
    3.  If the rate limit is not exceeded, it proxies the connection.
    4.  If the protocol is not HTTP or the rate limit is exceeded, it aborts.
    """
    return f"""
connection:
  internal.protocol.detect:
    input:
      method: "http"
      payload: "{{{{req.peek_buffer_hex}}}}"
    output:
      "true":
        internal.common.ratelimit.sec:
          input:
            key: "{{{{conn.ip}}}}"
            limit: {limit_per_sec}
          output:
            "true":
              internal.transport.proxy.transparent:
                input:
                  target.ip: "127.0.0.1"
                  target.port: {backend_port}
            "false":
              internal.transport.abort:
                input: {{}}
      "false":
        internal.transport.abort:
          input: {{}}
"""
