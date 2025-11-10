# tests/units/test_config_formats_3.py

from typing import Tuple
from .config_utils import (
    run_config_test,
    YAML_TCP,
    JSON_UDP,
)


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests loading TCP from YAML and UDP from JSON.
    """
    files_to_create = {
        "tcp.yaml": YAML_TCP,
        "udp.json": JSON_UDP,
    }
    return run_config_test(files_to_create, debug_mode)
