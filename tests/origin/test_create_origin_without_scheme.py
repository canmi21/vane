# test_create_origin_without_scheme.py

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
        url = "example.com"
        resp = http_post("/v1/origins", {"url": url})
        if resp.status_code != 201:
            print("POST failed! Status code:", resp.status_code)
            print("Response body:", resp.text)
            raise AssertionError("POST request failed")
        created_ids.append(resp.json().get("id"))

        if not wait_for_file():
            print("Wait for origins file failed!")
            raise AssertionError("Origins file not created")

        data = read_origins()
        matches = [v for v in data.values() if v.get("raw_url") == url]
        if not matches:
            print("No matches found for raw_url:", url)
            raise AssertionError("No origin found")
    finally:
        for _id in created_ids:
            try:
                http_delete(f"/v1/origins/{_id}")
            except Exception:
                pass
        restore_origins_if_backup(tmp)


if __name__ == "__main__":
    run()
