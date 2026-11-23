"""Portable tests for the Windows x64 Polar and pinned-core acceptance driver."""

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, call, patch

from sdk_tooling.adapters import load

from distribution import verify_windows as windows

TARGETS = {"arm64": "aarch64-pc-windows-msvc", "x64": windows.TARGET}


class WindowsTests(unittest.TestCase):
    def test_captured_source_requires_commit_fingerprint_and_matching_actions_source(self):
        approved = "b" * 40
        fingerprint = "d" * 64
        env = {"VM_RUNNER_SOURCE_COMMIT": approved, "VM_RUNNER_SOURCE_FINGERPRINT": fingerprint}
        self.assertEqual(windows.acceptance_source(env), (approved, fingerprint))
        with self.assertRaisesRegex(ValueError, "workspace SHA-256"):
            windows.acceptance_source({"VM_RUNNER_SOURCE_COMMIT": approved})
        with self.assertRaisesRegex(ValueError, "Actions revision"):
            windows.acceptance_source(dict(env, GITHUB_SHA="c" * 40))

    def test_core_checkout_must_match_pin_before_archiving(self):
        with (
            patch.object(windows.bindings, "core_root", return_value=Path("from-cargo")),
            patch.object(windows.subprocess, "check_output", return_value="d" * 40) as git,
            self.assertRaisesRegex(ValueError, "pinned revision"),
        ):
            windows.stage_core({}, Path("isolated-build"))
        git.assert_called_once_with(
            ["git", "rev-parse", "HEAD"], cwd=Path("from-cargo"), env={}, text=True
        )

    def test_stage_rejects_a_dll_that_changed_after_native_receipt_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            native = SimpleNamespace(root=root, library="software_licensing_polar")
            built = root / "built"
            built.mkdir()
            dll = built / (native.library + ".dll")
            dll.write_bytes(b"changed DLL")
            shared, artifacts = MagicMock(), MagicMock()
            shared.MSVC_TARGETS = TARGETS
            shared.build_library.return_value = built
            shared.verify_artifact.return_value = (
                built,
                {
                    "toolchain": {
                        "backend": "msvc",
                        "host": TARGETS["arm64"],
                        "target": windows.TARGET,
                    },
                    "outputs": {dll.name: "a" * 64},
                },
            )
            artifacts.stage_native.side_effect = lambda source, destination: source
            with (
                patch.object(windows, "verify_notices"),
                self.assertRaisesRegex(ValueError, "DLL differs from its native build receipt"),
            ):
                windows.stage_library(
                    native,
                    windows=shared,
                    artifacts=artifacts,
                    cargo=MagicMock(),
                    tools=root / "msvc",
                    env={},
                    output=root / "staged",
                    host="arm64",
                )
            shared.verify_library.assert_not_called()

    def test_x64_pipeline_runs_both_c_fixtures_and_consumes_same_provider_core_pair(self):
        for host, compiler_host in TARGETS.items():
            with self.subTest(host=host):
                self.check_x64_pipeline(host, compiler_host)

    def test_x64_pipeline_does_not_retain_evidence_after_support_changes(self):
        self.check_x64_pipeline("x64", TARGETS["x64"], change_support=True)

    def check_x64_pipeline(self, host, compiler_host, *, change_support=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "mise.toml").write_text('[tools]\ndotnet = "10.0.403"\n')
            (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.1"\n')
            shared = MagicMock()
            c_compiler = MagicMock()
            dotnet_tools = MagicMock()
            cargo = MagicMock()
            artifacts = MagicMock()
            ffi = MagicMock()
            dotnet = root / "x64-dotnet.exe"
            artifacts_dir = root / "vm-artifacts"
            env = {
                "CARGO_ENCODED_RUSTFLAGS": "-Ctarget-feature=+crt-static",
                "VM_RUNNER_ARTIFACTS_DIR": str(artifacts_dir),
            }
            shared.host_architecture.return_value = host
            shared.MSVC_TARGETS = TARGETS
            shared.native_environment.return_value = (env, root / "msvc", dotnet)

            def create_product(path, *, symbol_prefix, specification):
                return SimpleNamespace(
                    root=path,
                    ffi=path / "crates/ffi",
                    version="0.1.0",
                    symbol_prefix=symbol_prefix,
                    specification=specification,
                )

            ffi.FfiProduct.side_effect = create_product
            modules = dict(
                windows=shared,
                c_compiler=c_compiler,
                dotnet=dotnet_tools,
                artifacts=artifacts,
                cargo=cargo,
                ffi=ffi,
                vm_environment=load("vm_environment"),
            )

            def stage(native, *, output, host, **_):
                self.assertEqual(TARGETS[host], compiler_host)
                output.mkdir(exist_ok=True)
                library = output / (native.symbol_prefix.rstrip("_") + ".dll")
                library.write_bytes(native.symbol_prefix.encode())
                return (
                    library,
                    library.with_suffix(".lib"),
                    {
                        "exports": [native.symbol_prefix + "abi_version"],
                        "sha256": windows.digest(library),
                    },
                )

            source = ("b" * 40, None)
            revision = "c" * 40
            tool_inputs = {"native.toml": "d" * 64}
            final_tool_inputs = {"native.toml": "e" * 64} if change_support else tool_inputs
            with (
                patch.object(windows, "ROOT", root),
                patch.object(windows.Path, "home", return_value=root / "developer"),
                patch.object(windows, "acceptance_source", return_value=source),
                patch.object(windows, "load", side_effect=modules.__getitem__) as load_tooling,
                patch.object(
                    windows,
                    "ffi_product",
                    side_effect=lambda path, *, factory: factory(
                        path,
                        symbol_prefix="software_licensing_polar_",
                        specification="native.toml",
                    ),
                ) as vendor_product,
                patch.object(
                    windows, "support_inputs", side_effect=[tool_inputs, final_tool_inputs]
                ),
                patch.object(windows, "stage_core", return_value=revision),
                patch.object(windows, "verify_inherited_notices"),
                patch.object(windows, "stage_library", side_effect=stage),
                patch.object(windows, "retain_evidence") as retain,
                patch.object(windows.bindings, "run") as run,
                patch.object(
                    windows.bindings, "pack_and_consume", return_value={"packages": "sentinel"}
                ) as pack,
                patch.object(
                    windows.subprocess,
                    "check_output",
                    side_effect=[
                        "10.0.403\n",
                        f"rustc 1.99.1 (fixture)\nhost: {compiler_host}\n",
                        "rust-stdlib\n",
                    ],
                ),
                patch.dict(windows.os.environ, {"DOTNET_HOST_PATH": "wrong-host"}, clear=True),
                patch.object(windows, "print", create=True),
            ):
                if change_support:
                    with self.assertRaisesRegex(ValueError, "acceptance tooling changed"):
                        windows.main([])
                    retain.assert_not_called()
                    return
                windows.main([])

            vendor_product.assert_called_once_with(root, factory=ffi.FfiProduct)
            core, vendor = ffi.FfiProduct.call_args_list
            self.assertEqual(core.args[0].name, "core")
            self.assertEqual(core.kwargs["specification"], "tools/native.py")
            self.assertEqual(vendor.kwargs["specification"], "native.toml")
            shared.native_environment.assert_called_once_with(host, {}, target_architecture="x64")
            shared.require_rust_target_stdlib.assert_called_once_with("rust-stdlib", windows.TARGET)
            targets = [entry.args for entry in run.call_args_list if "--target" in entry.args]
            self.assertEqual(len(targets), 4)
            self.assertTrue(
                all(entry[entry.index("--target") + 1] == windows.TARGET for entry in targets)
            )
            executables = [entry.kwargs["output"] for entry in c_compiler.compile_c.call_args_list]
            self.assertEqual(
                [entry.name for entry in executables], ["core-abi.exe", "vendor-abi.exe"]
            )
            self.assertEqual(
                shared.require_pe_machine.call_args_list,
                [call(path, "x64") for path in executables],
            )
            managed = dotnet_tools.run_managed.call_args
            self.assertEqual(managed.args[0], dotnet)
            self.assertEqual(managed.kwargs["runtime"], windows.RID)
            self.assertEqual(managed.kwargs["properties"]["UseAppHost"], "false")
            pair = pack.call_args.args
            self.assertEqual(pair[0], dotnet)
            self.assertEqual(
                [path.name for path in pair[2:4]],
                ["software_licensing_polar.dll", "software_licensing.dll"],
            )
            self.assertEqual(pair[4], windows.RID)
            self.assertEqual(pack.call_args.kwargs, {"load_tools": load_tooling})
            receipt = retain.call_args.args[1]
            self.assertEqual(retain.call_args.kwargs["artifacts"], artifacts_dir)
            self.assertEqual(receipt["source"], source[0])
            self.assertEqual(receipt["coreRevision"], revision)
            self.assertEqual(receipt["toolInputs"], tool_inputs)
            self.assertEqual(receipt["runtime"], "win-x64")
            self.assertEqual(
                receipt["execution"], "x64-emulated-on-arm64" if host == "arm64" else "x64-native"
            )
            self.assertEqual(receipt["toolchain"]["host"], compiler_host)

    def test_retains_two_archives_and_receipt_without_overwriting_unrelated_files(self):
        artifacts = load("artifacts")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            packages = root / "packages"
            packages.mkdir()
            for name in ("core.nupkg", "polar.nupkg"):
                (packages / name).write_text(name)
            receipt = {"packages": {path.name: windows.digest(path) for path in packages.iterdir()}}
            local, external = root / "local", root / "external"
            local.mkdir()
            output = dict(version="0.1.0", output=local, artifacts=external, tooling=artifacts)
            windows.retain_evidence(packages, receipt, **output)
            for path in (local, external):
                retained = path / "win-x64/0.1.0"
                self.assertEqual(json.loads((retained / windows.RECEIPT).read_text()), receipt)
                self.assertEqual(
                    {p.name: windows.digest(p) for p in retained.glob("*.nupkg")},
                    receipt["packages"],
                )
            unrelated = external / "win-x64/0.1.0/keep.txt"
            unrelated.write_text("user file")
            with self.assertRaisesRegex(ValueError, "outside the SDK package pair"):
                windows.retain_evidence(packages, receipt, **output)
            self.assertEqual(unrelated.read_text(), "user file")


if __name__ == "__main__":
    unittest.main()
