from tests.utils.test_helpers import (
    backup_origins_if_exists,
    restore_origins_if_backup,
    ensure_origins_absent,
    http_post,
    wait_for_file,
    read_origins,
    http_put,
)

def run():
    tmp = backup_origins_if_exists()
    try:
        ensure_origins_absent()
        r = http_post("/v1/origins", {"url": "https://skip.example/"})
        assert r.status_code == 201
        assert wait_for_file()
        data = read_origins()
        _id = next(iter(data.keys()))
        # flip skip_ssl_verify
        pr = http_put(f"/v1/origins/{_id}", {"skip_ssl_verify": True})
        assert pr.status_code == 200
        newdata = read_origins()
        assert newdata[_id]["skip_ssl_verify"] is True
    finally:
        restore_origins_if_backup(tmp)
