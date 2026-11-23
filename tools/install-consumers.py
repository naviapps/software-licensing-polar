#!/usr/bin/env python3
"""Run the provider's independent Git Rust and installed macOS C fixtures."""

import tomllib
from pathlib import Path

from _tooling import load

ROOT = Path(__file__).resolve().parent.parent


def main(argv=None):
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    core = workspace["dependencies"]["software-licensing"]
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    (tokio,) = (entry for entry in lock["package"] if entry["name"] == "tokio")
    load("installed_consumers").check_installed(
        root=ROOT,
        git_url="https://github.com/naviapps/software-licensing-polar.git",
        rust_packages=("software-licensing-polar",),
        rust_git_dependencies={"software-licensing": core},
        rust_registry_dependencies={
            "tokio": {"version": "=" + tokio["version"], "features": ["macros", "rt"]}
        },
        framework=(
            "Artifacts/CSoftwareLicensingPolar.xcframework/macos-arm64_x86_64/"
            "CSoftwareLicensingPolar.framework"
        ),
        header="software_licensing_polar.h",
        argv=argv,
    )


if __name__ == "__main__":
    main()
