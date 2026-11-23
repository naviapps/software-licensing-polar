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
            ["git", "rev-parse", "HEAD"],
            cwd=Path("from-cargo"),
            env={"GIT_NO_REPLACE_OBJECTS": "1"},
            text=True,
        )

    def test_export_keeps_both_native_receipts_and_rejects_changed_build_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = load("artifacts")
            products, evidence = {}, {}
            for name in ("core", "vendor"):
                source = root / name
                built = source / ".native-build/windows" / windows.RID
                built.mkdir(parents=True)
                library = built / f"{name}.dll"
                library.write_bytes(name.encode())
                receipt = built / "native-build.json"
                receipt.write_text(json.dumps({"outputs": {library.name: windows.digest(library)}}))
                products[name] = SimpleNamespace(root=source)
                evidence[name] = (library, {"receiptSha256": windows.digest(receipt)})

            destination = root / "artifacts"
            destination.mkdir()
            windows.export_native(destination, products, evidence, artifacts)
            exported = destination / "sdk-windows"
            self.assertEqual(
                windows.digest(exported / windows.RID / "native-build.json"),
                evidence["vendor"][1]["receiptSha256"],
            )
            self.assertEqual(
                windows.digest(
                    exported / "dependencies/software-licensing" / windows.RID / "native-build.json"
                ),
                evidence["core"][1]["receiptSha256"],
            )
            bad_candidate = root / "changed"
            bad_candidate.mkdir()
            evidence["vendor"][0].write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "differ from the accepted build"):
                windows.export_native(bad_candidate, products, evidence, artifacts)
            self.assertFalse((bad_candidate / "sdk-windows").exists())

    def test_windows_receipt_binds_root_provider_and_cargo_configuration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            provider = root / "src" / "transport.rs"
            provider.parent.mkdir()
            provider.write_text("// first provider\n")
            config = root / ".cargo" / "config.toml"
            config.parent.mkdir()
            config.write_text("[build]\n")
            native = SimpleNamespace(root=root, build_sources=lambda: {"Cargo.toml": "locked"})
            artifacts = load("artifacts")

            before = windows.source_inputs(native, artifacts)
            self.assertEqual(
                before,
                {
                    ".cargo/config.toml": windows.digest(config),
                    "Cargo.toml": "locked",
                    "src/transport.rs": windows.digest(provider),
                },
            )
            provider.write_text("// changed provider\n")
            self.assertNotEqual(windows.source_inputs(native, artifacts), before)

    def test_stage_rejects_a_dll_that_changed_after_native_receipt_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            native = SimpleNamespace(root=root, library="software_licensing_polar")
            built = root / "built"
            built.mkdir()
            dll = built / (native.library + ".dll")
            dll.write_bytes(b"changed DLL")
            library = dll.with_suffix(".dll.lib")
            library.write_bytes(b"import library")
            shared, artifacts = MagicMock(), load("artifacts")
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
                    "outputs": {
                        dll.name: "a" * 64,
                        library.name: artifacts.file_sha256(library),
                    },
                },
            )
            with (
                patch.object(windows, "verify_notices"),
                self.assertRaisesRegex(ValueError, "differs from its verified receipt"),
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
            self.assertIs(
                shared.build_library.call_args.kwargs["source_inputs"],
                shared.verify_artifact.call_args.kwargs["source_inputs"],
            )

    def test_x64_pipeline_runs_both_c_fixtures_and_consumes_same_provider_core_pair(self):
        for host, compiler_host in TARGETS.items():
            with self.subTest(host=host):
                self.check_x64_pipeline(host, compiler_host)

    def test_x64_pipeline_does_not_retain_evidence_after_support_changes(self):
        self.check_x64_pipeline("x64", TARGETS["x64"], change_support=True)

    def test_x64_aot_pipeline_publishes_only_after_running_the_package_pair(self):
        self.check_x64_pipeline("x64", TARGETS["x64"], native_aot=True)

    def test_combined_export_retains_native_inputs_instead_of_the_windows_only_pair(self):
        self.check_x64_pipeline("x64", TARGETS["x64"], native_aot=True, export_native=True)

    def check_x64_pipeline(
        self, host, compiler_host, *, change_support=False, native_aot=False, export_native=False
    ):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "global.json").write_text(
                '{"sdk":{"version":"10.0.403","rollForward":"disable"}}'
            )
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

            def select_product(path, *, factory):
                return factory(
                    path,
                    symbol_prefix=(
                        "software_licensing_polar_" if path == root else "software_licensing_"
                    ),
                    specification="native.toml",
                )

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
                patch.object(windows, "ffi_product", side_effect=select_product),
                patch.object(
                    windows, "support_inputs", side_effect=[tool_inputs, final_tool_inputs]
                ),
                patch.object(windows, "stage_core", return_value=revision),
                patch.object(windows, "verify_inherited_notices"),
                patch.object(windows, "stage_library", side_effect=stage),
                patch.object(windows, "publish_versioned_x64") as retain,
                patch.object(windows, "export_native") as export,
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
                options = ["--native-aot"] if native_aot else []
                if export_native:
                    options.append("--export-native")
                windows.main(options)
                if export_native:
                    retain.assert_not_called()
                    export.assert_called_once()
                    self.assertEqual(export.call_args.args[0], artifacts_dir)
                    self.assertEqual(set(export.call_args.args[1]), {"core", "vendor"})
                    self.assertTrue(pack.call_args.kwargs["native_aot"])
                    return
                export.assert_not_called()

            core, vendor = ffi.FfiProduct.call_args_list
            self.assertEqual(core.args[0].name, "core")
            self.assertEqual(core.kwargs["specification"], "native.toml")
            self.assertEqual(vendor.args[0], root)
            self.assertEqual(vendor.kwargs["specification"], "native.toml")
            shared.native_environment.assert_called_once_with(host, {}, target_architecture="x64")
            shared.require_rust_target_stdlib.assert_called_once_with("rust-stdlib", windows.TARGET)
            targets = [entry.args for entry in run.call_args_list if "--target" in entry.args]
            self.assertEqual(len(targets), 6)
            self.assertEqual(targets[:2], [windows.locked_fetch_args()] * 2)
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
            self.assertEqual(managed.kwargs["arguments"], ("--native",))
            pair = pack.call_args.args
            self.assertEqual(pair[0], dotnet)
            self.assertEqual(
                [path.name for path in pair[2:4]],
                ["software_licensing_polar.dll", "software_licensing.dll"],
            )
            self.assertEqual(pair[4], windows.RID)
            self.assertEqual(
                pack.call_args.kwargs, {"load_tools": load_tooling, "native_aot": native_aot}
            )
            properties = managed.kwargs["properties"]
            self.assertEqual(Path(properties["PolarNativeLibrary"]), pair[2])
            self.assertEqual(Path(properties["SoftwareLicensingNativeLibrary"]), pair[3])
            receipt = retain.call_args.args[1]
            self.assertEqual(retain.call_args.kwargs["artifact_root"], artifacts_dir)
            self.assertEqual(receipt["source"], source[0])
            self.assertEqual(receipt["coreRevision"], revision)
            self.assertEqual(receipt["toolInputs"], tool_inputs)
            self.assertEqual(receipt["runtime"], "win-x64")
            self.assertEqual(receipt.get("nativeAotFixtureExecuted", False), native_aot)
            self.assertEqual(
                receipt["execution"], "x64-emulated-on-arm64" if host == "arm64" else "x64-native"
            )
            self.assertEqual(receipt["toolchain"]["host"], compiler_host)


if __name__ == "__main__":
    unittest.main()
