# tests/units/test_config_formats_2.py

from typing import Tuple
from .config_utils import (
    run_config_test,
    JSON_TCP,
    TOML_UDP,
)


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Tests loading TCP from JSON and UDP from TOML.
    """
    files_to_create = {
        "tcp.json": JSON_TCP,
        "udp.toml": TOML_UDP,
    }
    return run_config_test(files_to_create, debug_mode)
