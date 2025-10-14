# test_http_origin_with_path.py

from tests.utils.test_helpers import (
    backup_origins_if_exists,
    restore_origins_if_backup,
    ensure_origins_absent,
    http_post,
    wait_for_file,
    read_origins,
    http_delete,
)
import json

def run():
    tmp = backup_origins_if_exists()
    created_ids = []
    try:
        ensure_origins_absent()
        resp = http_post("/v1/origins", {"url": "http://example.com:8080/abc"})
        assert resp.status_code == 201, f"expected 201 got {resp.status_code} {resp.text}"
        _id = resp.json().get("id")
        created_ids.append(_id)

        ok = wait_for_file(timeout_sec=5)
        assert ok, "origins.json not created"
        data = read_origins()
        assert isinstance(data, dict)
        found = any(v.get("raw_url") == "http://example.com:8080/abc" for v in data.values())
        assert found, "created origin not present in file"
    finally:
        for _id in created_ids:
            try:
                http_delete(f"/v1/origins/{_id}")
            except Exception:
                pass
        restore_origins_if_backup(tmp)

if __name__ == "__main__":
    run()
