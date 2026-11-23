"""Verify the provider's managed dependency and package policy."""

import json
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import patch

from fixtures.distribution import bindings


class DependencySourceTests(unittest.TestCase):
    def test_standalone_prepare_uses_resolved_source_and_preserves_explicit_override(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "standalone-vendor"
            root.mkdir()
            core = Path(directory) / "immutable & core"
            with (
                patch.object(bindings, "ROOT", root),
                patch.object(bindings, "core_root", return_value=core) as resolve,
            ):
                output = bindings.prepare_dotnet({"fixture": "environment"})
            resolve.assert_called_once_with({"fixture": "environment"})
            node = ET.parse(output).getroot().find("PropertyGroup/SoftwareLicensingProject")
            self.assertEqual(
                node.text,
                str(
                    core
                    / "bindings/dotnet/src"
                    / bindings.CORE_PACKAGE
                    / (bindings.CORE_PACKAGE + ".csproj")
                ),
            )
            self.assertEqual(node.attrib["Condition"], "'$(SoftwareLicensingProject)' == ''")
            self.assertEqual(output, root / "build/bindings/core.props")
            self.assertFalse((root.parent / "software-licensing").exists())

    def test_pinned_dependency_resolves_managed_source_from_cargo_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            vendor = root / "vendor"
            vendor.mkdir()
            (vendor / "Cargo.toml").write_text(
                '[workspace.dependencies]\nsoftware-licensing = {git="https://example.invalid/core", rev="fixture"}\n'
            )
            core = root / "resolved-core"
            project = (
                core
                / "bindings/dotnet/src/NaviApps.SoftwareLicensing/NaviApps.SoftwareLicensing.csproj"
            )
            project.parent.mkdir(parents=True)
            project.write_text('<Project Sdk="Microsoft.NET.Sdk" />')
            metadata = {
                "packages": [
                    {
                        "name": "software-licensing",
                        "manifest_path": str(core / "crates/core/Cargo.toml"),
                    }
                ]
            }
            with (
                patch.object(bindings, "ROOT", vendor),
                patch.object(
                    bindings.subprocess, "check_output", return_value=json.dumps(metadata)
                ) as command,
            ):
                self.assertEqual(bindings.core_root({}), core.resolve())
            self.assertEqual(
                command.call_args.args[0],
                ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"],
            )


if __name__ == "__main__":
    unittest.main()
