# tests/http/test.py

import importlib
import traceback
import sys
from pathlib import Path

# --- Module Import Setup ---
# FIX: The ROOT must be the project's root directory (vane/), not the tests/ directory.
# This is two levels up from this file (tests/http/test.py -> tests/ -> vane/).
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

# --- List of all HTTP tests to run ---
tests = [
    "tests.http.h1.test_no_host_header",
    # Add future http tests here
]

results = []
print(f"Running {len(tests)} HTTP tests")

for name in tests:
    short = name.split(".")[-1]
    print(f"{short} ... ", end="", flush=True)

    try:
        module = importlib.import_module(name)
        if hasattr(module, "run"):
            module.run()
        else:
            raise RuntimeError(f"Test module '{name}' has no run() function.")

        print("ok")
        results.append((name, True, None))

    except Exception:
        print("FAILED")
        tb = traceback.format_exc()
        results.append((name, False, tb))
        print(tb)

passed = sum(1 for r in results if r[1])
failed = len(results) - passed

print(
    f"HTTP test result: {'ok' if failed == 0 else 'FAILED'}. {passed} passed; {failed} failed"
)

if failed:
    sys.exit(1)
