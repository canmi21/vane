import importlib
import traceback
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

tests = [
    "tests.origin.test_file_generation",
    "tests.origin.test_create_origin_without_scheme",
    "tests.origin.test_delete_origin",
    "tests.origin.test_dummy_https_origin",
    "tests.origin.test_http_origin_with_path",
    "tests.origin.test_invalid_url",
    "tests.origin.test_ip_origin",
    "tests.origin.test_list_and_get",
    "tests.origin.test_multiple_writes",
    "tests.origin.test_skip_ssl_flag",
    "tests.origin.test_update_origin",
]

results = []
print(f"running {len(tests)} tests")
for name in tests:
    short = name.split(".")[-1]
    print(f"{short} ... ", end="", flush=True)
    try:
        m = importlib.import_module(name)
        if hasattr(m, "run"):
            m.run()
        else:
            raise RuntimeError("no run()")
        print("ok")
        results.append((name, True, None))
    except Exception:
        print("FAILED")
        tb = traceback.format_exc()
        results.append((name, False, tb))
        print(tb)

passed = sum(1 for r in results if r[1])
failed = len(results) - passed
print()
print(f"test result: {'ok' if failed==0 else 'FAILED'}. {passed} passed; {failed} failed")
if failed:
    sys.exit(1)
