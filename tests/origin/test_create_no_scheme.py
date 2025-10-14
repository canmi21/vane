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
        url = "example.com"
        r = http_post("/v1/origins", {"url": url})
        assert r.status_code == 201
        assert wait_for_file()
        data = read_origins()
        matches = [v for v in data.values() if v.get("raw_url") == url]
        assert matches, "no entry for example.com"
        origin = matches[0]
        # per Rust logic dummy -> non-IP => https:443
        assert origin["scheme"] == "https"
        assert int(origin["port"]) == 443
        assert origin["host"] == "example.com"
    finally:
        restore_origins_if_backup(tmp)
