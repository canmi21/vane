#!/usr/bin/env python3

import os
import sys
import shutil
import subprocess
import time
from pathlib import Path
from dotenv import load_dotenv

# --- Ensure tests/ is in sys.path so utils can be imported ---
sys.path.insert(0, str(Path(__file__).parent))

# --- Load .env ---
env_path = Path(__file__).resolve().parents[1] / ".env"
load_dotenv(dotenv_path=env_path)


def get_env(key: str):
    v = os.getenv(key)
    if v is None:
        return False
    return v


# --- Read config ---
from utils import config  # now works

CONFIG_DIR = get_env("CONFIG_DIR") or config.CONFIG_DIR
CONFIG_DIR = Path(CONFIG_DIR).expanduser()
BACKUP_DIR = CONFIG_DIR.with_suffix(".d")

# --- Backup and clear config ---
if CONFIG_DIR.exists():
    if BACKUP_DIR.exists():
        shutil.rmtree(BACKUP_DIR)
    CONFIG_DIR.rename(BACKUP_DIR)

CONFIG_DIR.mkdir(parents=True, exist_ok=True)

# --- Start engine subprocess ---
engine_dir = Path(__file__).resolve().parents[1] / "engine"
proc = subprocess.Popen(
    ["cargo", "run"],
    cwd=engine_dir,
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    text=True,
    bufsize=1,  # Line-buffered
    universal_newlines=True,  # Ensure text mode works correctly
)

all_tests_passed = True

try:
    # --- Wait for "Management console listening" signal ---
    if proc.stdout is None:
        raise RuntimeError("Failed to capture stdout from cargo process.")

    for line in iter(proc.stdout.readline, ""):
        print(line, end="")
        # A more robust signal that the server is ready
        if "Management console listening" in line:
            time.sleep(1)  # Give it a moment to stabilize
            break
    print("\n>>> Vane Engine Ready <<<")

    # --- Define Test Suites to Run ---
    tests_to_run = [
        Path(__file__).parent / "origin" / "test.py",
        Path(__file__).parent / "http" / "test.py",  # <-- ADD THIS LINE
    ]

    for test_file in tests_to_run:
        print(
            f"\n>>> Running Test Suite: {test_file.relative_to(Path(__file__).parent)} <<<"
        )
        # Use python3 -u for unbuffered output to see results in real-time
        test_proc = subprocess.run(
            ["python3", "-u", str(test_file)], capture_output=True, text=True
        )

        # Print the captured output from the test suite runner
        print(test_proc.stdout)
        if test_proc.stderr:
            print("--- Stderr ---")
            print(test_proc.stderr)

        if test_proc.returncode != 0:
            print(f"!!! Test Suite {test_file.name} FAILED! !!!")
            all_tests_passed = False
        else:
            print(f"+++ Test Suite {test_file.name} passed! +++")

finally:
    # --- Terminate engine ---
    print("\n>>> Clean up the test environment <<<")
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        print("!!! Engine did not terminate gracefully, killing. !!!")
        proc.kill()

    # --- Restore original config ---
    if BACKUP_DIR.exists():
        if CONFIG_DIR.exists():
            shutil.rmtree(CONFIG_DIR)
        BACKUP_DIR.rename(CONFIG_DIR)

    print("\n--- Test Run Finished ---")
    if not all_tests_passed:
        print("!!! One or more test suites failed. !!!")
        sys.exit(1)
    else:
        print("+++ All test suites passed successfully. +++")
