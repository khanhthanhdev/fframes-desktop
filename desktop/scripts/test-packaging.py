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
    def m2_record(self):
        record = json.loads((assembly.ROOT / "desktop/qualification/m2-results.json").read_text())
        for gate in record["gates"].values():
            gate["status"] = "NOT_RUN"
            gate.pop("evidence", None)
        record["metrics"] = {}
        record["target_platform"]["status"] = "PENDING"
        record["environment"].update(clock_source="TEST_ONLY", physical_audio_evidence=False, native_platform_evidence=False)
        return record

    def validate_temp_record(self, root, record):
        path = Path(root) / "m2-results.json"
        path.write_text(json.dumps(record))
        return qualification.validate(path)

    def test_legacy_sdk_manifest_has_stable_digest_and_no_preview_fields(self):
        raw = json.loads((assembly.ROOT / "desktop/packaging/sdk/phase-zero-sdk.json").read_text())
        self.assertNotIn("preview_contract_versions", raw)
        canonical = json.dumps(raw, separators=(",", ":"))
        self.assertEqual(__import__("hashlib").sha256(canonical.encode()).hexdigest(), "105670909607f0b6043ddaee5e2b98a9f2b2614271831a5cbff804b3983e764b")

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
            (root / "include/libavutil/ffversion.h").write_text('#define FFMPEG_VERSION "n9.0.2-22-g46d8f462ee-20261001"')
            for lib in ["libavcodec", "libavformat", "libavutil", "libswscale", "libswresample"]:
                (root / "include" / lib).mkdir(parents=True, exist_ok=True)
                (root / "lib").mkdir(parents=True, exist_ok=True)
                (root / "lib" / f"{lib}.lib").write_text("stub")
            self.assertEqual(assembly.validate_ffmpeg(root, "9.0.0"), "n9.0.2-22-g46d8f462ee-20261001")

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

    def test_legacy_m0_record_without_discriminator_remains_valid(self):
        record = qualification.validate(assembly.ROOT / "desktop/qualification/m0-results.json")
        self.assertNotIn("kind", record)

    def test_unknown_explicit_qualification_kind_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            record = self.m2_record()
            record["kind"] = "m02"
            with self.assertRaisesRegex(ValueError, "Unsupported qualification kind"):
                self.validate_temp_record(temp, record)

    def test_m2_requires_exact_gates_criteria_and_pass_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            record = self.m2_record()
            del record["gates"]["cleanup"]
            with self.assertRaisesRegex(ValueError, "Invalid M2 gates"):
                self.validate_temp_record(temp, record)
            record = self.m2_record()
            record["gates"]["cleanup"]["criteria"] = ""
            with self.assertRaisesRegex(ValueError, "Missing qualification criteria"):
                self.validate_temp_record(temp, record)
            record = self.m2_record()
            record["gates"]["cleanup"]["status"] = "PASSED"
            with self.assertRaisesRegex(ValueError, "without evidence"):
                self.validate_temp_record(temp, record)

    def test_m2_rejects_missing_and_changed_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record = self.m2_record()
            gate = record["gates"]["cleanup"]
            gate["status"] = "PASSED"
            gate["evidence"] = [{"path": "proof.txt", "sha256": "0" * 64}]
            with self.assertRaisesRegex(ValueError, "Missing qualification evidence"):
                self.validate_temp_record(root, record)
            (root / "proof.txt").write_text("changed")
            with self.assertRaisesRegex(ValueError, "Evidence changed"):
                self.validate_temp_record(root, record)

    def test_m2_rejects_boolean_string_nan_and_infinite_metrics(self):
        invalid = [True, "1", float("nan"), float("inf")]
        with tempfile.TemporaryDirectory() as temp:
            for value in invalid:
                record = self.m2_record()
                record["metrics"]["seek_p95_ms"] = {"value": value, "unit": "ms"}
                with self.subTest(value=value), self.assertRaisesRegex(ValueError, "Invalid"):
                    self.validate_temp_record(temp, record)

    def test_m2_rejects_unmet_timing_and_resource_measurements(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            proof = root / "proof.txt"
            proof.write_text("measured")
            digest = __import__("hashlib").sha256(proof.read_bytes()).hexdigest()
            record = self.m2_record()
            record["gates"]["resource_stress"].update(
                status="PASSED", evidence=[{"path": proof.name, "sha256": digest}]
            )
            record["metrics"] = {
                "session_duration_minutes": {"value": 9, "unit": "min"},
                "seek_count": {"value": 1999, "unit": "count"},
                "rebuild_count": {"value": 49, "unit": "count"},
                "seek_p95_ms": {"value": 151, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": 2.1, "unit": "MiB/min"},
                "video_frame_duration_ms": {"value": 33.34, "unit": "ms"},
                "timestamp_residual_ms": {"value": 2, "unit": "ms"},
                "av_error_ms": {"value": 36, "unit": "ms"},
            }
            with self.assertRaisesRegex(ValueError, "stress counts"):
                self.validate_temp_record(root, record)

    def test_m2_test_clock_cannot_pass_output_clock(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            proof = root / "proof.txt"
            proof.write_text("measured")
            evidence = [{"path": proof.name, "sha256": __import__("hashlib").sha256(proof.read_bytes()).hexdigest()}]
            record = self.m2_record()
            record["metrics"] = {
                "session_duration_minutes": {"value": 10, "unit": "min"},
                "seek_count": {"value": 2000, "unit": "count"},
                "rebuild_count": {"value": 50, "unit": "count"},
                "seek_p95_ms": {"value": 150, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": 2, "unit": "MiB/min"},
                "video_frame_duration_ms": {"value": 33.34, "unit": "ms"},
                "timestamp_residual_ms": {"value": 2, "unit": "ms"},
                "av_error_ms": {"value": 35.34, "unit": "ms"},
            }
            record["gates"]["output_clock"].update(status="PASSED", evidence=evidence)
            with self.assertRaisesRegex(ValueError, "physical live-output"):
                self.validate_temp_record(root, record)

    def test_virtual_resource_pass_does_not_invent_physical_timing_or_qualify_platform(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            proof = root / "proof.txt"
            proof.write_text("measured resources, not DAC timing")
            evidence = [{"path": proof.name, "sha256": __import__("hashlib").sha256(proof.read_bytes()).hexdigest()}]
            record = self.m2_record()
            record["gates"]["resource_stress"].update(status="PASSED", evidence=evidence)
            record["metrics"] = {
                "session_duration_minutes": {"value": 10, "unit": "min"},
                "seek_count": {"value": 2000, "unit": "count"},
                "rebuild_count": {"value": 50, "unit": "count"},
                "seek_p95_ms": {"value": 149, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": -0.5, "unit": "MiB/min"},
            }
            self.validate_temp_record(root, record)
            record["gates"]["output_clock"].update(status="PASSED", evidence=record["gates"]["resource_stress"]["evidence"])
            with self.assertRaisesRegex(ValueError, "Missing required M2 measurements"):
                self.validate_temp_record(root, record)
            record = self.m2_record()
            for gate in record["gates"].values():
                gate.update(status="PASSED", evidence=evidence)
            record["target_platform"]["status"] = "QUALIFIED"
            record["metrics"] = {
                "session_duration_minutes": {"value": 10, "unit": "min"},
                "seek_count": {"value": 2000, "unit": "count"},
                "rebuild_count": {"value": 50, "unit": "count"},
                "seek_p95_ms": {"value": 150, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": 2, "unit": "MiB/min"},
                "video_frame_duration_ms": {"value": 33.34, "unit": "ms"},
                "timestamp_residual_ms": {"value": 2, "unit": "ms"},
                "av_error_ms": {"value": 35.34, "unit": "ms"},
            }
            with self.assertRaisesRegex(ValueError, "physical live-output"):
                self.validate_temp_record(root, record)


if __name__ == "__main__":
    unittest.main()
