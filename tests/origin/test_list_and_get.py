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

        if r1.status_code != 201 or r2.status_code != 201:
            print("POST requests failed!")
            print("r1 status:", r1.status_code, "body:", r1.text)
            print("r2 status:", r2.status_code, "body:", r2.text)
            raise AssertionError("POST requests failed")

        if not wait_for_file():
            print("Wait for origins file failed!")
            raise AssertionError("Origins file not created")

        list_resp = http_get("/v1/origins")
        if list_resp.status_code != 200:
            print("GET /v1/origins failed! Status:", list_resp.status_code)
            print("Response body:", list_resp.text)
            raise AssertionError("List origins failed")

        json_data = list_resp.json()
        if not isinstance(json_data, dict) or "data" not in json_data:
            print("GET /v1/origins returned unexpected structure:", json_data)
            raise AssertionError("Invalid origins list structure")

        origins_list = json_data["data"]
        if not isinstance(origins_list, list):
            print("Expected list in 'data' field, got:", type(origins_list))
            print("Data:", origins_list)
            raise AssertionError("Invalid origins list content")

        file_data = read_origins()
        if not file_data:
            print("read_origins() returned empty data")
            raise AssertionError("Origins file empty")

        any_id = next(iter(file_data.keys()), None)
        if not any_id:
            print("No ID found in origins file:", file_data)
            raise AssertionError("No origin IDs found")

        get_resp = http_get(f"/v1/origins/{any_id}")
        if get_resp.status_code != 200:
            print(f"GET /v1/origins/{any_id} failed! Status:", get_resp.status_code)
            print("Response body:", get_resp.text)
            raise AssertionError("Get origin by ID failed")

    finally:
        restore_origins_if_backup(tmp)


if __name__ == "__main__":
    run()
