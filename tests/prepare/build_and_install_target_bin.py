# tests/prepare/build_and_install_target_bin.py

import subprocess
import sys
import pathlib
import threading


def _stream_output(pipe, file):
    """Reads from a pipe and prints to the specified file."""
    for line in iter(pipe.readline, b""):
        file.write(line.decode("utf-8"))
    pipe.close()


def run(project_root: pathlib.Path):
    """
    Navigates to the project root and runs 'cargo install'.
    """
    command = ["cargo", "install", "--color=always", "--path", "."]

    print(f"+ Build Required. Executing: {' '.join(command)}")

    process = subprocess.Popen(
        command, cwd=project_root, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )

    stdout_thread = threading.Thread(
        target=_stream_output, args=(process.stdout, sys.stdout)
    )
    stderr_thread = threading.Thread(
        target=_stream_output, args=(process.stderr, sys.stderr)
    )

    stdout_thread.start()
    stderr_thread.start()
    stdout_thread.join()
    stderr_thread.join()
    process.wait()

    if process.returncode != 0:
        raise RuntimeError(f"Build process failed with exit code {process.returncode}")

    print("+ Build and installation completed successfully.")
