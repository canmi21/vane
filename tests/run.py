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
)

try:
    # --- Wait for "Running ..." signal safely ---
    if proc.stdout is None:
        raise RuntimeError("Failed to capture stdout from cargo process.")

    for line in iter(proc.stdout.readline, ''):
        print(line, end="")
        if "Running" in line:
            time.sleep(1)
            break

    # --- Run tests ---
    tests_to_run = [
        Path(__file__).parent / "origin" / "test.py",
        # add more test files here if needed
    ]

    for test_file in tests_to_run:
        print(f"> Running test: {test_file.name}")
        test_proc = subprocess.run(
            ["python3", str(test_file)],
            capture_output=True,
            text=True
        )
        print(test_proc.stdout)
        if test_proc.returncode != 0:
            print(f"! Test {test_file.name} failed!")
        else:
            print(f"+ Test {test_file.name} passed!")

finally:
    # --- Terminate engine ---
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()

    # --- Restore original config ---
    if BACKUP_DIR.exists():
        if CONFIG_DIR.exists():
            shutil.rmtree(CONFIG_DIR)
        BACKUP_DIR.rename(CONFIG_DIR)
