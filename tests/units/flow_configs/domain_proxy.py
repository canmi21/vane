# tests/units/flow_configs/domain_proxy.py


def generate_config(domain: str, backend_port: int) -> str:
    """
    Generates a YAML configuration for a TCP listener that proxies traffic
    to a specific Domain name.
    """
    return f"""
connection:
  # Directly terminate the connection by proxying to the resolved Domain.
  internal.transport.proxy.domain:
    input:
      target.domain: "{domain}"
      target.port: {backend_port}
"""
