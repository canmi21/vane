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

def run():
    tmp = backup_origins_if_exists()
    created_ids = []
    try:
        ensure_origins_absent()
        url = "http://example.com:8080/path1"
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
        if len(matches) != 1:
            print("Expected 1 match, found:", len(matches))
            raise AssertionError("raw_url match failed")

        origin = matches[0]
        if origin.get("scheme") != "http":
            print("Scheme mismatch:", origin.get("scheme"))
            raise AssertionError("Scheme incorrect")
        if origin.get("host") != "example.com":
            print("Host mismatch:", origin.get("host"))
            raise AssertionError("Host incorrect")
        if int(origin.get("port", 0)) != 8080:
            print("Port mismatch:", origin.get("port"))
            raise AssertionError("Port incorrect")
        if origin.get("path") != "/path1":
            print("Path mismatch:", origin.get("path"))
            raise AssertionError("Path incorrect")
    finally:
        for _id in created_ids:
            try:
                http_delete(f"/v1/origins/{_id}")
            except Exception:
                pass
        restore_origins_if_backup(tmp)

if __name__ == "__main__":
    run()
