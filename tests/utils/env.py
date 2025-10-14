from dotenv import load_dotenv
from pathlib import Path
import os

env_path = Path(__file__).resolve().parents[2] / ".env"
load_dotenv(dotenv_path=env_path)

def get_env(key: str):
    v = os.getenv(key)
    if v is None:
        return False
    return v