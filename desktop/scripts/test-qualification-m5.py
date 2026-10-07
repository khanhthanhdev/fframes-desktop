#!/usr/bin/env python3
"""False-pass tests for the M5 qualification ledger validator."""
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("validate-qualification.py")
SPEC = importlib.util.spec_from_file_location("validate_qualification_m5", SCRIPT)
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)
TEMPLATE = SCRIPT.parents[1] / "qualification" / "m5-results.json"


class M5QualificationValidationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / "evidence").mkdir()
        self.ledger_path = self.root / "m5-results.json"
        self.record = json.loads(TEMPLATE.read_text())
        for gate in self.record["gates"].values():
            gate["status"] = "not_run"
            gate["evidence"] = []
            gate["prerequisite"] = "Qualification evidence has not been recorded for this gate."
        self.record["acceptance"].update(
            development="not_run",
            full="not_run",
            reason="M5 qualification evidence has not been recorded.",
        )

    def tearDown(self):
        self.directory.cleanup()

    def write_record(self):
        self.ledger_path.write_text(json.dumps(self.record))

    def cite_json(self, relative, value):
        path = self.root / "evidence" / relative
        path.write_text(json.dumps(value))
        return {
            "path": f"evidence/{relative}",
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }

    def development_proof(self, gate, tests=None, **overrides):
        proof = {
            "schema": "m5-development/1",
            "evidence_kind": "development",
            "fixture_only": True,
            "gate": gate,
            "result": "pass",
            "exit_code": 0,
            "command": "cargo test --locked --test m5_contract",
            "tests": list(tests or VALIDATOR.M5_REQUIRED_TESTS[gate]),
            "passed": 1,
            "failed": 0,
        }
        proof.update(overrides)
        return proof

    def authentic_proof(self, gate, system):
        return {
            "schema": "m5-authentic/1",
            "evidence_kind": "authentic",
            "fixture_only": False,
            "gate": gate,
            "result": "pass",
            "adapter": {
                "name": "authenticated-provider",
                "protocol_version": 1,
                "launch_identity": "a" * 64,
                "fixture_detected": False,
            },
            "platform": {"system": system, "arch": "x86_64"},
            "cleanup_empty": True,
            "measurements": {
                "selection": {
                    "semantic_object_selected": True,
                    "displayed_frame_identity_matched": True,
                },
                "source_anchor": {
                    "anchor_resolved": True,
                    "source_hash_verified": True,
                },
            },
        }

    def resource_measurement(self):
        return {
            "schema": "m5-resource-measurements/1",
            "fixture_only": True,
            "rustc_version": "rustc 1.98.1",
            "sdk_id": self.record["environment"]["sdk_id"],
            "sdk_manifest_sha256": self.record["environment"]["sdk_manifest_sha256"],
            "source": {
                "file_count": 256,
                "indexed_bytes": 5808,
                "build_micros": 4000,
                "lookup_samples": 50,
                "lookup_p95_micros": 1,
            },
            "frame": {
                "width": 1920,
                "height": 1080,
                "object_count": 3780,
                "serialized_metadata_bytes": 1_040_000,
                "serialized_bytes_at_object_limit": 1_130_000,
                "objects_excluded_by_metadata_byte_limit": 316,
                "metadata_budget_bytes": 1024 * 1024 - 4096,
                "hit_test_samples": 30,
                "hit_test_p95_micros": 7000,
            },
            "limits": {"source_files": 256, "source_bytes": 8 * 1024 * 1024, "frame_objects": 4096},
        }

    def mark_pass(self, gate, proof, extra_files=()):
        self.record["gates"][gate]["status"] = "pass"
        self.record["gates"][gate].pop("prerequisite", None)
        self.record["gates"][gate]["notes"] = "Pass evidence is recorded."
        self.record["gates"][gate]["evidence"] = [self.cite_json(f"{gate}.json", proof), *extra_files]

    def test_pending_ledger_validates_without_promoting_pending_gates(self):
        self.write_record()
        result = VALIDATOR.validate(self.ledger_path)
        self.assertEqual(result["acceptance"]["development"], "not_run")
        self.assertEqual(result["acceptance"]["full"], "not_run")
        self.assertTrue(result["acceptance"]["reason"])

    def test_pass_without_hash_bound_structured_evidence_is_rejected(self):
        gate = "dev_displayed_canvas_selection"
        self.record["gates"][gate]["status"] = "pass"
        self.record["gates"][gate].pop("prerequisite", None)
        self.write_record()
        with self.assertRaisesRegex(ValueError, "without evidence"):
            VALIDATOR.validate(self.ledger_path)

    def test_test_proof_must_name_the_required_m5_behavior(self):
        gate = "dev_displayed_canvas_selection"
        self.mark_pass(gate, self.development_proof(gate, tests=["irrelevant_test_passed"]))
        self.write_record()
        with self.assertRaisesRegex(ValueError, "incomplete or not bound"):
            VALIDATOR.validate(self.ledger_path)

    def test_failed_test_cannot_be_serialized_as_a_pass(self):
        gate = "dev_source_retrieval_and_tools"
        proof = self.development_proof(gate, failed=1)
        self.mark_pass(gate, proof)
        self.write_record()
        with self.assertRaisesRegex(ValueError, "incomplete or not bound"):
            VALIDATOR.validate(self.ledger_path)

    def test_sdk_title_pass_is_bound_to_the_recorded_manifest(self):
        gate = "dev_managed_sdk_title_lookup"
        proof = self.development_proof(
            gate,
            sdk_id="different-sdk",
            sdk_manifest_sha256="a" * 64,
        )
        self.mark_pass(gate, proof)
        self.write_record()
        with self.assertRaisesRegex(ValueError, "not bound to the recorded manifest"):
            VALIDATOR.validate(self.ledger_path)

    def test_managed_title_pass_requires_frame_inspect_and_strip_artifacts(self):
        gate = "dev_managed_sdk_title_lookup"
        proof = self.development_proof(
            gate,
            sdk_id=self.record["environment"]["sdk_id"],
            sdk_manifest_sha256=self.record["environment"]["sdk_manifest_sha256"],
        )
        self.mark_pass(gate, proof)
        self.write_record()
        with self.assertRaisesRegex(ValueError, "must cite frame, inspect and strip artifacts"):
            VALIDATOR.validate(self.ledger_path)

    def test_resource_pass_requires_sdk_bound_bounded_measurements(self):
        gate = "dev_resource_bounds_and_cleanup"
        proof = self.development_proof(
            gate,
            sdk_id=self.record["environment"]["sdk_id"],
            sdk_manifest_sha256=self.record["environment"]["sdk_manifest_sha256"],
        )
        measurement = self.resource_measurement()
        measurement["frame"]["metadata_budget_bytes"] = 2 * 1024 * 1024
        extra = self.cite_json("resource-measurements.json", measurement)
        self.mark_pass(gate, proof, [extra])
        self.write_record()
        with self.assertRaisesRegex(ValueError, "measurements are missing, unbounded"):
            VALIDATOR.validate(self.ledger_path)

    def test_authentic_fixture_peer_cannot_pass_a_provider_gate(self):
        gate = "auth_provider_title_edit_undo_reopen"
        proof = {
            "schema": "m5-authentic/1",
            "evidence_kind": "authentic",
            "fixture_only": True,
            "gate": gate,
            "result": "pass",
            "adapter": {
                "name": "scripted-agent",
                "protocol_version": 1,
                "launch_identity": "a" * 64,
                "fixture_detected": True,
            },
            "platform": {"system": "Linux", "arch": "x86_64"},
            "cleanup_empty": True,
            "transaction": [
                {"name": name, "passed": True}
                for name in ("prompt", "candidate_validation", "apply", "undo", "reopen")
            ],
        }
        self.mark_pass(gate, proof)
        self.write_record()
        with self.assertRaisesRegex(ValueError, "lacks real adapter"):
            VALIDATOR.validate(self.ledger_path)

    def test_windows_and_macos_passes_require_selection_and_source_anchor_measurements(self):
        for gate, system in (("auth_windows", "Windows"), ("auth_macos", "Darwin")):
            proof = self.authentic_proof(gate, system)
            proof.pop("measurements")
            with self.subTest(gate=gate), self.assertRaisesRegex(
                ValueError, "lacks semantic selection and source-anchor evidence"
            ):
                VALIDATOR._m5_authentic_contract(gate, proof)

    def test_physical_input_pass_requires_selection_and_source_anchor_measurements(self):
        gate = "auth_physical_display_input"
        proof = self.authentic_proof(gate, "Linux")
        proof["measurements"].update(
            physical_device=True,
            display_input_exercised=True,
        )
        del proof["measurements"]["source_anchor"]
        with self.assertRaisesRegex(ValueError, "lacks semantic selection and source-anchor evidence"):
            VALIDATOR._m5_authentic_contract(gate, proof)

    def test_complete_selection_and_anchor_measurements_satisfy_authentic_platform_gates(self):
        for gate, system in (
            ("auth_physical_display_input", "Linux"),
            ("auth_windows", "Windows"),
            ("auth_macos", "Darwin"),
        ):
            proof = self.authentic_proof(gate, system)
            if gate == "auth_physical_display_input":
                proof["measurements"].update(
                    physical_device=True,
                    display_input_exercised=True,
                )
            VALIDATOR._m5_authentic_contract(gate, proof)

    def test_development_acceptance_cannot_overstate_incomplete_gates(self):
        self.record["acceptance"]["development"] = "pass"
        self.write_record()
        with self.assertRaisesRegex(ValueError, "exactly when every development gate passes"):
            VALIDATOR.validate(self.ledger_path)

    def test_evidence_hash_tampering_is_rejected(self):
        gate = "dev_displayed_canvas_selection"
        evidence = self.cite_json("proof.json", self.development_proof(gate))
        evidence["sha256"] = "0" * 64
        self.record["gates"][gate]["evidence"] = [evidence]
        self.write_record()
        with self.assertRaisesRegex(ValueError, "Evidence changed"):
            VALIDATOR.validate(self.ledger_path)


if __name__ == "__main__":
    unittest.main(verbosity=2)
