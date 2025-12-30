# tests/utils/template.py

import subprocess
import tempfile
import pathlib
import threading
import sys
import secrets
from typing import List, Dict


def _reader_thread(
    proc_stdout,
    captured_output: List[str],
    event: threading.Event,
    expected_string: str,
    debug_mode: bool,
):
    """
    Internal helper to read a process's output.
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


class VaneInstance:
    """
    A context manager to handle the lifecycle of a Vane subprocess for testing.
    """

    def __init__(
        self,
        env_vars: Dict[str, str],
        string_to_find: str,
        debug_mode: bool,
    ):
        self.env_vars = env_vars
        self.string_to_find = string_to_find
        self.debug_mode = debug_mode
        self.access_token = secrets.token_hex(16)  # Generate 32-char hex token

        self.process = None
        self.reader_thread = None
        self.captured_output: List[str] = []
        self.found_event = threading.Event()
        self._temp_dir_manager = tempfile.TemporaryDirectory()
        self.tmpdir = pathlib.Path(self._temp_dir_manager.name)

    def __enter__(self):
        """Sets up the environment and starts the Vane process."""
        # Start with sane defaults for isolated testing
        final_env_vars = {
            "CONFIG_DIR": str(self.tmpdir),
            "SOCKET_DIR": str(self.tmpdir),
            "DETECT_PUBLIC_NETWORK": "false",
            "ACCESS_TOKEN": self.access_token,  # Default token
        }
        # Allow user-provided env_vars to override the defaults
        final_env_vars.update(self.env_vars)

        env_content = "\n".join(
            f"{key.upper()}={value}" for key, value in final_env_vars.items()
        )
        (self.tmpdir / ".env").write_text(env_content + "\n")

        self.process = subprocess.Popen(
            ["vane"],
            cwd=self.tmpdir,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )

        self.reader_thread = threading.Thread(
            target=_reader_thread,
            args=(
                self.process.stdout,
                self.captured_output,
                self.found_event,
                self.string_to_find,
                self.debug_mode,
            ),
        )
        self.reader_thread.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Cleans up the process and temporary directory."""
        if self.process:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()

        if self.reader_thread and self.reader_thread.is_alive():
            self.reader_thread.join()

        self._temp_dir_manager.cleanup()
