from tests.utils.test_helpers import (
    backup_origins_if_exists,
    restore_origins_if_backup,
    ensure_origins_absent,
    http_post,
    wait_for_file,
    http_get,
    read_origins,
)

def run():
    tmp = backup_origins_if_exists()
    try:
        ensure_origins_absent()
        r1 = http_post("/v1/origins", {"url": "http://list1.example:7000/"})
        r2 = http_post("/v1/origins", {"url": "http://list2.example:7001/"})
        assert r1.status_code == 201 and r2.status_code == 201
        assert wait_for_file()
        list_resp = http_get("/v1/origins")
        assert list_resp.status_code == 200
        json_data = list_resp.json()
        # should be a list of origins
        assert isinstance(json_data, list)
        # pick one id from file and GET it
        file_data = read_origins()
        any_id = next(iter(file_data.keys()))
        get_resp = http_get(f"/v1/origins/{any_id}")
        assert get_resp.status_code == 200
    finally:
        restore_origins_if_backup(tmp)
