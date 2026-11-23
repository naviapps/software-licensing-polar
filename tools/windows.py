#!/usr/bin/env python3
"""Build and accept the Windows x64 Polar and pinned core SDKs."""

import argparse
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from contextlib import ExitStack
from pathlib import Path

from _tooling import load, support_inputs

import bindings

ROOT = Path(__file__).resolve().parent.parent
TARGET = "x86_64-pc-windows-msvc"
RID = "win-x64"
RECEIPT = "software-licensing-polar-windows.json"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def acceptance_source(env):
    commit = env.get("NAVI_CI_SOURCE_COMMIT")
    fingerprint = env.get("NAVI_CI_SOURCE_FINGERPRINT")
    if commit is not None or fingerprint is not None:
        if not re.fullmatch(r"[a-f0-9]{64}", fingerprint or ""):
            raise ValueError("Captured Windows x64 acceptance requires the workspace SHA-256")
    else:
        if subprocess.check_output(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            cwd=ROOT,
            env=env,
            text=True,
        ).strip():
            raise ValueError("Windows x64 acceptance requires a clean committed checkout")
        commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, env=env, text=True
        ).strip()
    if not re.fullmatch(r"[a-f0-9]{40}", commit or ""):
        raise ValueError("Windows x64 acceptance requires the full source commit SHA")
    if env.get("GITHUB_SHA", commit) != commit:
        raise ValueError("Windows x64 acceptance checkout differs from the Actions source")
    return commit, fingerprint


def stage_core(env, destination):
    dependency = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["dependencies"][
        "software-licensing"
    ]
    revision = dependency.get("rev", "")
    url = "https://github.com/naviapps/software-licensing.git"
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    sources = [p.get("source") for p in lock if p["name"] == "software-licensing"]
    swift = json.loads((ROOT / "tools/swift-packages.json").read_text())
    if (
        not re.fullmatch(r"[a-f0-9]{40}", revision)
        or dependency.get("git") != url
        or sources != [f"git+{url}?rev={revision}#{revision}"]
        or swift != {"naviapps/software-licensing": revision}
    ):
        raise ValueError("Polar requires the same immutable SoftwareLicensing dependency")
    resolved = bindings.core_root(env)
    if (
        subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=resolved, env=env, text=True
        ).strip()
        != revision
    ):
        raise ValueError("Resolved SoftwareLicensing checkout differs from the pinned revision")
    # Historical core revisions may predate a first-party LICENSE.
    licenses = subprocess.check_output(
        ["git", "ls-tree", "--name-only", revision, "--", "LICENSE"],
        cwd=resolved,
        env=env,
        text=True,
    ).splitlines()
    # Build the core's own locked workspace without writing to Cargo's dependency checkout.
    archive = subprocess.check_output(
        [
            "git",
            "archive",
            revision,
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "crates",
            "tools/native.py",
            "ThirdParty",
            "bindings/dotnet",
            "README.md",
            ".editorconfig",
            ".gitattributes",
            *licenses,
        ],
        cwd=resolved,
        env=env,
    )
    with tarfile.open(fileobj=io.BytesIO(archive)) as source:
        source.extractall(destination, filter="data")
    return revision


def verify_notices(cargo, native, env):
    notices = cargo.CargoDependencies(native.root, [native.package], [TARGET], env)
    for directory, record in notices.registry_notices().items():
        for relative, data in record["files"].items():
            committed = native.root / "ThirdParty" / directory / relative
            if committed.is_symlink() or not committed.is_file() or committed.read_bytes() != data:
                raise ValueError(
                    f"Missing or changed Windows x64 registry notice: {directory}/{relative}"
                )


