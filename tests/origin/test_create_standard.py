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
        url = "http://example.com:8080/path1"
        r = http_post("/v1/origins", {"url": url})
        assert r.status_code == 201
        assert wait_for_file()
        data = read_origins()
        # validate parsed result
        matches = [v for v in data.values() if v.get("raw_url") == url]
        assert len(matches) == 1
        origin = matches[0]
        assert origin["scheme"] == "http"
        assert origin["host"] == "example.com"
        assert int(origin["port"]) == 8080
        assert origin["path"] == "/path1"
    finally:
        restore_origins_if_backup(tmp)
