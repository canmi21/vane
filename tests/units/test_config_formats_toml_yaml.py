# tests/units/test_config_formats_1.py

from typing import Tuple
from .config_utils import (
    run_config_test,
    TOML_TCP,
    YAML_UDP,
)


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests loading TCP from TOML and UDP from YAML.
    """
    files_to_create = {
        "tcp.toml": TOML_TCP,
        "udp.yaml": YAML_UDP,
    }
    return run_config_test(files_to_create, debug_mode)
