#!/usr/bin/env python3
"""Validate M7 candidate evidence and prove incomplete releases fail closed."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "desktop/qualification/m7-results.json"
RELEASE_POLICY = ROOT / "desktop/packaging/release-policy.json"
TARGETS = {
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
}
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")
GATE_KINDS = {
    "native_package": "automated",
    "guided_setup": "installation",
    "export": "installed_export",
    "signing": "signature",
    "update_rollback": "update",
    "authentic_journey": "authentic",
    "physical_devices": "physical",
    "retention": "retention",
}
STATUSES = {"pass", "fail", "blocked", "not_run"}


def fail(message):
    raise ValueError(message)


def safe_evidence_path(root, raw):
    candidate = Path(raw)
    if candidate.is_absolute() or any(part in {"", ".", ".."} for part in candidate.parts):
        fail(f"unsafe evidence path: {raw}")
    resolved_root = root.resolve()
    resolved = (root / candidate).resolve()
    if not resolved.is_relative_to(resolved_root):
        fail(f"evidence path escapes qualification root: {raw}")
    return resolved


def trusted_evidence_keys():
    policy = json.loads(RELEASE_POLICY.read_text())
    keys = policy.get("qualification", {}).get("trusted_evidence_keys", [])
    if not isinstance(keys, list):
        fail("trusted qualification evidence keys must be an array")
    by_id = {}
    for key in keys:
        if set(key) != {"key_id", "public_key_pem"} or not key["key_id"] or not key["public_key_pem"].strip():
            fail("invalid trusted qualification evidence key")
        if key["key_id"] in by_id:
            fail(f"duplicate trusted qualification evidence key: {key['key_id']}")
        by_id[key["key_id"]] = key["public_key_pem"]
    return by_id


def canonical_evidence_payload(evidence):
    signed = {key: value for key, value in evidence.items() if key != "signature"}
    return json.dumps(signed, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def verify_evidence_signature(evidence, trusted_keys):
    key_id = evidence.get("key_id")
    public_key = trusted_keys.get(key_id)
    if public_key is None:
        fail(f"qualification evidence signer is not trusted: {key_id}")
    try:
        signature = base64.b64decode(evidence["signature"], validate=True)
    except (KeyError, ValueError) as error:
        fail(f"qualification evidence signature is malformed: {error}")
    with tempfile.TemporaryDirectory(prefix="m7-evidence-verify-") as temporary:
        root = Path(temporary)
        public_key_path = root / "public.pem"
        payload_path = root / "payload.json"
        signature_path = root / "signature.bin"
        public_key_path.write_text(public_key)
        payload_path.write_bytes(canonical_evidence_payload(evidence))
        signature_path.write_bytes(signature)
        try:
            result = subprocess.run(
                [
                    "openssl",
                    "pkeyutl",
                    "-verify",
                    "-pubin",
                    "-inkey",
                    str(public_key_path),
                    "-rawin",
                    "-in",
                    str(payload_path),
                    "-sigfile",
                    str(signature_path),
                ],
                capture_output=True,
                check=False,
                text=True,
            )
        except OSError as error:
            fail(f"cannot verify qualification evidence signatures: {error}")
    if result.returncode != 0:
        fail("qualification evidence signature verification failed")


def source_worktree_digest(root):
    root = Path(root).resolve()
    # Git's raw diff reports the normalized blob identities and modes, so the digest
    # is stable across checkout line-ending and permission differences on release OSes.
    changes = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "diff",
            "--raw",
            "--abbrev=64",
            "-z",
            "HEAD",
            "--",
            ".",
            ":(exclude)desktop/qualification/m7-results.json",
            ":(exclude)desktop/qualification/evidence/**",
        ],
        check=True,
        capture_output=True,
    ).stdout
    digest = hashlib.sha256(b"fframes-m7-source-worktree/1\0" + changes)

    untracked = subprocess.run(
        ["git", "-C", str(root), "ls-files", "--others", "--exclude-standard", "-z"],
        check=True,
        capture_output=True,
    ).stdout.split(b"\0")
    unexpected = [
        os.fsdecode(path)
        for path in untracked
        if path
        and not os.fsdecode(path).replace("\\", "/").startswith("desktop/qualification/evidence/")
    ]
    if unexpected:
        fail(f"source worktree contains untracked files: {', '.join(unexpected[:5])}")
    return digest.hexdigest()


def validate_source_worktree(record, root):
    source = record["source"]
    if source["worktree_digest"] is None:
        fail("release eligibility requires an exact source worktree digest")
    head = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if head.lower() != source["commit"].lower():
        fail("qualified source commit does not match the checked-out source")
    actual_digest = source_worktree_digest(root)
    if actual_digest.lower() != source["worktree_digest"].lower():
        fail("qualified source worktree digest does not match the checked-out source")


def validate(path, trusted_keys=None):
    path = Path(path)
    record = json.loads(path.read_text())
    if set(record) != {"schema_version", "source", "candidates"} or record["schema_version"] != 1:
        fail("invalid M7 ledger envelope")
    source = record["source"]
    if set(source) != {"commit", "worktree_digest"}:
        fail("invalid M7 source identity")
    if len(source["commit"]) not in {40, 64} or any(c not in "0123456789abcdefABCDEF" for c in source["commit"]):
        fail("invalid M7 source commit")
    if source["worktree_digest"] is not None and (
        len(source["worktree_digest"]) != 64
        or any(c not in "0123456789abcdefABCDEF" for c in source["worktree_digest"])
    ):
        fail("invalid M7 source worktree digest")
    trusted_keys = trusted_evidence_keys() if trusted_keys is None else trusted_keys

    if not isinstance(record["candidates"], list):
        fail("M7 candidates must be an array")
    seen = set()
    for candidate in record["candidates"]:
        if set(candidate) != {"target", "app_version", "artifact", "eligibility", "gates"}:
            fail("invalid M7 candidate fields")
        target = candidate["target"]
        if target not in TARGETS or target in seen:
            fail(f"unknown or duplicate M7 target: {target}")
        seen.add(target)
        if not isinstance(candidate["app_version"], str) or not SEMVER.fullmatch(candidate["app_version"]):
            fail(f"{target} has an invalid app version")
        gates = candidate["gates"]
        if set(gates) != set(GATE_KINDS):
            fail(f"{target} does not contain the complete M7 gate matrix")

        artifact = candidate["artifact"]
        if artifact is not None:
            if set(artifact) != {"app_sha256", "sdk_sha256", "release_manifest_sha256", "package_sha256", "source_commit", "source_worktree_digest"}:
                fail(f"{target} has invalid artifact identity")
            for name in ("app_sha256", "sdk_sha256", "release_manifest_sha256", "package_sha256", "source_worktree_digest"):
                digest = artifact[name]
                if len(digest) != 64 or any(c not in "0123456789abcdefABCDEF" for c in digest):
                    fail(f"{target} has invalid {name}")
            if artifact["source_commit"] != source["commit"]:
                fail(f"{target} artifact source commit does not match the ledger source")
            if artifact["source_worktree_digest"] != source["worktree_digest"]:
                fail(f"{target} artifact worktree digest does not match the ledger source")

        for gate_name, gate in gates.items():
            if set(gate) != {"status", "evidence", "blockers"} or gate["status"] not in STATUSES:
                fail(f"{target}.{gate_name} has an invalid status record")
            if not isinstance(gate["evidence"], list) or not isinstance(gate["blockers"], list):
                fail(f"{target}.{gate_name} evidence and blockers must be arrays")
            if any(not isinstance(blocker, str) or not blocker.strip() for blocker in gate["blockers"]):
                fail(f"{target}.{gate_name} blockers must be non-empty descriptions")
            if gate["status"] == "pass" and not gate["evidence"]:
                fail(f"{target}.{gate_name} claims a pass without hash-bound evidence")
            if gate["status"] == "pass" and gate["blockers"]:
                fail(f"{target}.{gate_name} cannot pass while listing blockers")
            if gate["status"] != "pass" and not gate["blockers"]:
                fail(f"{target}.{gate_name} non-pass status must explain its blocker")
            if gate["status"] != "pass" and gate["evidence"]:
                fail(f"{target}.{gate_name} non-pass status must not cite passing evidence")
            if gate["status"] == "pass" and artifact is None:
                fail(f"{target}.{gate_name} passes without exact candidate artifact identity")
            for evidence_ref in gate["evidence"]:
                if set(evidence_ref) != {"path", "sha256"}:
                    fail(f"{target}.{gate_name} has malformed evidence reference")
                evidence_path = safe_evidence_path(path.parent, evidence_ref["path"])
                if not evidence_path.is_file():
                    fail(f"missing M7 evidence: {evidence_path}")
                actual_hash = hashlib.sha256(evidence_path.read_bytes()).hexdigest()
                if actual_hash != evidence_ref["sha256"]:
                    fail(f"M7 evidence hash mismatch: {evidence_path}")
                try:
                    evidence = json.loads(evidence_path.read_text())
                except (UnicodeDecodeError, json.JSONDecodeError) as error:
                    fail(f"M7 evidence must be UTF-8 JSON: {error}")
                expected_evidence_fields = {
                    "schema",
                    "status",
                    "gate",
                    "target",
                    "kind",
                    "artifact",
                    "source",
                    "key_id",
                    "signature",
                }
                if not isinstance(evidence, dict) or set(evidence) != expected_evidence_fields:
                    fail(f"{target}.{gate_name} evidence has invalid signed fields")
                verify_evidence_signature(evidence, trusted_keys)
                expected_identity = {
                    "app_sha256": artifact["app_sha256"],
                    "sdk_sha256": artifact["sdk_sha256"],
                    "release_manifest_sha256": artifact["release_manifest_sha256"],
                    "package_sha256": artifact["package_sha256"],
                    "source_commit": artifact["source_commit"],
                    "source_worktree_digest": artifact["source_worktree_digest"],
                }
                if (
                    evidence.get("schema") != "m7-evidence/1"
                    or evidence.get("status") != "pass"
                    or evidence.get("gate") != gate_name
                    or evidence.get("target") != target
                    or evidence.get("kind") != GATE_KINDS[gate_name]
                    or evidence.get("artifact") != expected_identity
                    or evidence.get("source") != source
                ):
                    fail(f"{target}.{gate_name} evidence does not bind the target, gate, evidence kind and artifact")

        all_passed = all(gate["status"] == "pass" for gate in gates.values())
        if candidate["eligibility"] == "eligible":
            if not all_passed or artifact is None or source["worktree_digest"] is None:
                fail(f"{target} is eligible without every gate and exact source/artifact identities")
        elif candidate["eligibility"] not in {"blocked", "not_run"}:
            fail(f"{target} has invalid release eligibility")
        elif all_passed:
            fail(f"{target} has all gates passed but is not marked eligible")

    if seen != TARGETS:
        fail(f"M7 ledger must include all targets; missing {sorted(TARGETS - seen)}")
    return record


def verify_package(record, target, package_path):
    if target not in TARGETS:
        fail(f"unknown M7 target: {target}")
    candidate = next((item for item in record["candidates"] if item["target"] == target), None)
    if candidate is None or candidate["eligibility"] != "eligible":
        fail(f"{target} is not release-eligible")
    actual_hash = hashlib.sha256(Path(package_path).read_bytes()).hexdigest()
    if actual_hash != candidate["artifact"]["package_sha256"]:
        fail(f"{target} package bytes do not match the qualified digest")
    return target


class M7QualificationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.signing_directory = tempfile.TemporaryDirectory(prefix="m7-evidence-test-key-")
        key_root = Path(cls.signing_directory.name)
        cls.private_key = key_root / "private.pem"
        cls.public_key = key_root / "public.pem"
        subprocess.run(
            ["openssl", "genpkey", "-algorithm", "Ed25519", "-out", str(cls.private_key)],
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["openssl", "pkey", "-in", str(cls.private_key), "-pubout", "-out", str(cls.public_key)],
            check=True,
            capture_output=True,
        )
        cls.test_trusted_keys = {"test-qualification-key": cls.public_key.read_text()}

    @classmethod
    def tearDownClass(cls):
        cls.signing_directory.cleanup()

    @classmethod
    def signed_evidence(cls, evidence):
        evidence["key_id"] = "test-qualification-key"
        payload_path = Path(cls.signing_directory.name) / "payload.json"
        signature_path = Path(cls.signing_directory.name) / "signature.bin"
        payload_path.write_bytes(canonical_evidence_payload(evidence))
        subprocess.run(
            [
                "openssl",
                "pkeyutl",
                "-sign",
                "-inkey",
                str(cls.private_key),
                "-rawin",
                "-in",
                str(payload_path),
                "-out",
                str(signature_path),
            ],
            check=True,
            capture_output=True,
        )
        evidence["signature"] = base64.b64encode(signature_path.read_bytes()).decode()
        return evidence

    def test_baseline_records_all_targets_as_unqualified(self):
        record = validate(LEDGER)
        self.assertEqual({candidate["target"] for candidate in record["candidates"]}, TARGETS)
        self.assertTrue(all(candidate["eligibility"] != "eligible" for candidate in record["candidates"]))

    def test_rejects_eligibility_without_all_gates_and_artifact_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = json.loads(LEDGER.read_text())
            record["candidates"][0]["eligibility"] = "eligible"
            path = root / "m7-results.json"
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "eligible without every gate"):
                validate(path)

    def test_non_pass_gate_must_explain_its_blocker(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = json.loads(LEDGER.read_text())
            record["candidates"][0]["gates"]["native_package"]["blockers"] = []
            path = root / "m7-results.json"
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "non-pass status must explain its blocker"):
                validate(path)

    def test_rejects_changed_hash_or_wrong_target_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = json.loads(LEDGER.read_text())
            candidate = record["candidates"][0]
            candidate["artifact"] = {
                "app_sha256": "1" * 64,
                "sdk_sha256": "2" * 64,
                "release_manifest_sha256": "3" * 64,
                "package_sha256": "4" * 64,
                "source_commit": record["source"]["commit"],
                "source_worktree_digest": "5" * 64,
            }
            record["source"]["worktree_digest"] = "5" * 64
            gate_name = "native_package"
            gate = candidate["gates"][gate_name]
            gate["status"] = "pass"
            gate["blockers"] = []
            evidence = self.signed_evidence({
                "schema": "m7-evidence/1",
                "status": "pass",
                "kind": GATE_KINDS[gate_name],
                "gate": gate_name,
                "target": "x86_64-unknown-linux-gnu",
                "artifact": {
                    key: candidate["artifact"][key]
                    for key in (
                        "app_sha256",
                        "sdk_sha256",
                        "release_manifest_sha256",
                        "package_sha256",
                        "source_commit",
                        "source_worktree_digest",
                    )
                },
                "source": record["source"],
            })
            evidence_path = root / "evidence.json"
            evidence_path.write_text(json.dumps(evidence))
            gate["evidence"] = [{"path": "evidence.json", "sha256": hashlib.sha256(evidence_path.read_bytes()).hexdigest()}]
            path = root / "m7-results.json"
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "does not bind the target"):
                validate(path, self.test_trusted_keys)

    def test_unsigned_or_untrusted_pass_evidence_cannot_satisfy_a_gate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = json.loads(LEDGER.read_text())
            record["source"]["worktree_digest"] = "5" * 64
            candidate = record["candidates"][0]
            candidate["artifact"] = {
                "app_sha256": "1" * 64,
                "sdk_sha256": "2" * 64,
                "release_manifest_sha256": "3" * 64,
                "package_sha256": "4" * 64,
                "source_commit": record["source"]["commit"],
                "source_worktree_digest": record["source"]["worktree_digest"],
            }
            gate_name = "native_package"
            candidate["gates"][gate_name].update(status="pass", blockers=[])
            evidence = self.signed_evidence({
                "schema": "m7-evidence/1",
                "status": "pass",
                "kind": GATE_KINDS[gate_name],
                "gate": gate_name,
                "target": candidate["target"],
                "artifact": {
                    key: candidate["artifact"][key]
                    for key in (
                        "app_sha256",
                        "sdk_sha256",
                        "release_manifest_sha256",
                        "package_sha256",
                        "source_commit",
                        "source_worktree_digest",
                    )
                },
                "source": record["source"],
            })
            evidence_path = root / "evidence.json"
            evidence_path.write_text(json.dumps(evidence))
            candidate["gates"][gate_name]["evidence"] = [{
                "path": "evidence.json",
                "sha256": hashlib.sha256(evidence_path.read_bytes()).hexdigest(),
            }]
            path = root / "m7-results.json"
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "signer is not trusted"):
                validate(path, {})

    def test_signed_evidence_is_bound_to_exact_artifact_and_worktree(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = json.loads(LEDGER.read_text())
            record["source"]["worktree_digest"] = "5" * 64
            candidate = record["candidates"][0]
            candidate["artifact"] = {
                "app_sha256": "1" * 64,
                "sdk_sha256": "2" * 64,
                "release_manifest_sha256": "3" * 64,
                "package_sha256": "4" * 64,
                "source_commit": record["source"]["commit"],
                "source_worktree_digest": record["source"]["worktree_digest"],
            }
            gate_name = "native_package"
            candidate["gates"][gate_name].update(status="pass", blockers=[])
            evidence = self.signed_evidence({
                "schema": "m7-evidence/1",
                "status": "pass",
                "kind": GATE_KINDS[gate_name],
                "gate": gate_name,
                "target": candidate["target"],
                "artifact": {
                    key: candidate["artifact"][key]
                    for key in (
                        "app_sha256",
                        "sdk_sha256",
                        "release_manifest_sha256",
                        "package_sha256",
                        "source_commit",
                        "source_worktree_digest",
                    )
                },
                "source": record["source"],
            })
            evidence_path = root / "evidence.json"
            evidence_path.write_text(json.dumps(evidence))
            candidate["gates"][gate_name]["evidence"] = [{
                "path": "evidence.json",
                "sha256": hashlib.sha256(evidence_path.read_bytes()).hexdigest(),
            }]
            path = root / "m7-results.json"
            path.write_text(json.dumps(record))
            validated = validate(path, self.test_trusted_keys)
            self.assertEqual(validated["candidates"][0]["gates"][gate_name]["status"], "pass")
            tampered = dict(evidence)
            tampered["target"] = next(target for target in TARGETS if target != evidence["target"])
            with self.assertRaisesRegex(ValueError, "signature verification failed"):
                verify_evidence_signature(tampered, self.test_trusted_keys)
            unsigned = dict(evidence)
            unsigned.pop("signature")
            with self.assertRaisesRegex(ValueError, "signature is malformed"):
                verify_evidence_signature(unsigned, self.test_trusted_keys)

    def test_worktree_digest_changes_with_tracked_source_and_rejects_untracked_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "src.rs"
            source.write_text("fn main() {}\n")
            subprocess.run(["git", "init", str(root)], check=True, capture_output=True)
            subprocess.run(["git", "-C", str(root), "config", "user.email", "test@example.invalid"], check=True)
            subprocess.run(["git", "-C", str(root), "config", "user.name", "M7 Test"], check=True)
            subprocess.run(["git", "-C", str(root), "add", "src.rs"], check=True)
            subprocess.run(["git", "-C", str(root), "commit", "-m", "test source"], check=True, capture_output=True)
            original = source_worktree_digest(root)
            source.write_text("fn main() { println!(\"changed\"); }\n")
            changed = source_worktree_digest(root)
            self.assertNotEqual(original, changed)
            head = subprocess.run(
                ["git", "-C", str(root), "rev-parse", "HEAD"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            source_identity = {"source": {"commit": head, "worktree_digest": changed}}
            validate_source_worktree(source_identity, root)
            source_identity["source"]["worktree_digest"] = original
            with self.assertRaisesRegex(ValueError, "digest does not match"):
                validate_source_worktree(source_identity, root)
            (root / "untracked.rs").write_text("fn main() {}\n")
            with self.assertRaisesRegex(ValueError, "untracked files"):
                source_worktree_digest(root)

    def test_rejects_evidence_path_escape_and_replay_of_changed_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = json.loads(LEDGER.read_text())
            gate = record["candidates"][0]["gates"]["native_package"]
            gate.update(status="pass", evidence=[{"path": "../outside.json", "sha256": "0" * 64}], blockers=[])
            record["candidates"][0]["artifact"] = {
                "app_sha256": "1" * 64,
                "sdk_sha256": "2" * 64,
                "release_manifest_sha256": "3" * 64,
                "package_sha256": "4" * 64,
                "source_commit": record["source"]["commit"],
                "source_worktree_digest": "5" * 64,
            }
            record["source"]["worktree_digest"] = "5" * 64
            path = root / "m7-results.json"
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "unsafe evidence path"):
                validate(path)

    def test_package_verification_requires_an_eligible_target_and_matching_bytes(self):
        package = b"qualified native package"
        digest = hashlib.sha256(package).hexdigest()
        target = "x86_64-unknown-linux-gnu"
        record = {
            "candidates": [
                {
                    "target": target,
                    "eligibility": "eligible",
                    "artifact": {"package_sha256": digest},
                }
            ]
        }
        with tempfile.TemporaryDirectory() as temporary:
            package_path = Path(temporary) / "studio.zip"
            package_path.write_bytes(package)
            self.assertEqual(verify_package(record, target, package_path), target)
            package_path.write_bytes(b"changed package")
            with self.assertRaisesRegex(ValueError, "do not match the qualified digest"):
                verify_package(record, target, package_path)
            with self.assertRaisesRegex(ValueError, "unknown M7 target"):
                verify_package(record, "unqualified-target", package_path)

    def test_consumer_release_requires_opt_in_provenance_and_immutable_create(self):
        workflow = (ROOT / ".github/workflows/desktop-release.yml").read_text()
        publish_job = workflow.split("\n  publish:", maxsplit=1)[1]
        publication = json.loads((ROOT / "desktop/packaging/release-policy.json").read_text())["publication"]
        self.assertIn('default: false', workflow.split("consumer_release:", maxsplit=1)[1].split("\n\n", maxsplit=1)[0])
        self.assertIn("github.event_name == 'workflow_dispatch' && inputs.consumer_release == true", publish_job)
        self.assertIn("needs.matrix.result == 'failure'", publish_job.split("\n    runs-on:", maxsplit=1)[0])
        self.assertFalse(publication["consumer_release_enabled"])
        self.assertFalse(publication["trusted_release_keys_configured"])
        self.assertIn("trusted_release_keys_configured", publish_job)
        self.assertIn("--eligible-targets", publish_job)
        self.assertIn("--eligible-targets --verify-worktree", publish_job)
        self.assertNotIn("> eligible-targets.json", publish_job)
        self.assertIn("--verify-package", publish_job)
        self.assertIn('"$tag_commit" != "$ledger_commit"', publish_job)
        self.assertIn('"$run_commit" != "$tag_commit"', publish_job)
        self.assertNotIn("--clobber", publish_job)
        self.assertNotIn("gh release edit", publish_job)
        self.assertIn("gh release create", publish_job)

    def test_windows_ffmpeg_dependency_is_immutable_and_hash_pinned_in_ci(self):
        workflows = [
            ROOT / ".github/workflows/desktop.yml",
            ROOT / ".github/workflows/desktop-phase-zero.yml",
            ROOT / ".github/workflows/desktop-release.yml",
            ROOT / ".github/workflows/main.yml",
        ]
        for path in workflows:
            text = path.read_text()
            with self.subTest(workflow=path.name):
                self.assertNotIn("releases/download/latest/", text)
                self.assertIn("124799f0643a75eb84a64a589f1abac8f81732587942f3f34458688036153f0d", text)
                self.assertIn("autobuild-2026-10-07-13-07", text)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ledger", nargs="?", type=Path, default=LEDGER)
    parser.add_argument("--require-eligible", choices=sorted(TARGETS))
    parser.add_argument("--eligible-targets", action="store_true")
    parser.add_argument("--verify-package", nargs=2, metavar=("TARGET", "FILE"))
    parser.add_argument("--verify-worktree", action="store_true")
    arguments = parser.parse_args()
    try:
        if arguments.verify_package:
            record = validate(arguments.ledger)
            if arguments.verify_worktree:
                validate_source_worktree(record, ROOT)
            target, package_path = arguments.verify_package
            print(f"package verified: {verify_package(record, target, package_path)}")
        elif arguments.require_eligible or arguments.eligible_targets:
            record = validate(arguments.ledger)
            if arguments.verify_worktree:
                validate_source_worktree(record, ROOT)
            eligible = sorted(
                item["target"] for item in record["candidates"] if item["eligibility"] == "eligible"
            )
            if arguments.require_eligible:
                if arguments.require_eligible not in eligible:
                    fail(f"{arguments.require_eligible} is not release-eligible")
                print(f"eligible: {arguments.require_eligible}")
            else:
                print(json.dumps(eligible))
        else:
            suite = unittest.defaultTestLoader.loadTestsFromTestCase(M7QualificationTests)
            result = unittest.TextTestRunner(verbosity=2).run(suite)
            raise SystemExit(0 if result.wasSuccessful() else 1)
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise SystemExit(f"M7 qualification rejected: {error}") from error
