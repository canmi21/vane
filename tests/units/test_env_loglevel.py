# tests/units/test_env_loglevel.py

import subprocess
import tempfile
import pathlib
import threading
import time
import sys
from typing import Tuple, List


def _reader_thread(
    proc_stdout,
    captured_output: List[str],
    event: threading.Event,
    expected_string: str,
    debug_mode: bool,
):
    """
    Reads a process's output, optionally printing it and setting an event
    when a specific string is found.
    """
    try:
        for line in iter(proc_stdout.readline, ""):
            if debug_mode:
                sys.stdout.write(line)
            captured_output.append(line)
            if not event.is_set() and expected_string in line:
                event.set()
        proc_stdout.close()
    except Exception:
        pass


def _run_vane_and_assert_output(
    log_level: str,
    timeout: float,
    expected_string: str,
    should_find: bool,
    debug_mode: bool,
) -> Tuple[bool, str]:
    """
    A helper to run Vane in a specific configuration and assert on its output.
    """
    process = None
    reader = None
    captured_output: List[str] = []

    test_passed = False
    failure_reason = ""

    with tempfile.TemporaryDirectory() as tmpdir_str:
        try:
            tmpdir = pathlib.Path(tmpdir_str)

            env_content = f"""
LOG_LEVEL={log_level}
CONFIG_DIR={tmpdir}
SOCKET_DIR={tmpdir}
DETECT_PUBLIC_NETWORK=false
"""
            (tmpdir / ".env").write_text(env_content)

            process = subprocess.Popen(
                ["vane"],
                cwd=tmpdir,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )

            found_event = threading.Event()
            reader = threading.Thread(
                target=_reader_thread,
                args=(
                    process.stdout,
                    captured_output,
                    found_event,
                    expected_string,
                    debug_mode,
                ),
            )
            reader.start()

            if should_find:
                event_was_set = found_event.wait(timeout=timeout)
                if event_was_set:
                    test_passed = True
                else:
                    failure_reason = (
                        f"Timeout after {timeout}s waiting for '{expected_string}'."
                    )
            else:
                time.sleep(timeout)
                if not found_event.is_set():
                    test_passed = True
                else:
                    failure_reason = f"Unexpectedly found '{expected_string}'."

        except Exception as e:
            test_passed = False
            failure_reason = f"An unexpected exception occurred: {e}"

        finally:
            if process:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
            if reader and reader.is_alive():
                reader.join()

    if test_passed:
        return (True, "Assertion passed.")
    else:
        log_dump = "".join(captured_output)
        return (
            False,
            f"  └─ Details: {failure_reason}\n\n--- Captured Log ---\n{log_dump}",
        )


def run(debug_mode: bool) -> Tuple[bool, str]:
    """
    Runs a multi-stage test for LOG_LEVEL behavior.
    """
    target_string = "Anynet completed before timeout."

    success, details = _run_vane_and_assert_output(
        log_level="debug",
        timeout=10,
        expected_string=target_string,
        should_find=True,
        debug_mode=debug_mode,
    )
    if not success:
        return (False, details)

    success, details = _run_vane_and_assert_output(
        log_level="info",
        timeout=0.5,
        expected_string=target_string,
        should_find=False,
        debug_mode=debug_mode,
    )
    if not success:
        return (False, details)

    return (True, "")
