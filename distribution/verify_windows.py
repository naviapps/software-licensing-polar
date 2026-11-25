#!/usr/bin/env python3
"""Build and accept the Windows x64 Polar and pinned core SDKs."""

import argparse
import io
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from pathlib import Path

from sdk_tooling.adapters import load, pinned_tooling, support_inputs
from sdk_tooling.artifacts import file_sha256 as digest
from sdk_tooling.c_headers import prepare_generated
from sdk_tooling.dotnet import pinned_sdk_version
from sdk_tooling.native_plan import ffi_product
from sdk_tooling.windows_delivery import (
    RID,
    TARGET,
    locked_fetch_args,
    publish_versioned_x64,
    stage_verified_library,
)

from distribution import verify_bindings as bindings

ROOT = Path(__file__).resolve().parents[1]
RECEIPT = "software-licensing-polar-windows.json"
CORE_WINDOWS_BUILD = Path("dependencies/software-licensing")


def acceptance_source(env):
    return load("windows_delivery").acceptance_source(ROOT, match_actions=True, environment=env)


def stage_core(env, destination, *, include_macos=False):
    revision = bindings.locked_core_revision()
    resolved = bindings.core_root(env, target=None if include_macos else TARGET)
    git_env = dict(env, GIT_NO_REPLACE_OBJECTS="1")
    if (
        subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=resolved, env=git_env, text=True
        ).strip()
        != revision
    ):
        raise ValueError("Resolved SoftwareLicensing checkout differs from the pinned revision")
    # Build the core's own locked workspace without writing to Cargo's dependency checkout.
    paths = (
        []
        if include_macos
        else [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "crates",
            "LICENSE",
            "mise.toml",
            "native.toml",
            "ThirdParty",
            "bindings/dotnet",
            "README.md",
            ".editorconfig",
            ".gitattributes",
        ]
    )
    archive = subprocess.check_output(
        ["git", "archive", revision, *paths], cwd=resolved, env=git_env
    )
    with tarfile.open(fileobj=io.BytesIO(archive)) as source:
        source.extractall(destination, filter="data")
    return revision


def export_native(artifact_root, native_products, native_evidence, artifacts):
    artifact_root.mkdir(parents=True, exist_ok=True)
    destination = artifact_root / "sdk-windows"
    if destination.exists() or destination.is_symlink():
        raise ValueError("Polar Windows export requires a new artifact directory")
    with tempfile.TemporaryDirectory(dir=artifact_root, prefix="polar-native-") as directory:
        staged = Path(directory) / destination.name
        for name, product in native_products.items():
            output = product.root / ".native-build"
            built = output / "windows" / RID
            selected = staged if name == "vendor" else staged / CORE_WINDOWS_BUILD
            evidence = native_evidence[name][1]
            selected.mkdir(parents=True, exist_ok=True)
            with artifacts.build_lock(output):
                native_receipt = built / "native-build.json"
                if digest(native_receipt) != evidence["receiptSha256"]:
                    raise ValueError("Polar Windows native receipt changed after acceptance")
                inventory = json.loads(native_receipt.read_text(encoding="utf-8"))["outputs"]
                shutil.copytree(built, selected / RID, symlinks=True)
                actual = artifacts.hashes(artifacts.files(selected / RID), selected / RID)
                if actual != inventory | {"native-build.json": evidence["receiptSha256"]}:
                    raise ValueError("Exported Polar Windows files differ from the accepted build")
        staged.rename(destination)


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


def source_inputs(native, artifacts):
    # The provider crate lives at the workspace root. The shared FFI builder
    # records crates/ and the workspace manifests, so add the root source tree.
    provider_sources = native.root / "src"
    if provider_sources.is_symlink() or provider_sources.is_junction():
        raise ValueError("Workspace Rust sources must be inside the SDK checkout")
    configuration = [
        path
        for name in ("config", "config.toml")
        if (path := native.root / ".cargo" / name).is_file() or path.is_symlink()
    ]
    extra = [path for path in artifacts.files(provider_sources) if path.name != ".DS_Store"]
    if any(path.is_symlink() or path.is_junction() for path in [*extra, *configuration]):
        raise ValueError("Workspace Rust inputs must be ordinary files")
    return native.build_sources() | artifacts.hashes([*extra, *configuration], native.root)


