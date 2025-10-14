from tests.utils.test_helpers import (
    backup_origins_if_exists,
    restore_origins_if_backup,
    ensure_origins_absent,
    http_post,
    wait_for_file,
    read_origins,
)

def run():
    tmp = backup_origins_if_exists()
    try:
        ensure_origins_absent()
        urls = [
            "http://m1.example/one",
            "http://m2.example/two",
            "m3.example",            # no scheme
            "192.168.9.9:4000",      # ip
        ]
        for u in urls:
            r = http_post("/v1/origins", {"url": u})
            assert r.status_code == 201, f"create failed for {u}"
        assert wait_for_file()
        data = read_origins()
        for u in urls:
            found = any(v.get("raw_url") == u for v in data.values())
            assert found, f"{u} not found in file"
    finally:
        restore_origins_if_backup(tmp)
