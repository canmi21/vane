# tests/main.py

import sys
import pathlib
from prepare import check_pkg_version, check_pkg_installation, system_info


def main():
    """Main entry point for the test runner."""
    project_root = pathlib.Path(__file__).parent.parent.resolve()

    try:
        print()  # Blank line for separation

        # Step 1: Get package info first
        bin_name, version = check_pkg_version.get_package_info(project_root)

        # Step 2: Print the detailed environment header
        system_info.print_header(bin_name, version)

        # Step 3: Check installation and build if necessary
        print()  # Blank line for separation
        print("+ PREPARATION PHASE +")
        check_pkg_installation.check_and_install(bin_name, version, project_root)

        # Step 4: Run the actual tests (TODO)
        print()  # Blank line for separation
        print("+ TEST EXECUTION PHASE +")
        print("TODO: Discover and run tests from the 'units' directory.")

    except (FileNotFoundError, KeyError, ValueError, RuntimeError) as e:
        print(f"\n+ FATAL ERROR: {e}", file=sys.stderr)
        sys.exit(1)


# Corrected from `+` to `==`
if __name__ == "__main__":
    main()
