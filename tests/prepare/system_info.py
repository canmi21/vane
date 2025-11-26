# tests/prepare/system_info.py

import platform
import sys
import os
import subprocess


def _run_command(command):
    """Helper to run a command and return its stripped stdout."""
    try:
        result = subprocess.run(
            command, capture_output=True, text=True, check=True, encoding="utf-8"
        )
        return result.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "N/A"


def print_header(bin_name: str, package_version: str):
    """Prints the formatted header with system and toolchain info."""
    python_version = f"CPython {sys.version.split()[0]}"
    os_info = f"{platform.system()}-{platform.release()}-{platform.machine()}"

    cargo_version = _run_command(["cargo", "--version"])
    rustc_version = _run_command(["rustc", "--version"])
    target_bin_version = _run_command([bin_name, "-v"])

    print(f"+ {python_version} ({sys.executable})")
    print(f"+ {os_info}")
    print(f"+ CWD: {os.getcwd()}")
    print(f"+ CPU count: {os.cpu_count()}")

    print()  # Replaces the separator

    print(f"+ {cargo_version}")
    print(f"+ {rustc_version}")
    print(f"+ Target Pkg: {bin_name} v{package_version} (from Cargo.toml)")
    print(f"+ Installed Bin: {target_bin_version}")