def stage_library(native, *, windows, artifacts, cargo, env, tools, output, host):
    def inputs():
        return source_inputs(native, artifacts)

    built = windows.build_library(native, env, tools, "x64", native_host=host, source_inputs=inputs)
    with artifacts.build_lock(native.root / ".native-build"):
        verified, receipt = windows.verify_artifact(
            native, "x64", source_inputs=inputs, environment=env
        )
        if verified != built or receipt.get("toolchain") != {
            "backend": "msvc",
            "host": windows.MSVC_TARGETS[host],
            "target": TARGET,
        }:
            raise ValueError("Windows x64 artifact differs from the selected native host build")
        verify_notices(cargo, native, env)
        library, library_import = stage_verified_library(native, built, output, receipt, artifacts)
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
            "sha256": receipt["outputs"][library.name],
            "receiptSha256": native_receipt,
        },
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--architecture", choices=("x64",), default="x64")
    parser.add_argument(
        "--native-aot", action="store_true", help="Publish and run the Native AOT consumer"
    )
    parser.add_argument(
        "--export-native", action="store_true", help="Export both accepted Windows native builds"
    )
    args = parser.parse_args(argv)
    source = acceptance_source(os.environ)
    tool_inputs = support_inputs(ROOT)
    windows = load("windows")
    artifacts = load("artifacts")
    environment = {
        key.upper(): value
        for key, value in os.environ.items()
        if key.upper() not in {"DOTNET_HOST_PATH", "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER"}
    }
    host = windows.host_architecture()
    compiler_host = windows.MSVC_TARGETS[host]
    env, tools, dotnet = windows.native_environment(host, environment, target_architecture="x64")
    artifact_root = load("vm_environment").artifacts_dir(env)
    if args.export_native and artifact_root is None:
        parser.error("--export-native requires an owned artifact directory")
    env.update(DOTNET_GENERATE_ASPNET_CERTIFICATE="false", RUSTUP_AUTO_INSTALL="0")
    dotnet_version = subprocess.check_output(
        [dotnet, "--version"], cwd=ROOT, env=env, text=True
    ).strip()
    if dotnet_version != pinned_sdk_version(ROOT):
        raise ValueError("Use the pinned x64 .NET SDK from global.json")
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
        bindings.run(*locked_fetch_args(), cwd=ROOT, env=env)
        revision = stage_core(env, core)
        bindings.run(*locked_fetch_args(), cwd=core, env=env)
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
        prepare_generated(ROOT, env=env)
        core_tooling = pinned_tooling(core, candidate / "core-tooling")
        core_tooling("c_headers").prepare_generated(core, env=env)
        core_native = core_tooling("native_plan").ffi_product(core)
        vendor_native = ffi_product(ROOT, factory=load("ffi").FfiProduct)
        accepted = {}
        native_output = candidate / "dlls"
        native_products = {"core": core_native, "vendor": vendor_native}
        for name, native in native_products.items():
            producer = core_tooling if name == "core" else load
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
                windows=producer("windows"),
                artifacts=producer("artifacts"),
                cargo=producer("cargo"),
                env=env,
                tools=tools,
                output=native_output,
                host=host,
            )
            executable = native_output / (name + "-abi.exe")
            load("c_compiler").compile_c(
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
        bindings.managed_tests(
            dotnet,
            core,
            vendor_library,
            core_library,
            env,
            load_tools=load,
            runtime=RID,
        )
        packages = bindings.pack_and_consume(
            dotnet,
            core,
            vendor_library,
            core_library,
            RID,
            env,
            candidate,
            load_tools=load,
            native_aot=args.native_aot,
        )
        if source != acceptance_source(os.environ):
            raise ValueError("Windows x64 acceptance source changed")
        if tool_inputs != support_inputs(ROOT):
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
        if args.native_aot:
            receipt["nativeAotFixtureExecuted"] = True
        if args.export_native:
            export_native(artifact_root, native_products, accepted, artifacts)
        else:
            publish_versioned_x64(
                candidate / "packages",
                receipt,
                version=vendor_native.version,
                receipt_name=RECEIPT,
                output_dir=output,
                artifact_root=artifact_root,
                tooling=artifacts,
            )
    print(f"Windows x64 Polar and SoftwareLicensing acceptance passed on Windows {host}.")


if __name__ == "__main__":
    main()
