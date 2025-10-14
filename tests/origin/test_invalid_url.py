from tests.utils.test_helpers import (
    backup_origins_if_exists,
    restore_origins_if_backup,
    ensure_origins_absent,
    http_post,
)

def run():
    tmp = backup_origins_if_exists()
    try:
        ensure_origins_absent()
        r = http_post("/v1/origins", {"url": "http://"})
        assert r.status_code == 400, f"expected 400, got {r.status_code}"
    finally:
        restore_origins_if_backup(tmp)
