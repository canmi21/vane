# test_delete_origin.py

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
    created_ids = []
    try:
        ensure_origins_absent()
        resp = http_post("/v1/origins", {"url": "http://d.example:5000/"})
        if resp.status_code != 201:
            print("POST failed! Status code:", resp.status_code)
            print("Response body:", resp.text)
            raise AssertionError("POST request failed")
        _id = resp.json().get("id")
        created_ids.append(_id)

        if not wait_for_file():
            print("Wait for origins file failed!")
            raise AssertionError("Origins file not created")

        del_resp = http_delete(f"/v1/origins/{_id}")
        if del_resp.status_code not in (200, 204):
            print("DELETE failed! Status:", del_resp.status_code)
            print("Response body:", del_resp.text)
            raise AssertionError("Delete origin failed")
        created_ids.remove(_id)

        newdata = read_origins()
        if _id in newdata:
            print("Deleted origin still present in file:", _id)
            raise AssertionError("Delete did not remove origin")
    finally:
        for _id in created_ids:
            try:
                http_delete(f"/v1/origins/{_id}")
            except Exception:
                pass
        restore_origins_if_backup(tmp)

if __name__ == "__main__":
    run()
