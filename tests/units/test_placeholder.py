# tests/units/test_placeholder.py

import time


def test_example_success():
    """A placeholder test that always passes."""
    time.sleep(0.1)  # Simulate some work
    assert 1 + 1 == 2  # Corrected from `+ 2`


def test_example_failure():
    """A placeholder test that always fails."""
    time.sleep(0.1)
    # Corrected to be a real comparison
    assert "hello" == "world", "Strings do not match"
