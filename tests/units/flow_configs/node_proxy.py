# tests/units/flow_configs/node_proxy.py


def generate_config(node_name: str, backend_port: int) -> str:
    """
    Generates a YAML configuration for a TCP listener that proxies traffic
    to a specific named Node.
    """
    return f"""
connection:
  # Directly terminate the connection by proxying to the resolved Node.
  internal.transport.proxy.node:
    input:
      target.node: "{node_name}"
      target.port: {backend_port}
"""