def verify_inherited_notices(core):
    directory = core / "ThirdParty/LicenseKit"
    if not (directory / "LICENSE").is_file() or not (directory / "LICENSE").stat().st_size:
        raise ValueError("Pinned SoftwareLicensing LicenseKit notice is missing")
    for path in directory.rglob("*"):
        if path.is_file():
            inherited = ROOT / "ThirdParty" / path.relative_to(core / "ThirdParty")
            if not inherited.is_file() or inherited.read_bytes() != path.read_bytes():
                raise ValueError("Polar inherited LicenseKit notice differs from SoftwareLicensing")


def stage_library(native, *, windows, tooling, artifacts, cargo, env, tools, output, host):
    built = windows.build_library(native, env, tools, "x64", native_host=host)
    with artifacts.build_lock(native.root / ".native-build"):
        verified, receipt = windows.verify_artifact(native, "x64", environment=env)
        if verified != built or receipt.get("toolchain") != {
            "backend": "msvc",
            "host": windows.MSVC_TARGETS[host],
            "target": TARGET,
        }:
            raise ValueError("Windows x64 artifact differs from the selected native host build")
        verify_notices(cargo, native, env)
        dll = native.library + ".dll"
        library = tooling.stage_native(built / dll, output / dll)
        library_import = tooling.stage_native(built / (dll + ".lib"), output / (dll + ".lib"))
        if digest(library) != receipt["outputs"][dll]:
            raise ValueError("Staged Windows x64 DLL differs from its native build receipt")
        if digest(library_import) != receipt["outputs"][dll + ".lib"]:
            raise ValueError(
                "Staged Windows x64 import library differs from its native build receipt"
            )
        native_receipt = digest(built / "native-build.json")
    exports = windows.verify_library(
        library, native.headers, native.symbol_prefix, tools, env, "x64"
    )
    if sorted(exports) != receipt["exports"]:
        raise ValueError("Staged Windows x64 exports differ from the native build receipt")
    return (
        library,
        library_import,
        {
            "exports": sorted(exports),
            "sha256": digest(library),
            "receiptSha256": native_receipt,
        },
    )


