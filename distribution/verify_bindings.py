#!/usr/bin/env python3
"""Validate C/.NET against parent-built artifacts; never builds or alters native libraries."""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path
from tempfile import TemporaryDirectory
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
VENDOR_PACKAGE = "NaviApps.SoftwareLicensing.Polar"
CORE_PACKAGE = "NaviApps.SoftwareLicensing"
CORE_URL = "https://github.com/naviapps/software-licensing.git"


def run(*args, env, cwd=ROOT):
    subprocess.run([str(a) for a in args], cwd=cwd, env=env, check=True)


def locked_core_revision():
    dependency = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["dependencies"][
        "software-licensing"
    ]
    revision = dependency.get("rev", "")
    packages = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    sources = [p.get("source") for p in packages if p["name"] == "software-licensing"]
    if (
        dependency.get("git") != CORE_URL
        or not re.fullmatch(r"[a-f0-9]{40}", revision)
        or sources != [f"git+{CORE_URL}?rev={revision}#{revision}"]
    ):
        raise ValueError("Polar requires one immutable SoftwareLicensing dependency in Cargo")
    return revision


def verify_swift_core_pin(env):
    revision = locked_core_revision()
    env = {key: value for key, value in env.items() if key != "SOFTWARE_LICENSING_PACKAGE_PATH"}
    for package in (ROOT, ROOT / "fixtures/consumers/swift"):
        with TemporaryDirectory(prefix="polar-swift-pin-") as scratch:
            manifest = json.loads(
                subprocess.check_output(
                    ["swift", "package", "--scratch-path", scratch, "dump-package"],
                    cwd=package,
                    env=env,
                    text=True,
                )
            )
        sources = [
            source
            for dependency in manifest["dependencies"]
            for source in dependency.get("sourceControl", [])
            if source["identity"] == "software-licensing"
        ]
        if (
            len(sources) != 1
            or sources[0]["location"].get("remote") != [{"urlString": CORE_URL}]
            or sources[0]["requirement"] != {"revision": [revision]}
        ):
            raise ValueError(f"{package / 'Package.swift'} must use the Cargo-locked core revision")


def core_root(env):
    dependency = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["dependencies"][
        "software-licensing"
    ]
    if "path" in dependency:
        candidate = (ROOT / dependency["path"]).resolve()
    else:
        metadata = json.loads(
            subprocess.check_output(
                ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"],
                cwd=ROOT,
                env=env,
                text=True,
            )
        )
        matches = [p for p in metadata["packages"] if p["name"] == "software-licensing"]
        if len(matches) != 1:
            raise SystemExit("The resolved software-licensing source is ambiguous")
        candidate = Path(matches[0]["manifest_path"]).resolve().parent
    for directory in (candidate, *candidate.parents):
        if (
            directory / "bindings/dotnet/src" / CORE_PACKAGE / (CORE_PACKAGE + ".csproj")
        ).is_file():
            return directory
    raise SystemExit("Resolved core source has no managed project")


def prepare_dotnet(env):
    """Resolve the locked core source without requiring a sibling or building native code."""
    core = core_root(env)
    project = core / "bindings/dotnet/src" / CORE_PACKAGE / (CORE_PACKAGE + ".csproj")
    output = ROOT / "build/bindings/core.props"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        "<Project><PropertyGroup>\n"
        "<SoftwareLicensingProject Condition=\"'$(SoftwareLicensingProject)' == ''\">"
        + escape(str(project))
        + "</SoftwareLicensingProject>\n</PropertyGroup></Project>\n",
        encoding="utf-8",
    )
    return output


def framework_binary(root, module):
    return (
        root
        / "Artifacts"
        / (module + ".xcframework")
        / "macos-arm64_x86_64"
        / (module + ".framework")
        / module
    )


