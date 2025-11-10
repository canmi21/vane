# tests/main.py

import sys
import pathlib
from prepare import check_pkg_version, check_pkg_installation, system_info
import runner


def main():
    """Main entry point for the test runner."""
    project_root = pathlib.Path(__file__).parent.parent.resolve()

    # Extract --debug flag and other arguments separately
    args = sys.argv[1:]
    debug_mode = "--debug" in args
    if debug_mode:
        args.remove("--debug")

    try:
        bin_name, version = check_pkg_version.get_package_info(project_root)
        system_info.print_header(bin_name, version)

        print()
        print("+ PREPARATION PHASE +")
        check_pkg_installation.check_and_install(bin_name, version, project_root)

        print()
        print("+ TEST EXECUTION PHASE +")
        # Pass the remaining arguments to the test suite runner
        runner.run_suite(debug_mode, args)

    except (FileNotFoundError, KeyError, ValueError, RuntimeError) as e:
        print(f"\n+ FATAL ERROR: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