def retain_evidence(package_directory, receipt, *, version, output, artifacts, tooling):
    relative = Path(RID) / version
    local = output / relative
    external = artifacts / relative if artifacts is not None else local
    destinations = [local]
    if local.resolve() != external.resolve():
        if local.resolve().is_relative_to(external.resolve()) or external.resolve().is_relative_to(
            local.resolve()
        ):
            raise ValueError("Local and external Windows x64 evidence paths must be disjoint")
        destinations.append(external)
    names = receipt["packages"].keys() | {RECEIPT}

    def check_destination(path):
        if (
            path.parent.is_symlink()
            or path.parent.is_junction()
            or path.is_symlink()
            or path.is_junction()
            or (
                path.exists()
                and (
                    not path.is_dir()
                    or any(
                        entry.name not in names or not entry.is_file() or entry.is_symlink()
                        for entry in path.iterdir()
                    )
                )
            )
        ):
            raise ValueError("Windows x64 evidence contains files outside the SDK package pair")

    def verify(path):
        if (
            {entry.name for entry in path.iterdir()} != names
            or json.loads((path / RECEIPT).read_bytes()) != receipt
            or any(
                digest(path / name) != expected for name, expected in receipt["packages"].items()
            )
        ):
            raise ValueError("Windows x64 evidence differs from the accepted SDK package pair")

    with tooling.build_lock(output), ExitStack() as temporary:
        stages = []
        for destination in destinations:
            check_destination(destination)
            destination.parent.mkdir(parents=True, exist_ok=True)
            scratch = temporary.enter_context(
                tempfile.TemporaryDirectory(dir=destination.parent, prefix="polar-evidence-")
            )
            stage = Path(scratch) / version
            stage.mkdir()
            for name in receipt["packages"]:
                shutil.copyfile(package_directory / name, stage / name)
            tooling.write_json(stage / RECEIPT, receipt)
            verify(stage)
            stages.append(stage)

        cleanup_failure = None

        def verify_local():
            nonlocal cleanup_failure
            verify(local)
            if len(stages) == 1:
                return
            check_destination(external)
            external_verified = False

            def verify_external():
                nonlocal external_verified
                verify(external)
                external_verified = True

            try:
                tooling.publish(stages[1], external, verify_external)
            except BaseException as error:
                if not external_verified:
                    raise
                verify(external)
                cleanup_failure = error

        check_destination(local)
        tooling.publish(stages[0], local, verify_local)
        if cleanup_failure is not None:
            raise cleanup_failure


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--architecture", choices=("x64",), default="x64")
    parser.parse_args(argv)
    source = acceptance_source(os.environ)
    tool_inputs = support_inputs()
    windows = load("windows")
    tooling = load("bindings")
    artifacts = load("artifacts")
    cargo = load("cargo")
    builder = load("native").NativeBuilder
    environment = {
        key.upper(): value
        for key, value in os.environ.items()
        if key.upper() not in {"DOTNET_HOST_PATH", "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER"}
    }
    host = windows.host_architecture()
    compiler_host = windows.MSVC_TARGETS[host]
    env, tools, dotnet = windows.native_environment(host, environment, target_architecture="x64")
    env.update(DOTNET_GENERATE_ASPNET_CERTIFICATE="false", RUSTUP_AUTO_INSTALL="0")
    dotnet_version = subprocess.check_output(
        [dotnet, "--version"], cwd=ROOT, env=env, text=True
    ).strip()
    if dotnet_version != tomllib.loads((ROOT / "mise.toml").read_text())["tools"]["dotnet"]:
        raise ValueError("Use the pinned x64 .NET SDK from mise.toml")
    compiler = subprocess.check_output(
        ["rustc", "--version", "--verbose"], cwd=ROOT, env=env, text=True
    )
    pinned_rust = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if (
        not compiler.startswith(f"rustc {pinned_rust} ")
        or f"host: {compiler_host}" not in compiler.splitlines()
    ):
        raise ValueError(f"Use the pinned {compiler_host} Rust toolchain")
    windows.require_rust_target_stdlib(
        subprocess.check_output(
            ["rustc", "--print", "target-libdir", "--target", TARGET], cwd=ROOT, env=env, text=True
        ).strip(),
        TARGET,
    )

    output = ROOT / ".native-build"
    output.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output, prefix="polar-x64-") as directory:
        candidate = Path(directory)
        core = candidate / "core"
        bindings.run("cargo", "fetch", "--locked", cwd=ROOT, env=env)
        revision = stage_core(env, core)
        bindings.run("cargo", "fetch", "--locked", cwd=core, env=env)
        verify_inherited_notices(core)
        remaps = [
            (ROOT, "/software-licensing-polar"),
            (core, "/software-licensing"),
            (Path.home(), "/developer"),
        ]
        if env.get("CARGO_HOME"):
            remaps.append((env["CARGO_HOME"], "/cargo"))
        env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(
            [
                env["CARGO_ENCODED_RUSTFLAGS"],
                *[f"--remap-path-prefix={path}={alias}" for path, alias in remaps],
            ]
        )
        core_native = builder(core, symbol_prefix="software_licensing_", minimum_macos="26.0")
        vendor_native = builder(
            ROOT, symbol_prefix="software_licensing_polar_", minimum_macos="26.0"
        )
        bindings.run("cargo", "fmt", "--all", "--", "--check", cwd=ROOT, env=env)
        accepted = {}
        native_output = candidate / "dlls"
        for name, native in (("core", core_native), ("vendor", vendor_native)):
            # Keep test output separate from the source-bound native build stages.
            test_env = dict(env, CARGO_TARGET_DIR=str(candidate / (name + "-test")))
            commands = ("--workspace", "--target", TARGET, "--locked", "--offline")
            bindings.run(
                "cargo",
                "clippy",
                "--all-targets",
                *commands,
                "--",
                "-D",
                "warnings",
                cwd=native.root,
                env=test_env,
            )
            bindings.run("cargo", "test", *commands, cwd=native.root, env=test_env)
            library, import_library, evidence = stage_library(
                native,
                windows=windows,
                tooling=tooling,
                artifacts=artifacts,
                cargo=cargo,
                env=env,
                tools=tools,
                output=native_output,
                host=host,
            )
            executable = native_output / (name + "-abi.exe")
            tooling.compile_c(
                native.ffi / "tests/c/abi.c",
                output=executable,
                compiler=[tools / "cl.exe"],
                env=env,
                cwd=native.root,
                style="msvc",
                standard="c17",
                include_dirs=[native.ffi / "include"],
                libraries=[import_library],
            )
            # Verify the x64 executable before running the C fixture against the staged DLL.
            windows.require_pe_machine(executable, "x64")
            bindings.run(executable, cwd=native_output, env=env)
            accepted[name] = (library, evidence)

        core_library, core_evidence = accepted["core"]
        vendor_library, vendor_evidence = accepted["vendor"]
        if any(name.startswith(vendor_native.symbol_prefix) for name in core_evidence["exports"]):
            raise ValueError("Core DLL exposes Polar provider operations")
        test_env = dict(
            env,
            SOFTWARE_LICENSING_NATIVE_LIBRARY=str(core_library),
            SOFTWARE_LICENSING_POLAR_NATIVE_LIBRARY=str(vendor_library),
        )
        core_project = (
            core
            / "bindings/dotnet/src"
            / bindings.CORE_PACKAGE
            / (bindings.CORE_PACKAGE + ".csproj")
        )
        tooling.run_managed(
            dotnet,
            ROOT
            / "bindings/dotnet/tests/SoftwareLicensing.Polar.Tests/SoftwareLicensing.Polar.Tests.csproj",
            cwd=ROOT,
            properties={"SoftwareLicensingProject": str(core_project), "UseAppHost": "false"},
            arguments=("--native",),
            env=test_env,
            verify_style=True,
            runtime=RID,
        )
        packages = bindings.pack_and_consume(
            dotnet,
            core,
            vendor_library,
            core_library,
            RID,
            test_env,
            candidate,
            tooling=tooling,
        )
        if source != acceptance_source(os.environ):
            raise ValueError("Windows x64 acceptance source changed")
        if tool_inputs != support_inputs():
            raise ValueError("Windows x64 acceptance tooling changed")
        if any(digest(library) != evidence["sha256"] for library, evidence in accepted.values()):
            raise ValueError("Accepted Windows x64 DLL changed during package validation")
        not_run = ["real License API", "host app integration"]
        if host == "arm64":
            not_run.append("physical Windows x64 host")
        receipt = {
            "source": source[0],
            "workspaceFingerprint": source[1],
            "coreRevision": revision,
            "toolInputs": tool_inputs,
            "dotnetVersion": dotnet_version,
            "compiler": compiler.splitlines()[0],
            "target": TARGET,
            "runtime": RID,
            "execution": "x64-emulated-on-arm64" if host == "arm64" else "x64-native",
            "toolchain": {"backend": "msvc", "host": compiler_host, "target": TARGET},
            "native": {"core": core_evidence, "vendor": vendor_evidence},
            "packages": packages,
            "coverage": [
                "Rust workspace tests",
                "C ownership for both DLLs",
                "managed callback and cancellation",
                "independent NuGet lifecycle",
            ],
            "notRun": not_run,
        }
        retain_evidence(
            candidate / "packages",
            receipt,
            version=vendor_native.version,
            output=output,
            artifacts=Path(env["NAVI_CI_ARTIFACTS"]) if env.get("NAVI_CI_ARTIFACTS") else None,
            tooling=artifacts,
        )
    print(f"Windows x64 Polar and SoftwareLicensing acceptance passed on Windows {host}.")


if __name__ == "__main__":
    main()
