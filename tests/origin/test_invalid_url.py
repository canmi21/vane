# test_invalid_url.py

from tests.utils.test_helpers import (
    backup_origins_if_exists,
    restore_origins_if_backup,
    ensure_origins_absent,
    http_post,
)


def run():
    tmp = backup_origins_if_exists()
    try:
        ensure_origins_absent()
        resp = http_post("/v1/origins", {"url": "http://"})
        if resp.status_code != 400:
            print("Expected 400, got:", resp.status_code)
            print("Response body:", resp.text)
            raise AssertionError("Invalid URL accepted")
    finally:
        restore_origins_if_backup(tmp)


if __name__ == "__main__":
    run()
