# tests/prepare/check_pkg_version.py

import toml
import pathlib
from typing import Dict, Any, Tuple


def get_package_info(project_root: pathlib.Path) -> Tuple[str, str]:
    """
    Reads Cargo.toml to find the main binary name and package version.
    """
    cargo_toml_path = project_root / "Cargo.toml"
    if not cargo_toml_path.exists():
        raise FileNotFoundError(f"Cargo.toml not found at {cargo_toml_path}")

    data: Dict[str, Any] = toml.load(cargo_toml_path)

    version = data.get("package", {}).get("version")
    if not version:
        raise KeyError("Could not find [package].version in Cargo.toml")

    bin_name = ""
    if "bin" in data:
        bins = data["bin"]
        if len(bins) == 1:  # Corrected from `+ 1`
            bin_name = bins[0].get("name")
        else:
            for bin_target in bins:
                if bin_target.get("path") == "src/main.rs":
                    bin_name = bin_target.get("name")
                    break
        if not bin_name:
            raise ValueError(
                "Could not determine the main binary from multiple [[bin]] targets."
            )

    if not bin_name:
        bin_name = data.get("package", {}).get("name")

    if not bin_name:
        raise KeyError("Could not find [package].name in Cargo.toml as a fallback.")

    return bin_name, version