def pack_and_consume(dotnet, core, vendor_library, core_library, rid, env, output, *, load_tools):
    """Pack resolved core without modifying its source checkout or runtime staging folder."""
    suffix = ".dll" if rid.startswith("win-") else ".dylib"
    prefix = "" if rid.startswith("win-") else "lib"
    vendor_name = prefix + "software_licensing_polar" + suffix
    core_name = prefix + "software_licensing" + suffix
    stage_native = load_tools("artifacts").stage_native
    staged_vendor = stage_native(vendor_library, output / "vendor-native" / rid / vendor_name)
    staged_core = stage_native(core_library, output / "core-native" / rid / core_name)
    packages = output / "packages"
    packages.mkdir(parents=True, exist_ok=True)
    core_project = core / "bindings/dotnet/src" / CORE_PACKAGE / (CORE_PACKAGE + ".csproj")
    # Core source remains owned by its repository. Add this one freshly built asset
    # through an external targets file instead of writing into its runtimes folder.
    targets = output / "core-pack.targets"
    targets.write_text(
        '''<Project><ItemGroup Condition="'$(MSBuildProjectName)' == 'NaviApps.SoftwareLicensing'">
<None Remove="../../runtimes/**/*" />
<None Remove="../../../../ThirdParty/**/.DS_Store;../../../../ThirdParty/**/registry-notices.json" />
<None Include="'''
        + escape(str(staged_core), {'"': "&quot;"})
        + """" Pack="true" PackagePath="runtimes/"""
        + rid
        + """/native/"""
        + core_name
        + """" />
</ItemGroup></Project>""",
        encoding="utf-8",
    )
    pack_project = load_tools("dotnet").pack_project
    pack_project(
        dotnet,
        core_project,
        output=packages,
        env=env,
        cwd=ROOT,
        properties={
            "CustomAfterMicrosoftCommonTargets": str(targets),
            "PathMap": str(core_project.parent) + "=/software-licensing/dotnet",
            "PublishRepositoryUrl": "false",
            "EnableSourceControlManagerQueries": "false",
            "IncludeSourceRevisionInInformationalVersion": "false",
        },
    )
    pack_project(
        dotnet,
        ROOT / "bindings/dotnet/src" / VENDOR_PACKAGE / (VENDOR_PACKAGE + ".csproj"),
        output=packages,
        env=env,
        cwd=ROOT,
        properties={
            "SoftwareLicensingProject": str(core_project),
            "PolarRuntimeIdentifier": rid,
            "PolarNativeFileName": vendor_name,
            "PolarNativeLibrary": str(staged_vendor),
        },
    )
    versions = {
        CORE_PACKAGE: ET.parse(core_project).findtext("PropertyGroup/Version"),
        VENDOR_PACKAGE: ET.parse(
            ROOT / "bindings/dotnet/src" / VENDOR_PACKAGE / (VENDOR_PACKAGE + ".csproj")
        ).findtext("PropertyGroup/Version"),
    }
    if not all(versions.values()):
        raise SystemExit("Managed package projects must declare their versions")
    built = [
        packages / (name + "." + versions[name] + ".nupkg")
        for name in (CORE_PACKAGE, VENDOR_PACKAGE)
    ]
    nuget = load_tools("nuget")
    hashes = {}
    for package, source_root, filename, staged in (
        (built[0], core, core_name, staged_core),
        (built[1], ROOT, vendor_name, staged_vendor),
    ):
        notices = source_root / "ThirdParty"
        retained = {
            "licenses/" + path.relative_to(notices).as_posix(): path
            for path in notices.rglob("*")
            if path.is_file() and path.name not in {".DS_Store", "registry-notices.json"}
        }
        license_file = source_root / "LICENSE"
        if source_root == ROOT or license_file.is_file():
            retained["LICENSE"] = license_file
        hashes[package.name] = nuget.audit_package(
            package,
            expected_files=retained,
            runtime_files={"runtimes/" + rid + "/native/" + filename: staged},
            forbidden_nuspec_elements=("repository",),
        )
    nuget.run_isolated_consumer(
        dotnet,
        packages=built,
        references={VENDOR_PACKAGE: versions[VENDOR_PACKAGE]},
        source=(ROOT / "fixtures/consumers/dotnet/Program.cs").read_text(encoding="utf-8"),
        work_parent=output,
        env=env,
        runtime=rid,
        properties={"Nullable": "enable", "TreatWarningsAsErrors": "true"},
        clear_environment=(
            "SOFTWARE_LICENSING_NATIVE_LIBRARY",
            "SOFTWARE_LICENSING_POLAR_NATIVE_LIBRARY",
        ),
        exclude_paths=(
            ROOT / "target",
            core / "target",
            vendor_library.parent,
            core_library.parent,
        ),
    )
    return hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--prepare-dotnet",
        action="store_true",
        help="Resolve the locked core managed project; no native build or acceptance",
    )
    mode.add_argument("--c-only", action="store_true")
    mode.add_argument("--dotnet-only", action="store_true")
    mode.add_argument("--verify-swift-pin", action="store_true")
    args = parser.parse_args()
    if args.prepare_dotnet:
        print(prepare_dotnet(dict(os.environ)))
        return
    if args.verify_swift_pin:
        verify_swift_core_pin(os.environ)
        return
    if sys.platform != "darwin":
        raise SystemExit("Run the Windows distribution fixture on an existing worker for x64")
    from sdk_tooling.adapters import load

    env = dict(
        os.environ,
        DOTNET_CLI_TELEMETRY_OPTOUT="1",
        DOTNET_NOLOGO="1",
        DOTNET_CLI_UI_LANGUAGE="en",
        DOTNET_CLI_USE_MSBUILD_SERVER="0",
        MSBUILDDISABLENODEREUSE="1",
    )
    library = framework_binary(ROOT, "CSoftwareLicensingPolar")
    if not library.is_file():
        raise SystemExit(
            "Parent-built Polar framework is missing; bindings validation does not build native code"
        )
    output = ROOT / "build/bindings"
    output.mkdir(parents=True, exist_ok=True)
    if not args.dotnet_only:
        load("c_compiler").compile_c(
            ROOT / "crates/ffi/tests/c/abi.c",
            compiler=("xcrun", "clang"),
            output=output / "abi",
            include_dirs=(ROOT / "crates/ffi/include",),
            libraries=(library,),
            standard="c17",
            flags=("-Wl,-rpath," + str(library.parent.parent),),
            env=env,
            cwd=ROOT,
        )
        run(output / "abi", env=env)
    if not args.c_only:
        dotnet = env.get("DOTNET_BIN") or shutil.which("dotnet")
        if not dotnet:
            raise SystemExit("Set DOTNET_BIN to the task-scoped .NET 10 executable")
        core = core_root(env)
        core_library = framework_binary(core, "CSoftwareLicensing")
        if not core_library.is_file():
            raise SystemExit("The resolved core's tracked framework is missing")
        env["SOFTWARE_LICENSING_POLAR_NATIVE_LIBRARY"] = str(library)
        env["SOFTWARE_LICENSING_NATIVE_LIBRARY"] = str(core_library)
        core_project = core / "bindings/dotnet/src" / CORE_PACKAGE / (CORE_PACKAGE + ".csproj")
        load("dotnet").run_managed(
            dotnet,
            ROOT
            / "bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/SoftwareLicensing.Polar.Tests.csproj",
            env=env,
            cwd=ROOT,
            properties={"SoftwareLicensingProject": str(core_project)},
            arguments=("--native",),
            verify_style=True,
        )
        rid = "osx-arm64" if platform.machine().lower() in ("arm64", "aarch64") else "osx-x64"
        packages = pack_and_consume(
            dotnet, core, library, core_library, rid, env, output, load_tools=load
        )
        (output / "acceptance.json").write_text(
            json.dumps(
                {
                    "runtime": rid,
                    "nativeSha256": hashlib.sha256(library.read_bytes()).hexdigest(),
                    "coreNativeSha256": hashlib.sha256(core_library.read_bytes()).hexdigest(),
                    "packages": packages,
                    "coverage": [
                        "managed ownership races",
                        "native configuration and preflight rejection",
                        "clean NuGet consumer with native Polar rejection and core recovery",
                    ],
                    "notRun": ["real License API", "Windows runtime"],
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
    print(
        "Bindings acceptance passed against unchanged parent-built framework artifacts; no License API requests."
    )


if __name__ == "__main__":
    main()
