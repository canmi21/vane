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
        r = http_post("/v1/origins", {"url": "http://u.example:6000/a"})
        assert r.status_code == 201
        assert wait_for_file()
        data = read_origins()
        _id = next(iter(data.keys()))
        # update path and port
        put = http_put(f"/v1/origins/{_id}", {"path": "/b", "port": 6010})
        assert put.status_code == 200
        newdata = read_origins()
        origin = newdata[_id]
        assert origin["path"] == "/b"
        assert int(origin["port"]) == 6010
    finally:
        restore_origins_if_backup(tmp)


if __name__ == "__main__":
    run()
