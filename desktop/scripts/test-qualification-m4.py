#!/usr/bin/env python3
"""Regression tests for the M4 qualification ledger validator."""
import copy
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VALIDATOR_PATH = ROOT / "scripts" / "validate-qualification.py"
spec = importlib.util.spec_from_file_location("validate_qualification", VALIDATOR_PATH)
validator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validator)


class M4QualificationTests(unittest.TestCase):
    def setUp(self):
        self.record = json.loads((ROOT / "qualification" / "m4-results.json").read_text())
        for gate in self.record["gates"].values():
            if gate["status"] == "pass":
                gate["status"] = "not_run"
                gate["evidence"] = []
                gate["prerequisite"] = "This isolated validator fixture does not exercise this gate."
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "evidence").mkdir()

    def tearDown(self):
        self.temp.cleanup()

    def write_record(self, record):
        path = self.root / "m4-results.json"
        path.write_text(json.dumps(record))
        return path

    def add_evidence(self, record, gate_name, evidence, relative="evidence/proof.json"):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(evidence))
        record["gates"][gate_name]["status"] = "pass"
        record["gates"][gate_name].pop("prerequisite", None)
        record["gates"][gate_name]["evidence"] = [
            {"path": relative, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        ]
        return path

    def test_initial_ledger_validates_without_claiming_completion(self):
        validator.validate(ROOT / "qualification" / "m4-results.json")
        self.assertEqual(self.record["acceptance"]["m4_complete"], "not_run")

    def test_development_pass_requires_a_matching_hashed_success_record(self):
        record = copy.deepcopy(self.record)
        proof = {
            "schema": "m4-development/1",
            "evidence_kind": "development",
            "gate": "dev_scoped_task_context",
            "result": "pass",
            "command": "cargo test focused-scope-tests",
            "exit_code": 0,
            "fixture_only": True,
        }
        evidence_path = self.add_evidence(record, "dev_scoped_task_context", proof)
        ledger = self.write_record(record)
        validator.validate(ledger)

        record["gates"]["dev_scoped_task_context"]["evidence"][0]["sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "Evidence changed"):
            validator.validate(self.write_record(record))
        self.assertTrue(evidence_path.is_file())

    def test_authentic_pass_rejects_fixture_evidence(self):
        record = copy.deepcopy(self.record)
        record["acceptance"]["m4_complete"] = "not_run"
        proof = {
            "schema": "m4-authentic/1",
            "evidence_kind": "authentic",
            "gate": "auth_provider_scoped_edit",
            "result": "pass",
            "fixture_only": True,
            "adapter": {"name": "scripted-agent", "protocol_version": 1, "launch_identity": "a" * 64},
            "platform": {"system": "Linux", "arch": "x86_64"},
            "cleanup_empty": True,
        }
        self.add_evidence(record, "auth_provider_scoped_edit", proof)
        with self.assertRaisesRegex(ValueError, "Authentic pass"):
            validator.validate(self.write_record(record))

    def test_credentials_in_evidence_are_rejected(self):
        record = copy.deepcopy(self.record)
        evidence = {
            "schema": "m4-development/1",
            "evidence_kind": "development",
            "gate": "dev_scoped_task_context",
            "result": "pass",
            "command": "cargo test focused-scope-tests",
            "exit_code": 0,
            "fixture_only": True,
            "token": "sk-" + "A" * 24,
        }
        self.add_evidence(record, "dev_scoped_task_context", evidence)
        with self.assertRaisesRegex(ValueError, "credential-like text"):
            validator.validate(self.write_record(record))


if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(M4QualificationTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    raise SystemExit(not result.wasSuccessful())
