#!/usr/bin/env python3
"""Negative evidence and ranking contract tests for M6 provider qualification.

Verifies that validate-qualification.py strictly refuses:
- Fake authentic passes
- Fixture agent claims of authentic qualification
- Unrun/blocked gates with evidence or missing prerequisites
- Tampered evidence hashes
- Ineligible or inflated recommendations (< 2 qualified providers)
- Recommendations of unqualified providers
- M6 claims of native app export UI (reserved for M7)
- Credential-like text and private paths in evidence
- Shared evidence files cited across multiple gates
- Unclean teardown in authentic evidence
- Platform mismatch between evidence and gate
"""

import copy
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS_DIR = ROOT / "desktop" / "scripts"
sys.path.insert(0, str(SCRIPTS_DIR))

import importlib
import importlib.util
validate_mod = importlib.import_module("validate-qualification")
validate_m6 = validate_mod.validate_m6
M6_LEDGER_PATH = ROOT / "desktop" / "qualification" / "m6-results.json"
M6_RUNNER_PATH = ROOT / "desktop" / "scripts" / "qualify-m6-providers.py"


def load_baseline():
    return json.loads(M6_LEDGER_PATH.read_text())


def test_runner_keeps_development_and_authentic_separate():
    before = hashlib.sha256(M6_LEDGER_PATH.read_bytes()).hexdigest()
    help_result = subprocess.run(
        [sys.executable, str(M6_RUNNER_PATH), "--help"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    assert "--mode {development,authentic}" in help_result.stdout
    assert "--provider" in help_result.stdout

    evidence_root = M6_LEDGER_PATH.parent / "evidence"
    with tempfile.TemporaryDirectory(prefix="m6-runner-test-", dir=evidence_root) as temporary:
        output_dir = Path(temporary) / "authentic-readiness"
        subprocess.run(
            [sys.executable, str(M6_RUNNER_PATH), "--mode", "authentic", "--out", str(output_dir)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        report = json.loads((output_dir / "run.json").read_text())
        assert report["ledger_updated"] is False
        assert report["authentic_status"] == "not_run"
        assert set(report["providers"]) == {"claude", "codex", "pi", "antigravity"}
        assert all(item["status"] in {"blocked", "not_run"} for item in report["providers"].values())

    after = hashlib.sha256(M6_LEDGER_PATH.read_bytes()).hexdigest()
    assert after == before, "Authentic readiness inventory must not alter the provider ledger"
    print("PASS: M6 runner keeps fixture development checks separate from authentic provider readiness")


def test_runner_timeout_reaps_owned_process_group():
    spec = importlib.util.spec_from_file_location("qualify_m6_providers", M6_RUNNER_PATH)
    assert spec is not None and spec.loader is not None
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    evidence_root = M6_LEDGER_PATH.parent / "evidence"
    with tempfile.TemporaryDirectory(prefix="m6-runner-timeout-", dir=evidence_root) as temporary:
        result = runner.run_check(
            "timeout",
            [[sys.executable, "-c", "import time; time.sleep(60)"]],
            Path(temporary),
            timeout=1,
        )
    assert result["status"] == "fail"
    assert result["commands"][0]["status"] == "timeout"
    assert result["commands"][0]["cleanup_verified"] is True
    print("PASS: M6 runner timeout stops its owned process group")


def test_baseline_passes():
    record = load_baseline()
    validate_m6(M6_LEDGER_PATH, record)
    print("PASS: Baseline m6-results.json validates cleanly")


def expect_failure(record, expected_substr, label, tmp_path=None):
    target = tmp_path or M6_LEDGER_PATH
    if tmp_path is not None:
        tmp_path.write_text(json.dumps(record))
    try:
        validate_m6(target, record)
    except ValueError as error:
        msg = str(error)
        if expected_substr.lower() not in msg.lower():
            raise AssertionError(
                f"{label}: Expected error containing '{expected_substr}', got '{msg}'"
            )
        print(f"PASS: {label} -> rejected with: {msg}")
        return
    raise AssertionError(f"{label}: Expected failure with '{expected_substr}', but validation passed!")


def test_missing_provider():
    record = load_baseline()
    del record["providers"]["claude"]
    expect_failure(record, "must contain claude", "Missing provider in matrix")


def test_pass_without_evidence():
    record = load_baseline()
    gate = record["providers"]["claude"]["gates"]["create_edit_export"]
    gate["status"] = "pass"
    del gate["prerequisite"]
    expect_failure(record, "claims a pass without evidence", "Pass claimed without evidence")


def test_pass_with_prerequisite():
    record = load_baseline()
    gate = record["providers"]["claude"]["gates"]["create_edit_export"]
    gate["status"] = "pass"
    gate["evidence"] = []
    expect_failure(record, "claims a pass without evidence", "Pass with prerequisite")


def test_unrun_without_prerequisite():
    record = load_baseline()
    gate = record["providers"]["claude"]["gates"]["create_edit_export"]
    gate["prerequisite"] = "short"
    expect_failure(record, "without prerequisite", "Unrun gate with too-short prerequisite")


def test_unrun_with_evidence():
    record = load_baseline()
    gate = record["providers"]["claude"]["gates"]["create_edit_export"]
    gate["evidence"] = [{"path": "evidence/foo.json", "sha256": "a" * 64}]
    expect_failure(record, "cites evidence", "Unrun gate citing evidence")


def test_recommend_unqualified_provider():
    record = load_baseline()
    record["ranking"]["status"] = "recommended"
    record["ranking"]["recommended"] = ["claude"]
    expect_failure(record, "cannot recommend unqualified provider", "Recommending unqualified provider")


def test_recommend_with_fewer_than_two_providers():
    record = load_baseline()
    record["providers"]["claude"]["status"] = "qualified"
    record["ranking"]["status"] = "recommended"
    record["ranking"]["recommended"] = ["claude"]
    expect_failure(record, "claims qualified status but has unmet mandatory gates", "Recommend with unmet gates")


def test_recommendation_requires_at_least_two_providers():
    record = load_baseline()
    record["ranking"]["status"] = "recommended"
    record["ranking"]["recommended"] = []
    expect_failure(record, "status must be insufficient_evidence", "Ranking status recommended with empty list")


def test_ranking_status_mismatch():
    record = load_baseline()
    record["ranking"]["status"] = "recommended"
    expect_failure(record, "status must be insufficient_evidence", "Status recommended with 0 qualified")


def test_secret_in_ledger():
    record = load_baseline()
    record["environment"]["scope"] = "sk-1234567890abcdef1234567890abcdef"
    # A directory, not an open NamedTemporaryFile: Windows cannot reopen an open temporary file.
    with tempfile.TemporaryDirectory() as directory:
        expect_failure(record, "credential-like text", "Secret in ledger file", tmp_path=Path(directory) / "ledger.json")


def test_invalid_launch_identity():
    record = load_baseline()
    record["providers"]["claude"]["launch_identity"] = "not-a-hex-digest"
    expect_failure(record, "invalid launch identity digest", "Invalid launch identity format")


def test_malformed_capability_object_is_rejected():
    record = load_baseline()
    record["providers"]["claude"]["capabilities"] = {}
    expect_failure(record, "complete boolean capability set", "Incomplete provider capabilities")


def test_provider_extra_properties_are_rejected():
    record = load_baseline()
    record["providers"]["claude"]["unreviewed_claim"] = True
    expect_failure(record, "has invalid fields", "Provider schema rejects unknown property")


def test_pass_without_launch_identity_is_rejected():
    sandbox = EvidenceTestSandbox()
    try:
        record = load_baseline()
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        rel_path, sha256 = sandbox.write_evidence("ev_missing_launch.json", default_authentic_proof())
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(
            record,
            "pass requires a launch identity",
            "Missing provider launch identity rejected",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()


def test_evidence_must_match_its_provider_and_gate():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["gate"] = "selected_context_edit"
        rel_path, sha256 = sandbox.write_evidence("ev_wrong_gate.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(
            record,
            "bound to another provider or gate",
            "Evidence gate mismatch rejected",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()


def test_authentic_acceptance_requires_two_qualified():
    record = load_baseline()
    record["acceptance"]["authentic"] = "pass"
    expect_failure(record, "authentic acceptance requires at least two qualified providers", "Authentic acceptance without 2 qualified")


# ---- Real Evidence File Negative Tests -----------------------------------------------------------

def default_authentic_proof(
    launch_identity="11" * 32,
    agent_name="claude-agent-acp",
    provider_id="claude",
):
    return {
        "evidence_kind": "authentic",
        "schema": "m6-authentic/1",
        "provider_id": provider_id,
        "gate": "interruption_and_cleanup",
        "fixture_only": False,
        "result": "pass",
        "cleanup_empty": True,
        "cleanup": {
            "owned_processes_after": 0
        },
        "platform": {
            "system": "Linux",
            "arch": "x86_64"
        },
        "adapter": {
            "name": agent_name,
            "protocol_version": 1,
            "launch_identity": launch_identity,
            "fixture_detected": False
        },
        "measurements": {
            "scenarios": [
                {"name": "edit", "escaped_descendants": 0, "group_empty_after": True},
                {"name": "stop", "escaped_descendants": 0, "group_empty_after": True},
                {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": True}
            ]
        }
    }


def bind_provider(record, provider_id="claude", agent_name="claude-agent-acp", launch_identity="11" * 32):
    provider = record["providers"][provider_id]
    provider["launch_identity"] = launch_identity
    provider["adapter_name"] = agent_name


def qualifying_measurements(gate_name, latency=120.0, rss=45000.0):
    measurements = {"environment_name": "linux-ci-runner-01"}
    measurements.update({
        "create_edit_export": {
            "create_completed": True,
            "selected_title_edit_completed": True,
            "inspect_passed": True,
            "preview_verified": True,
            "apply_undo_reopen_passed": True,
            "cli_export_passed": True,
            "export_revision_sha256": "ab" * 32,
            "latency_p95_ms": latency,
        },
        "selected_context_edit": {
            "frozen_scope_verified": True,
            "out_of_scope_unchanged": True,
        },
        "compiler_error_repair": {
            "repair_attempts": 1,
            "repair_succeeded": True,
        },
        "completion_and_streaming": {
            "stream_responsive": True,
            "follow_up_completed": True,
            "quiet_turn_quiescent": True,
        },
        "permission_and_errors": {
            "approve_handled": True,
            "decline_handled": True,
            "protocol_error_recovered": True,
        },
        "interruption_and_cleanup": {
            "scenarios": [
                {"name": "edit", "escaped_descendants": 0, "group_empty_after": True},
                {"name": "stop", "escaped_descendants": 0, "group_empty_after": True},
                {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": True},
            ],
        },
        "restoration_and_fallback": {
            "resume_same_cwd": True,
            "fresh_grants": True,
            "unsupported_fallback": True,
            "interrupted_prompt_not_replayed": True,
        },
        "tool_parity": {
            "tool_route": "mcp",
            "tool_results": {name: "pass" for name in validate_mod.M6_TOOL_NAMES},
            "tools_succeeded": 9,
        },
        "image_context": {
            "visual_verified": True,
            "image_disabled_fallback_verified": True,
        },
        "twenty_cycle_cleanup": {
            "cycles_completed": 20,
            "peak_rss_kib": rss,
        },
        "controlled_handoff": {
            "draft_retained": True,
            "source_fence_passed": True,
            "queue_transfer_explicit": True,
            "writer_overlap_count": 0,
            "post_stop_writes": 0,
        },
        "platform_linux": {"platform_workflow_passed": True},
        "platform_windows": {"platform_workflow_passed": True},
        "platform_macos": {"platform_workflow_passed": True},
        "platform_physical_device": {
            "physical_device": True,
            "display_input_exercised": True,
            "audio_device_exercised": True,
        },
    }[gate_name])
    return measurements


class EvidenceTestSandbox:
    def __init__(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.evidence_dir = self.root / "evidence"
        self.evidence_dir.mkdir(parents=True, exist_ok=True)
        src_probes = ROOT / "desktop" / "qualification" / "evidence" / "m6-probes"
        if src_probes.exists():
            shutil.copytree(src_probes, self.evidence_dir / "m6-probes")
        self.ledger_path = self.root / "m6-results.json"

    def write_evidence(self, name, content):
        path = self.evidence_dir / name
        raw = json.dumps(content).encode("utf-8")
        path.write_bytes(raw)
        sha256 = hashlib.sha256(raw).hexdigest()
        return f"evidence/{name}", sha256

    def close(self):
        self.temp_dir.cleanup()


def test_tampered_evidence_hash():
    sandbox = EvidenceTestSandbox()
    try:
        rel_path, actual_sha = sandbox.write_evidence("ev1.json", default_authentic_proof())
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": "0" * 64}]  # Tampered hash!
        expect_failure(record, "evidence changed", "Tampered evidence hash", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_fixture_agent_in_authentic_evidence():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof(agent_name="scripted-agent")  # Fixture agent!
        rel_path, sha256 = sandbox.write_evidence("ev_fixture.json", proof)
        record = load_baseline()
        bind_provider(record, agent_name="scripted-agent")
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "authentic evidence uses fixture adapter", "Fixture agent in authentic evidence", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()

def test_fixture_agent_name_field_bypass_rejected():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["adapter"] = {
            "agent_name": "scripted-agent",
            "protocol_version": 1,
            "launch_identity": "11" * 32,
            "fixture_detected": False,
        }
        rel_path, sha256 = sandbox.write_evidence("ev_fixture_bypass.json", proof)
        record = load_baseline()
        bind_provider(record, agent_name="scripted-agent")
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(
            record,
            "authentic evidence uses fixture adapter",
            "Fixture agent_name field bypass rejected",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()



def test_launch_identity_mismatch_in_evidence():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof(launch_identity="22" * 32)
        rel_path, sha256 = sandbox.write_evidence("ev_launch.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "launch identity does not match provider", "Launch identity mismatch", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_platform_mismatch_in_evidence():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["platform"]["system"] = "Darwin"  # Wrong platform for Linux run!
        rel_path, sha256 = sandbox.write_evidence("ev_plat.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "authentic evidence records wrong platform", "Platform mismatch in evidence", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_development_evidence_for_authentic_gate():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["evidence_kind"] = "development"
        proof["fixture_only"] = True
        rel_path, sha256 = sandbox.write_evidence("ev_dev.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "cites development or fixture evidence", "Development evidence for authentic gate", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_duplicate_evidence_citation():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        rel_path, sha256 = sandbox.write_evidence("ev_shared.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate1 = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate1["status"] = "pass"
        del gate1["prerequisite"]
        evidence_ref = {"path": rel_path, "sha256": sha256}
        gate1["evidence"] = [evidence_ref, evidence_ref]
        expect_failure(record, "duplicate evidence file", "Duplicate evidence citation", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_teardown_failure_in_evidence():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["cleanup"]["owned_processes_after"] = 3  # Unclean teardown!
        rel_path, sha256 = sandbox.write_evidence("ev_teardown.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "did not measure clean teardown", "Teardown failure in evidence", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_native_app_export_claim_rejected():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["gate"] = "create_edit_export"
        proof["measurements"] = qualifying_measurements("create_edit_export")
        proof["measurements"]["claimed_native_app_export"] = True  # Forbidden in M6!
        rel_path, sha256 = sandbox.write_evidence("ev_export.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["create_edit_export"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "claims native app export in m6", "Native app export claim in M6", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_credential_in_evidence_file():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["measurements"]["note"] = "sk-proj-1234567890abcdef1234567890abcdef"
        rel_path, sha256 = sandbox.write_evidence("ev_secret.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "credential-like text", "Credential in evidence file", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_private_path_in_evidence_file():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        proof["measurements"]["note"] = "/home/developer/workspace/project"
        rel_path, sha256 = sandbox.write_evidence("ev_path.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(record, "absolute private path", "Private path in evidence file", tmp_path=sandbox.ledger_path)
    finally:
        sandbox.close()


def test_development_kind_on_provider_gate_rejected():
    sandbox = EvidenceTestSandbox()
    try:
        rel_path, sha256 = sandbox.write_evidence("ev_dev_gate.json", default_authentic_proof())
        record = load_baseline()
        for prov_id in ("claude", "codex"):
            prov = record["providers"][prov_id]
            prov["status"] = "qualified"
            for gate_name in validate_mod.M6_MANDATORY_SCENARIO_GATES:
                prov["gates"][gate_name] = {
                    "kind": "development",
                    "status": "pass",
                    "criteria": "fake development pass",
                    "notes": "testing rejection",
                    "evidence": [{"path": rel_path, "sha256": sha256}],
                }
        record["ranking"]["status"] = "recommended"
        record["ranking"]["recommended"] = ["claude", "codex"]
        record["ranking"]["experimental"] = ["pi", "antigravity"]
        expect_failure(
            record,
            "must be of kind authentic (provider gates are authentic-only)",
            "Development kind on provider gates rejected",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()


def test_two_providers_fully_qualified_with_real_authentic_evidence():
    sandbox = EvidenceTestSandbox()
    try:
        record = load_baseline()
        for prov_id, agent_name, latency, rss in (
            ("claude", "claude-agent-acp", 120.0, 45000.0),
            ("codex", "codex-acp", 95.0, 42000.0),
        ):
            launch_id = "aa" * 32 if prov_id == "claude" else "bb" * 32
            prov = record["providers"][prov_id]
            prov["status"] = "qualified"
            prov["launch_identity"] = launch_id
            prov["adapter_name"] = agent_name
            for gate_name in validate_mod.M6_MANDATORY_SCENARIO_GATES:
                proof = default_authentic_proof(
                    launch_identity=launch_id,
                    agent_name=agent_name,
                    provider_id=prov_id,
                )
                proof["gate"] = gate_name
                proof["measurements"] = qualifying_measurements(gate_name, latency, rss)

                rel_path, sha256 = sandbox.write_evidence(f"{prov_id}_{gate_name}.json", proof)
                prov["gates"][gate_name] = {
                    "kind": "authentic",
                    "status": "pass",
                    "criteria": f"authentic qualification of {gate_name}",
                    "notes": "measured and verified",
                    "evidence": [{"path": rel_path, "sha256": sha256}],
                }
        record["ranking"]["status"] = "recommended"
        # Codex has lower latency (95ms vs 120ms) and lower RSS, so Codex ranks #1 ahead of Claude!
        record["ranking"]["recommended"] = ["codex", "claude"]
        record["ranking"]["experimental"] = ["pi", "antigravity"]
        record["acceptance"]["authentic"] = "pass"
        record["acceptance"]["full"] = "not_run"

        sandbox.ledger_path.write_text(json.dumps(record))
        validate_m6(sandbox.ledger_path, record)
        print("PASS: Two providers with genuine authentic evidence qualify and recommend based on evidence, not names!")

        record["ranking"]["status"] = "insufficient_evidence"
        record["ranking"]["recommended"] = []
        record["ranking"]["experimental"] = sorted(validate_mod.M6_PROVIDERS)
        expect_failure(
            record,
            "must be recommended when at least two providers qualify",
            "Insufficient ranking with two qualified providers rejected",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()


def test_writer_contract_missing_edit_scenario_rejected():
    sandbox = EvidenceTestSandbox()
    try:
        proof = default_authentic_proof()
        # Omit "edit" scenario
        proof["measurements"]["scenarios"] = [
            {"name": "stop", "escaped_descendants": 0, "group_empty_after": True},
            {"name": "provider_crash", "escaped_descendants": 0, "group_empty_after": True},
        ]
        rel_path, sha256 = sandbox.write_evidence("ev_no_edit.json", proof)
        record = load_baseline()
        bind_provider(record)
        gate = record["providers"]["claude"]["gates"]["interruption_and_cleanup"]
        gate["status"] = "pass"
        del gate["prerequisite"]
        gate["evidence"] = [{"path": rel_path, "sha256": sha256}]
        expect_failure(
            record,
            "scenario edit was not measured",
            "Writer contract missing edit scenario rejected",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()


def test_incomparable_environments_rejected_by_ranking():
    sandbox = EvidenceTestSandbox()
    try:
        record = load_baseline()
        for prov_id, agent_name, env_name in (
            ("claude", "claude-agent-acp", "linux-runner-01"),
            ("codex", "codex-acp", "linux-runner-02"),  # Different environment!
        ):
            launch_id = "aa" * 32 if prov_id == "claude" else "bb" * 32
            prov = record["providers"][prov_id]
            prov["status"] = "qualified"
            prov["launch_identity"] = launch_id
            prov["adapter_name"] = agent_name
            for gate_name in validate_mod.M6_MANDATORY_SCENARIO_GATES:
                proof = default_authentic_proof(
                    launch_identity=launch_id,
                    agent_name=agent_name,
                    provider_id=prov_id,
                )
                proof["gate"] = gate_name
                proof["measurements"] = qualifying_measurements(gate_name)
                proof["measurements"]["environment_name"] = env_name
                rel_path, sha256 = sandbox.write_evidence(f"{prov_id}_{gate_name}_incomp.json", proof)
                prov["gates"][gate_name] = {
                    "kind": "authentic",
                    "status": "pass",
                    "criteria": f"qualification of {gate_name}",
                    "notes": "verified",
                    "evidence": [{"path": rel_path, "sha256": sha256}],
                }
        record["ranking"]["status"] = "recommended"
        record["ranking"]["recommended"] = ["claude", "codex"]
        record["ranking"]["experimental"] = ["pi", "antigravity"]
        expect_failure(
            record,
            "incomparable ranking: providers measured on different environments",
            "Incomparable environments rejected by ranking",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()


def test_ranking_refuses_out_of_order_recommendation():
    sandbox = EvidenceTestSandbox()
    try:
        record = load_baseline()
        for prov_id, agent_name, latency, rss in (
            ("claude", "claude-agent-acp", 120.0, 45000.0),
            ("codex", "codex-acp", 95.0, 42000.0),
        ):
            launch_id = "aa" * 32 if prov_id == "claude" else "bb" * 32
            prov = record["providers"][prov_id]
            prov["status"] = "qualified"
            prov["launch_identity"] = launch_id
            prov["adapter_name"] = agent_name
            for gate_name in validate_mod.M6_MANDATORY_SCENARIO_GATES:
                proof = default_authentic_proof(
                    launch_identity=launch_id,
                    agent_name=agent_name,
                    provider_id=prov_id,
                )
                proof["gate"] = gate_name
                proof["measurements"] = qualifying_measurements(gate_name, latency, rss)
                rel_path, sha256 = sandbox.write_evidence(f"{prov_id}_{gate_name}_order.json", proof)
                prov["gates"][gate_name] = {
                    "kind": "authentic",
                    "status": "pass",
                    "criteria": f"qualification of {gate_name}",
                    "notes": "verified",
                    "evidence": [{"path": rel_path, "sha256": sha256}],
                }
        record["ranking"]["status"] = "recommended"
        # Wrong recommendation order: alphabetical ["claude", "codex"] instead of evidence-ranked ["codex", "claude"]!
        record["ranking"]["recommended"] = ["claude", "codex"]
        record["ranking"]["experimental"] = ["pi", "antigravity"]
        expect_failure(
            record,
            "ranking recommendation mismatch",
            "Alphabetical recommendation rejected when evidence favors codex",
            tmp_path=sandbox.ledger_path,
        )
    finally:
        sandbox.close()


def main():
    print("Running M6 qualification validator negative tests...")
    test_runner_keeps_development_and_authentic_separate()
    test_runner_timeout_reaps_owned_process_group()
    test_baseline_passes()
    test_missing_provider()
    test_pass_without_evidence()
    test_pass_with_prerequisite()
    test_unrun_without_prerequisite()
    test_unrun_with_evidence()
    test_recommend_unqualified_provider()
    test_recommend_with_fewer_than_two_providers()
    test_recommendation_requires_at_least_two_providers()
    test_ranking_status_mismatch()
    test_secret_in_ledger()
    test_invalid_launch_identity()
    test_malformed_capability_object_is_rejected()
    test_provider_extra_properties_are_rejected()
    test_pass_without_launch_identity_is_rejected()
    test_evidence_must_match_its_provider_and_gate()
    test_authentic_acceptance_requires_two_qualified()

    print("\nRunning real evidence file negative tests...")
    test_tampered_evidence_hash()
    test_fixture_agent_in_authentic_evidence()
    test_fixture_agent_name_field_bypass_rejected()
    test_launch_identity_mismatch_in_evidence()
    test_platform_mismatch_in_evidence()
    test_development_evidence_for_authentic_gate()
    test_development_kind_on_provider_gate_rejected()
    test_writer_contract_missing_edit_scenario_rejected()
    test_duplicate_evidence_citation()
    test_teardown_failure_in_evidence()
    test_native_app_export_claim_rejected()
    test_credential_in_evidence_file()
    test_private_path_in_evidence_file()
    test_incomparable_environments_rejected_by_ranking()
    test_ranking_refuses_out_of_order_recommendation()

    print("\nRunning positive qualification evidence test...")
    test_two_providers_fully_qualified_with_real_authentic_evidence()

    print("\nALL M6 qualification tests passed cleanly!")


if __name__ == "__main__":
    main()
