from tests.utils.test_helpers import (
    backup_origins_if_exists,
    restore_origins_if_backup,
    ensure_origins_absent,
    http_post,
    wait_for_file,
    read_origins,
    http_delete,
)

def run():
    tmp = backup_origins_if_exists()
    try:
        ensure_origins_absent()
        r = http_post("/v1/origins", {"url": "http://d.example:5000/"})
        assert r.status_code == 201
        assert wait_for_file()
        data = read_origins()
        _id = next(iter(data.keys()))
        delr = http_delete(f"/v1/origins/{_id}")
        assert delr.status_code in (204, 200)
        # file should exist but id removed
        newdata = read_origins()
        assert _id not in newdata
    finally:
        restore_origins_if_backup(tmp)
