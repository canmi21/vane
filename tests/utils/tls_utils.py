# tests/utils/tls_utils.py

import subprocess
import pathlib


def generate_self_signed_cert(cert_path: pathlib.Path, key_path: pathlib.Path):
    """
    Generates a self-signed SSL certificate for testing purposes using openssl.
    This creates a temporary certificate and private key that can be used by
    a test server, avoiding the need to store pre-generated certs.

    Args:
        cert_path: The path where the certificate file will be saved.
        key_path: The path where the private key file will be saved.
    """
    command = [
        "openssl",
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        str(key_path),
        "-out",
        str(cert_path),
        "-subj",
        "/CN=localhost",
    ]
    try:
        subprocess.run(
            command,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except (subprocess.CalledProcessError, FileNotFoundError) as e:
        raise RuntimeError(
            f"Failed to generate self-signed certificate using openssl: {e}"
        ) from e
