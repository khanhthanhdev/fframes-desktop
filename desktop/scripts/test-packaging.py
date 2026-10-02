#!/usr/bin/env python3
"""Regression checks for artifact integrity and qualification claims."""
import importlib.util
import json
from pathlib import Path
import tempfile
import tomllib
import unittest


def load(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


assembly = load("assemble-phase-zero-sdk")
packaging = load("package-phase-zero")
qualification = load("validate-qualification")


class ArtifactTests(unittest.TestCase):
    def test_bundled_ffmpeg_links_supplied_install_without_download_feature(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "fframes-media").mkdir()
            manifest = root / "fframes-media/Cargo.toml"
            manifest.write_text((assembly.ROOT / "fframes-media/Cargo.toml").read_text())
            assembly.configure_bundled_ffmpeg(root)
            dependencies = tomllib.loads(manifest.read_text())["target"]
            native = dependencies['cfg(not(any(target_arch = "wasm32", windows, fframes_docsrs)))']["dependencies"]
            self.assertEqual(native["ffmpeg-sys-fframes"]["features"], ["static"])

    def test_archive_metadata_matches_bytes_and_corruption_rejects(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            source.mkdir()
            (source / "file").write_text("actual archive bytes")
            bundle = root / "bundle"
            (bundle / "artifacts").mkdir(parents=True)
            artifacts = [assembly.archive(source, bundle, name, name) for name in ["toolchain", "ffmpeg", "framework"]]
            (bundle / "compatibility.json").write_text(json.dumps({"artifacts": artifacts}))
            packaging.verify_sdk(bundle)
            (bundle / artifacts[0]["url"].removeprefix("file://")).write_bytes(b"corrupted")
            with self.assertRaises(ValueError):
                packaging.verify_sdk(bundle)

    def test_host_ffmpeg_cannot_be_mislabeled_as_version_nine(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "include/libavutil").mkdir(parents=True)
            (root / "include/libavutil/ffversion.h").write_text('#define FFMPEG_VERSION "6.1.1"')
            with self.assertRaisesRegex(ValueError, "found 6.1.1"):
                assembly.validate_ffmpeg(root, "9.0.0")

    def test_standalone_workspace_serializer_preserves_nested_lints(self):
        workspace = {"members": ["fframes"], "lints": {"rust": {"unexpected_cfgs": {"level": "warn", "check-cfg": ["cfg(example)"]}}}}
        document = "[workspace]\n" + "\n".join(f"{key} = {assembly.toml_value(value)}" for key, value in workspace.items())
        self.assertEqual(tomllib.loads(document)["workspace"], workspace)

    def test_qualification_pass_requires_real_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "results.json"
            record = json.loads((assembly.ROOT / "desktop/qualification/m0-results.json").read_text())
            # The repository ledger can acquire real evidence; this fixture
            # starts unqualified independently of its current native results.
            for name, gate in record["gates"].items():
                if name == "acp_task":
                    gate["status"] = "NOT_RUN"
                else:
                    gate["passed"] = False
                gate.pop("evidence", None)
            record["target_platform"]["status"] = "PENDING"
            path.write_text(json.dumps(record))
            qualification.validate(path)
            record["gates"]["gpui_startup"]["passed"] = True
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "without evidence"):
                qualification.validate(path)


if __name__ == "__main__":
    unittest.main()
