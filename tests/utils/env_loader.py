# tests/utils/env_loader.py

import os
import pathlib
from typing import List, Optional


def load_env(project_root: pathlib.Path, allowed_keys: Optional[List[str]] = None):
    """
    Loads environment variables from the .env file located at the project root.

    This function parses the file line by line and injects variables into
    os.environ if they are not already set. This ensures that variables
    passed explicitly via the shell take precedence over the .env file.

    To prevent environment pollution, strictly pass a list of strings to
    `allowed_keys`. Only variables matching these keys will be loaded.

    Args:
        project_root: The pathlib.Path object pointing to the root of the repository.
        allowed_keys: A list of variable names (e.g. ["DEV_PROJECT_DIR"]) to allow.
                      If None, all variables in .env are loaded (NOT recommended).
    """
    env_path = project_root / ".env"

    if not env_path.exists():
        return

    try:
        with open(env_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                # Skip comments and empty lines
                if not line or line.startswith("#"):
                    continue

                # Parse key=value
                if "=" in line:
                    key, value = line.split("=", 1)
                    key = key.strip()
                    value = value.strip()

                    # Whitelist Check: If allowed_keys is provided, skip keys not in it.
                    if allowed_keys is not None and key not in allowed_keys:
                        continue

                    # Remove surrounding quotes if present
                    if (value.startswith('"') and value.endswith('"')) or (
                        value.startswith("'") and value.endswith("'")
                    ):
                        value = value[1:-1]

                    # Only set if not already present in the environment
                    if key not in os.environ:
                        os.environ[key] = value
    except Exception as e:
        print(f"Warning: Failed to parse .env file: {e}")
