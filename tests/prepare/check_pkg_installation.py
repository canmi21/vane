# tests/prepare/check_pkg_installation.py

import shutil
import subprocess
from pathlib import Path
from . import build_and_install_target_bin


def check_and_install(bin_name: str, expected_version: str, project_root: Path):
    """
    Checks if the correct binary version is installed and builds if not.
    """
    print(f"+ Verifying installation of '{bin_name}' v{expected_version}...")

    if not shutil.which(bin_name):
        print(f"+ INFO: Binary '{bin_name}' not found in PATH.")
        build_and_install_target_bin.run(project_root)
        return

    try:
        result = subprocess.run(
            [bin_name, "-v"], capture_output=True, text=True, check=True
        )
        output = result.stdout.strip()
        parts = output.split()

        if len(parts) >= 2 and parts[0] == bin_name and parts[1] == expected_version:
            print(f"+ OK: Correct version '{expected_version}' is already installed.")
        else:
            print(
                f"+ INFO: Version mismatch. Found '{output}', expected '{expected_version}'."
            )
            build_and_install_target_bin.run(project_root)

    except (subprocess.CalledProcessError, FileNotFoundError, IndexError) as e:
        print(f"+ WARN: Could not verify version of existing '{bin_name}': {e}")
        build_and_install_target_bin.run(project_root)
