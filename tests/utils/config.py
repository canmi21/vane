from tests.utils.env import get_env

CONFIG_DIR = get_env("CONFIG_DIR") or "~/vane"
PORT = get_env("PORT") or "3333"
SKIP_AUTH = get_env("SKIP_AUTH") or "false"
