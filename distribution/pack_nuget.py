#!/usr/bin/env python3
"""Build the Polar and pinned Software Licensing NuGet package pair."""

import argparse
import shutil
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

from sdk_tooling import artifacts, nuget
from sdk_tooling.c_headers import prepare_generated
from sdk_tooling.dotnet import subprocess_environment
from sdk_tooling.native_plan import ffi_product
from sdk_tooling.windows_delivery import RID
from sdk_tooling.workspaces import temporary_workspace
from sdk_tooling.xcframework import xcframework

from distribution import verify_bindings as bindings
from distribution.verify_windows import CORE_WINDOWS_BUILD, source_inputs, stage_core

RUNTIMES = ("osx-arm64", "osx-x64", RID)


def pack_pair(root, runtimes):
    env = subprocess_environment()
    env.pop(bindings.LEGACY_CORE_NATIVE_OVERRIDE, None)
    prepare_generated(root, env=env)
    provider = xcframework(root)
    managed = root / "bindings/dotnet"
    project = managed / "src" / bindings.VENDOR_PACKAGE / (bindings.VENDOR_PACKAGE + ".csproj")
    with temporary_workspace(None, prefix="polar-nuget-") as work:
        core = work / "software-licensing"
        stage_core(env, core, include_macos=True)
        core_version = ffi_product(core).version
        requirements = [
            node.get("Version")
            for node in ET.parse(project).findall("ItemGroup/PackageReference")
            if node.get("Include") == bindings.CORE_PACKAGE
        ]
        if requirements != [f"[{core_version}]"]:
            raise ValueError("Polar NuGet dependency must match the locked core version")
        options = {}
        if RID in runtimes:
            path = Path(".native-build/windows")
            (core / path).mkdir(parents=True)
            shutil.copytree(root / path / CORE_WINDOWS_BUILD / RID, core / path / RID)
            options["windows_source_inputs"] = lambda: source_inputs(provider, artifacts)

        # The base owns its package files and its producer's Tooling revision.
        env["NUGET_PACKAGES"] = str(work / "restore")
        bindings.run(
            "mise",
            "run",
            "sdk-tooling",
            "--",
            "python",
            "-m",
            "distribution.pack_nuget",
            *(arg for runtime in runtimes for arg in ("--runtime", runtime)),
            cwd=core,
            env=env | {"MISE_TRUSTED_CONFIG_PATHS": str(core)},
        )
        base = core / ".native-build/packages" / f"{bindings.CORE_PACKAGE}.{core_version}.nupkg"
        package = nuget.pack_native(
            provider,
            project=project,
            package_files={
                "LICENSE": root / "LICENSE",
                "README.md": managed / "README.md",
                **nuget.retained_notice_files(root / "ThirdParty", "licenses"),
            },
            nuget_config=managed / "NuGet.Config",
            input_files=[
                Path(__file__),
                root / "distribution/verify_windows.py",
                root / "distribution/verify_bindings.py",
                root / "mise.toml",
            ],
            runtimes=runtimes,
            sources=[base.parent],
            environment=env,
            **options,
        )
        for file in (base, base.with_suffix(".nupkg.json")):
            artifacts.stage_file(file, package.parent / file.name)
    return package, package.parent / base.name


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--runtime", action="append", required=True, choices=RUNTIMES)
    parser.add_argument("--result", required=True, type=Path)
    args = parser.parse_args(argv)
    if sys.platform != "darwin" or sorted(args.runtime) != list(RUNTIMES):
        parser.error("Pack all three distribution runtimes together on macOS")
    provider, core = pack_pair(bindings.ROOT, args.runtime)
    with args.result.open("x", encoding="utf-8") as output:
        output.write(f"{provider.name}\n{core.name}\n")


if __name__ == "__main__":
    main()
