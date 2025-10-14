from pathlib import Path
import shutil
import time
import json
import os
import requests

from tests.utils.config import CONFIG_DIR, PORT

def config_dir_path() -> Path:
    p = Path(CONFIG_DIR).expanduser()
    return p

def origins_path() -> Path:
    return config_dir_path() / "origins.json"

def backup_origins_if_exists():
    p = origins_path()
    if p.exists():
        tmp = p.with_suffix(".json.tmp")
        if tmp.exists():
            tmp.unlink()
        shutil.move(str(p), str(tmp))
        return tmp
    return None

def restore_origins_if_backup(tmp_path):
    if tmp_path is None:
        return
    dest = origins_path()
    if dest.exists():
        dest.unlink()
    shutil.move(str(tmp_path), str(dest))

def ensure_origins_absent():
    p = origins_path()
    if p.exists():
        p.unlink()

def wait_for_file(timeout_sec=5):
    p = origins_path()
    start = time.time()
    while time.time() - start < timeout_sec:
        if p.exists():
            return True
        time.sleep(0.1)
    return False

def read_origins():
    p = origins_path()
    if not p.exists():
        return {}
    with p.open("r", encoding="utf-8") as f:
        return json.load(f)

def http_post(path: str, json_payload: dict):
    url = f"http://127.0.0.1:{PORT}{path}"
    return requests.post(url, json=json_payload)

def http_get(path: str):
    url = f"http://127.0.0.1:{PORT}{path}"
    return requests.get(url)

def http_put(path: str, json_payload: dict):
    url = f"http://127.0.0.1:{PORT}{path}"
    return requests.put(url, json=json_payload)

def http_delete(path: str):
    url = f"http://127.0.0.1:{PORT}{path}"
    return requests.delete(url)
