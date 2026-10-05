#!/usr/bin/env python3
"""Check qualification claims and referenced evidence without inventing native results.

Usage: validate-qualification.py [LEDGER ...]

With no argument the three shipped ledgers (M0, M2, M3) are validated. M3 rules
(``qualification/m3-results.json``, see ``m3-results.schema.json``): every evidence file is
hashed, nothing passes without evidence, a ``not_run``/``blocked`` gate states its missing
prerequisite, and:

* a DEVELOPMENT pass must be backed by the harness run record it cites (the check that
  produced it ran, exited 0 and passed tests) and may never cite authentic evidence;
* an AUTHENTIC pass needs a real, non-fixture adapter identity (name, protocol v1, launch
  identity) and, for every file it cites, a structured evidence record bound to that one gate
  with the gate's own measurement contract (``AUTHENTIC_CONTRACTS``): explicit
  ``fixture_only: false``, a ready probe, the measured platform, the adapter's launch
  identity and a clean teardown. A label such as ``evidence_kind: authentic`` alone is
  never enough, a record cannot be shared between gates, and a ``not_run`` authentic gate
  cites nothing;
* neither the ledger nor any evidence file may contain credential-like text (scanned
  NUL-safely; textual evidence must be plain UTF-8) or an absolute private path (home,
  workspace, temporary directory).
"""
import argparse
import hashlib
import json
import math
import re
import string
import sys
from pathlib import Path

QUALIFICATION = Path(__file__).resolve().parents[1] / "qualification"
DEFAULT_LEDGERS = [QUALIFICATION / "m0-results.json", QUALIFICATION / "m2-results.json", QUALIFICATION / "m3-results.json"]

M0_GATES = {"managed_compilation", "gpui_startup", "renderer_worker", "acp_task", "selection_anchor"}
M2_GATES = {
    "output_clock",
    "audio_modes",
    "revision_handoff",
    "native_controls",
    "pixel_parity",
    "resource_stress",
    "cleanup",
}
M2_METRICS = {
    "session_duration_minutes": "min",
    "seek_count": "count",
    "rebuild_count": "count",
    "seek_p95_ms": "ms",
    "rss_slope_mib_per_min": "MiB/min",
    "video_frame_duration_ms": "ms",
    "timestamp_residual_ms": "ms",
    "av_error_ms": "ms",
}

M3_GATES = {
    "dev_acp_transport": "development",
    "dev_engine_transactions": "development",
    "dev_workflow_fixture": "development",
    "dev_tools_and_validation": "development",
    "dev_native_ui_workflow": "development",
    "dev_native_x11_shell": "development",
    "dev_resource_bounds": "development",
    "dev_compiler_count": "development",
    "dev_cleanup_cycles": "development",
    "dev_cli_mcp_parity": "development",
    "dev_publication_primitive": "development",
    "dev_real_sdk_workflow": "development",
    "auth_adapter_probe_v1": "authentic",
    "auth_two_edit_undo_restart": "authentic",
    "auth_writer_process_group": "authentic",
    "auth_compiler_error_repair": "authentic",
    "auth_interaction_and_failure_modes": "authentic",
    "auth_mcp_cli_support": "authentic",
    "auth_twenty_cycle_cleanup": "authentic",
    "auth_visual_ime": "authentic",
    "auth_physical_audio": "authentic",
    "auth_windows": "authentic",
    "auth_macos": "authentic",
}
M3_STATUSES = {"pass", "fail", "blocked", "not_run"}
# Agent names of the repository's scripted fixtures: they can never be an authentic adapter.
FIXTURE_AGENT_NAMES = {"scripted-agent", "protocol-peer", "acp-peer", "fixture", "test-agent"}
# Credential-like text. The M3 harness redacts with exactly these before serialization and the
# validator rejects any evidence or ledger that still matches. Names never reveal the match.
SECRET_PATTERNS = [
    ("openai-style key", re.compile(r"\bsk-[A-Za-z0-9_\-]{20,}")),
    ("github token", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})")),
    ("slack token", re.compile(r"\bxox[abprs]-[A-Za-z0-9\-]{10,}")),
    ("aws access key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("google api key", re.compile(r"\bAIza[0-9A-Za-z_\-]{35}")),
    ("json web token", re.compile(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}")),
    ("private key", re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")),
    ("bearer credential", re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=\-]{16,}")),
    (
        "assigned credential",
        re.compile(
            r"(?i)\b(?:api[_-]?key|secret|passw(?:or)?d|token|authorization)\b[\"']?\s*[:=]\s*[\"']?"
            r"(?!\[REDACTED\]|null\b|true\b|false\b|none\b|<)[A-Za-z0-9._~+/=\-]{12,}"
        ),
    ),
]
MAX_SCANNED_BYTES = 16 * 1024 * 1024


def load_record(path):
    return json.loads(
        path.read_text(),
        parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f"Invalid numeric constant: {value}")),
    )


def validate_evidence(path, name, gate):
    evidence_items = gate.get("evidence")
    if not evidence_items or type(evidence_items) is not list:
        raise ValueError(f"{name} claims a pass without evidence")
    for evidence in evidence_items:
        if type(evidence) is not dict or set(evidence) != {"path", "sha256"}:
            raise ValueError(f"Invalid qualification evidence entry for {name}")
        if type(evidence["path"]) is not str or not evidence["path"]:
            raise ValueError(f"Invalid qualification evidence path for {name}")
        checksum_claim = evidence["sha256"]
        if (
            type(checksum_claim) is not str
            or len(checksum_claim) != 64
            or any(character not in string.hexdigits for character in checksum_claim)
        ):
            raise ValueError(f"Invalid qualification evidence hash for {name}")
        source = (path.parent / evidence["path"]).resolve()
        if not source.is_file():
            raise ValueError(f"Missing qualification evidence: {source}")
        with source.open("rb") as stream:
            checksum = hashlib.file_digest(stream, "sha256").hexdigest()
        if checksum != checksum_claim.lower():
            raise ValueError(f"Evidence changed: {source}")


def validate_m0(path, record):
    if record["schema_version"] != 1 or set(record["gates"]) != M0_GATES:
        raise ValueError("Invalid qualification schema or missing gates")
    all_passed = True
    for name, gate in record["gates"].items():
        if name == "acp_task":
            if gate["status"] not in {"PASSED", "FAILED", "NOT_RUN"}:
                raise ValueError("Invalid ACP qualification status")
            passed = gate["status"] == "PASSED"
        else:
            if type(gate["passed"]) is not bool:
                raise ValueError("Gate passed must be boolean")
            passed = gate["passed"]
        all_passed &= passed
        if passed:
            validate_evidence(path, name, gate)
    if record["target_platform"]["status"] not in {"QUALIFIED", "BLOCKED", "PENDING"}:
        raise ValueError("Invalid platform status")
    if record["target_platform"]["status"] == "QUALIFIED" and not all_passed:
        raise ValueError("Platform cannot qualify with unmet gates")
    for metric, value in record["metrics"].items():
        if type(value) not in {int, float} or not math.isfinite(value) or value < 0:
            raise ValueError(f"Invalid measured metric: {metric}")


def validate_m2(path, record):
    required = {"kind", "schema_version", "timestamp", "environment", "target_platform", "gates", "metrics"}
    if not required.issubset(record) or record["schema_version"] != 1:
        raise ValueError("Invalid M2 qualification schema")
    if type(record["timestamp"]) is not str or not record["timestamp"]:
        raise ValueError("Invalid M2 timestamp")
    if set(record["gates"]) != M2_GATES:
        raise ValueError("Invalid M2 gates")

    all_passed = True
    for name, gate in record["gates"].items():
        if type(gate) is not dict or gate.get("status") not in {"PASSED", "FAILED", "NOT_RUN", "PENDING"}:
            raise ValueError(f"Invalid M2 gate status: {name}")
        if type(gate.get("criteria")) is not str or not gate["criteria"].strip():
            raise ValueError(f"Missing qualification criteria: {name}")
        passed = gate["status"] == "PASSED"
        all_passed &= passed
        if passed:
            validate_evidence(path, name, gate)

    metrics = record["metrics"]
    if type(metrics) is not dict or not set(metrics).issubset(M2_METRICS):
        raise ValueError("Unknown M2 metric")
    for name, measurement in metrics.items():
        if type(measurement) is not dict or set(measurement) != {"value", "unit"}:
            raise ValueError(f"Invalid measured metric: {name}")
        value = measurement["value"]
        if type(value) not in {int, float} or not math.isfinite(value) or (value < 0 and name != "rss_slope_mib_per_min"):
            raise ValueError(f"Invalid measured metric: {name}")
        if measurement["unit"] != M2_METRICS[name]:
            raise ValueError(f"Invalid metric unit: {name}")
        if name in {"seek_count", "rebuild_count"} and type(value) is not int:
            raise ValueError(f"Invalid measured metric: {name}")

    platform = record["target_platform"]
    environment = record["environment"]
    if type(platform) is not dict or platform.get("status") not in {"QUALIFIED", "BLOCKED", "PENDING"}:
        raise ValueError("Invalid platform status")
    if type(environment) is not dict or type(environment.get("scope")) is not str or not environment["scope"]:
        raise ValueError("Invalid environment scope")
    for field in ("os", "arch", "triple", "scope"):
        if type(platform.get(field)) is not str or not platform[field]:
            raise ValueError(f"Invalid target platform field: {field}")
    for field in ("physical_audio_evidence", "native_platform_evidence"):
        if type(environment.get(field)) is not bool:
            raise ValueError(f"Invalid environment flag: {field}")
    if environment.get("clock_source") not in {"TEST_ONLY", "PREDICTED_OUTPUT", "LIVE_OUTPUT_TIMESTAMP"}:
        raise ValueError("Invalid output clock source")

    timing_pass = record["gates"]["output_clock"]["status"] == "PASSED"
    resource_pass = record["gates"]["resource_stress"]["status"] == "PASSED"
    required_metrics = set()
    if timing_pass:
        required_metrics.update({"session_duration_minutes", "video_frame_duration_ms", "timestamp_residual_ms", "av_error_ms"})
    if resource_pass:
        required_metrics.update({"session_duration_minutes", "seek_count", "rebuild_count", "seek_p95_ms", "rss_slope_mib_per_min"})
    if timing_pass or resource_pass:
        missing = required_metrics - set(metrics)
        if missing:
            raise ValueError(f"Missing required M2 measurements: {', '.join(sorted(missing))}")
        value = lambda name: metrics[name]["value"]
    if resource_pass:
        if value("session_duration_minutes") < 10 or value("seek_count") < 2000 or value("rebuild_count") < 50:
            raise ValueError("M2 stress counts do not meet qualification limits")
        if value("seek_p95_ms") > 150 or value("rss_slope_mib_per_min") > 2:
            raise ValueError("M2 performance measurements exceed qualification limits")
    if timing_pass:
        if value("session_duration_minutes") < 10:
            raise ValueError("Output timing requires a 10-minute session")
        if value("av_error_ms") > value("video_frame_duration_ms") + value("timestamp_residual_ms"):
            raise ValueError("A/V error exceeds one frame plus timestamp residual")
    if timing_pass and (
        environment["clock_source"] != "LIVE_OUTPUT_TIMESTAMP" or not environment["physical_audio_evidence"]
    ):
        raise ValueError("Output clock pass requires physical live-output timestamp evidence")

    if platform["status"] == "QUALIFIED":
        if not all_passed:
            raise ValueError("Platform cannot qualify with unmet M2 gates")
        if not environment["physical_audio_evidence"] or not environment["native_platform_evidence"]:
            raise ValueError("Platform cannot qualify without required native physical evidence")
        if environment["clock_source"] != "LIVE_OUTPUT_TIMESTAMP":
            raise ValueError("Platform cannot qualify from a predicted or test-only clock")


def evidence_files(path, name, items):
    """Validate evidence entries (shape, containment, hash) and return the resolved files."""
    if type(items) is not list:
        raise ValueError(f"{name}: evidence must be a list")
    evidence_root = (path.parent / "evidence").resolve()
    resolved = []
    for evidence in items:
        if type(evidence) is not dict or set(evidence) != {"path", "sha256"}:
            raise ValueError(f"Invalid qualification evidence entry for {name}")
        relative = evidence["path"]
        if type(relative) is not str or not relative:
            raise ValueError(f"Invalid qualification evidence path for {name}")
        source = (path.parent / relative).resolve()
        if evidence_root not in source.parents:
            raise ValueError(f"Evidence for {name} escapes the evidence directory: {relative}")
        validate_evidence(path, name, {"evidence": [evidence]})
        resolved.append(source)
    if len(set(resolved)) != len(resolved):
        raise ValueError(f"{name}: duplicate evidence file")
    return resolved


# Absolute paths of a private machine: a user's home, a workspace or a temporary directory
# (system paths such as /usr/bin/Xvfb or /dev/vda2 identify a platform, not a person).
PRIVATE_PATH = re.compile(
    r"(?<![A-Za-z0-9_.~<:/-])/(?:root|home|Users|tmp|var|mnt|private|srv|opt|run|media|workspaces?|builds?)(?:/|\b)"
    r"|(?<![A-Za-z0-9])[A-Za-z]:\\"
    r"|(?<![A-Za-z0-9])[A-Za-z]:/(?!/)"
)
TEXT_SUFFIXES = {".json", ".log", ".md", ".txt"}
BINARY_SUFFIXES = {".png"}


def credential_views(data):
    """The byte strings credential patterns are matched against: the data as-is (NUL as a
    separator, so text on either side of a NUL is still scanned) and with every NUL removed
    (UTF-16-style text becomes ASCII)."""
    yield data.replace(b"\0", b" ").decode("utf-8", errors="ignore")
    if b"\0" in data:
        yield data.replace(b"\0", b"").decode("utf-8", errors="ignore")


def scan_for_secrets(source, label):
    """Reject credential-like text and private paths; the offending text is never echoed.
    A NUL never disables the scan, and textual evidence must be plain UTF-8 text."""
    with source.open("rb") as stream:
        data = stream.read(MAX_SCANNED_BYTES + 1)
    if len(data) > MAX_SCANNED_BYTES:
        raise ValueError(f"{label}: too large to scan for secrets")
    suffix = source.suffix.lower()
    if suffix in TEXT_SUFFIXES or suffix == "":
        if b"\0" in data:
            raise ValueError(f"{label}: unexpected binary content in textual evidence {source.name}")
        try:
            data.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ValueError(f"{label}: textual evidence {source.name} is not valid UTF-8") from error
    elif suffix not in BINARY_SUFFIXES:
        raise ValueError(f"{label}: unsupported evidence type {suffix} ({source.name})")
    for text in credential_views(data):
        for pattern_name, pattern in SECRET_PATTERNS:
            if pattern.search(text):
                raise ValueError(f"{label}: credential-like text ({pattern_name}) in {source.name}")
        if suffix not in BINARY_SUFFIXES and PRIVATE_PATH.search(text):
            raise ValueError(f"{label}: absolute private path in {source.name}")


def evidence_json(source):
    if source.suffix != ".json":
        return None
    try:
        value = json.loads(source.read_text())
    except (OSError, ValueError) as error:
        raise ValueError(f"Evidence is not valid JSON: {source.name}") from error
    return value if type(value) is dict else None


# ---- authentic evidence contracts ----------------------------------------------------------------------------
#
# An authentic pass is only as good as the records it cites. Every cited file must be a JSON
# record of schema ``m3-authentic/1`` bound to exactly that gate:
#
#   {"schema": "m3-authentic/1", "evidence_kind": "authentic", "fixture_only": false,
#    "gate": <gate name>, "platform": {"system": ..., "machine": ...},
#    "adapter": {"agent_name": ..., "protocol_version": 1, "launch_identity": <sha256>},
#    "probe": {"status": "Ready", "protocol_version": 1, "fixture_detected": false},
#    "cleanup": {"owned_processes_after": 0},
#    "measurements": { <the gate's own contract below> }}
#
# ``launch_identity`` is ``launch_identity_digest`` of the exact adapter launch (the same value
# the application derives before it trusts a writer-containment qualification).

EVIDENCE_SCHEMA = "m3-authentic/1"
HEX64 = re.compile(r"[0-9a-f]{64}")


def expect(condition, message):
    if not condition:
        raise ValueError(message)


def launch_identity_digest(executable_sha256, args, auth_env_names, arg_files=()):
    """SHA-256 over the NUL-joined parts ``fframes-launch/1``, the executable digest, the
    argument count and arguments, the sorted sign-in variable name count and names, and the
    count and ``index:sha256`` entries of arguments that name existing files."""
    names = sorted(auth_env_names)
    parts = ["fframes-launch/1", executable_sha256, str(len(args)), *args, str(len(names)), *names]
    parts.append(str(len(arg_files)))
    parts.extend(f"{index}:{digest}" for index, digest in arg_files)
    return hashlib.sha256(b"\0".join(part.encode() for part in parts)).hexdigest()


def _scenarios(measurements, required, label):
    scenarios = measurements.get("scenarios")
    expect(type(scenarios) is list and all(type(item) is dict for item in scenarios), f"{label}: scenarios must be a list of records")
    named = {item.get("name"): item for item in scenarios}
    for name in required:
        expect(name in named, f"{label}: scenario {name} was not measured")
    return named


def _probe_contract(m):
    expect(m.get("auth_status") == "ready", "probe: authentication was not proven ready")
    expect(type(m.get("capabilities")) is dict and m["capabilities"], "probe: no advertised capabilities recorded")


def _two_edit_contract(m):
    steps = m.get("steps")
    expect(type(steps) is list and all(type(step) is dict for step in steps), "two-edit run: steps must be a list of records")
    order = ["brief_result_a", "second_edit_b", "undo_to_a", "restart_recovered"]
    named = {step.get("name"): step for step in steps}
    expect([step.get("name") for step in steps] == order, f"two-edit run: steps must be exactly {order} in order")
    for name in order:
        step = named[name]
        for key in ("revision", "timeline_identity", "frame_identity", "audio_identity"):
            expect(type(step.get(key)) is str and len(step[key]) >= 12, f"two-edit run: {name} lacks {key}")
        expect(step.get("inventories_checked") is True, f"two-edit run: {name} did not check the source/Git/draft inventories")
    expect(named["second_edit_b"]["revision"] != named["brief_result_a"]["revision"], "two-edit run: B equals A")
    expect(named["undo_to_a"]["revision"] == named["brief_result_a"]["revision"], "two-edit run: Undo did not return to A")
    expect(named["restart_recovered"]["revision"] == named["undo_to_a"]["revision"], "two-edit run: restart did not recover the committed acceptance")


def _writer_contract(m):
    named = _scenarios(m, ["edit", "stop", "provider_crash"], "writer containment")
    for name in ("edit", "stop", "provider_crash"):
        expect(named[name].get("escaped_descendants") == 0, f"writer containment: {name} left an escaped descendant")
        expect(named[name].get("group_empty_after") is True, f"writer containment: the {name} group was not empty afterwards")


def _repair_contract(m):
    expect(m.get("compiler_error_introduced") is True, "repair: no compiler error was provoked")
    expect(m.get("repair_context_delivered") is True, "repair: structured repair context did not reach the agent")
    expect(m.get("repair_attempts") == 1, "repair: exactly one repair attempt is required")


def _interaction_contract(m):
    required = ["permission", "clarification", "stop_each_phase", "provider_crash", "source_conflict", "interrupted_publication_restart"]
    named = _scenarios(m, required, "interaction and failure modes")
    for name in required:
        expect(named[name].get("passed") is True, f"interaction: scenario {name} did not pass")
        expect(named[name].get("stuck_writers") == 0, f"interaction: scenario {name} left a stuck writer")


def _mcp_cli_contract(m):
    expect(m.get("mcp_session_new_accepted") is True or m.get("cli_route_ran") is True, "mcp/cli: neither route was exercised")
    expect(type(m.get("project_tool_calls")) is int and m["project_tool_calls"] >= 1, "mcp/cli: no project tool call succeeded")


def _cycles_contract(m):
    expect(type(m.get("cycles")) is int and m["cycles"] >= 20, "cleanup: fewer than 20 real cycles")
    for key in ("owned_processes_after", "broker_grants_after", "leases_after"):
        expect(m.get(key) == 0, f"cleanup: {key} is not zero")


def _ime_contract(m):
    ime = m.get("ime")
    expect(type(ime) is dict and type(ime.get("name")) is str and ime["name"].strip() and ime.get("composition_verified") is True, "ime: composition with a named input method was not verified")
    for key in ("focus_tab_order_verified", "scroll_while_streaming_verified", "space_not_stolen_verified"):
        expect(m.get(key) is True, f"ime: {key} is not true")
    session = m.get("session")
    expect(type(session) is dict and type(session.get("display_server")) is str and session["display_server"].strip(), "ime: the native session is not described")


def _audio_contract(m):
    device = m.get("device")
    expect(type(device) is dict and type(device.get("name")) is str and device["name"].strip() and device.get("physical") is True, "audio: no physical output device is recorded")
    for key in ("output_clock_measured", "handoff_epoch_fresh", "display_presented"):
        expect(m.get(key) is True, f"audio: {key} is not true")


def _windows_contract(m):
    for key in ("workflow_run", "packaging_verified", "job_object_containment_verified"):
        expect(m.get(key) is True, f"windows: {key} is not true")


def _macos_contract(m):
    for key in ("workflow_run", "packaging_verified", "process_containment_verified"):
        expect(m.get(key) is True, f"macos: {key} is not true")


# gate -> (platform system the record must name, or None; measurement contract)
AUTHENTIC_CONTRACTS = {
    "auth_adapter_probe_v1": (None, _probe_contract),
    "auth_two_edit_undo_restart": (None, _two_edit_contract),
    "auth_writer_process_group": (None, _writer_contract),
    "auth_compiler_error_repair": (None, _repair_contract),
    "auth_interaction_and_failure_modes": (None, _interaction_contract),
    "auth_mcp_cli_support": (None, _mcp_cli_contract),
    "auth_twenty_cycle_cleanup": (None, _cycles_contract),
    "auth_visual_ime": (None, _ime_contract),
    "auth_physical_audio": (None, _audio_contract),
    "auth_windows": ("Windows", _windows_contract),
    "auth_macos": ("Darwin", _macos_contract),
}
assert set(AUTHENTIC_CONTRACTS) == {name for name, kind in M3_GATES.items() if kind == "authentic"}


def validate_authentic_record(name, source, value, adapter):
    """The contract an authentic pass's cited file must satisfy (see the block above)."""
    label = f"Authentic pass {name}: {source.name}"
    expect(type(value) is dict, f"{label} is not a structured evidence record")
    expect(value.get("schema") == EVIDENCE_SCHEMA, f"{label} does not follow the {EVIDENCE_SCHEMA} contract")
    expect(value.get("evidence_kind") == "authentic", f"{label} is not authentic evidence")
    expect(value.get("fixture_only") is False, f"{label} does not state fixture_only: false")
    expect(value.get("gate") == name, f"{label} is bound to another gate")
    system, contract = AUTHENTIC_CONTRACTS[name]
    platform = value.get("platform")
    expect(
        type(platform) is dict and all(type(platform.get(k)) is str and platform[k].strip() for k in ("system", "machine")),
        f"{label} does not record the platform it was measured on",
    )
    if system is not None:
        expect(platform["system"] == system, f"{label} was measured on {platform['system']}, not {system}")
    record_adapter = value.get("adapter")
    expect(type(record_adapter) is dict, f"{label} records no adapter identity")
    expect(record_adapter.get("agent_name") == adapter.get("agent_name"), f"{label} names another adapter than the ledger")
    expect(record_adapter.get("protocol_version") == 1, f"{label} did not negotiate ACP v1")
    expect(
        type(record_adapter.get("launch_identity")) is str and HEX64.fullmatch(record_adapter["launch_identity"])
        and record_adapter["launch_identity"] == adapter.get("launch_identity"),
        f"{label} is bound to another adapter launch than the ledger",
    )
    probe = value.get("probe")
    expect(
        type(probe) is dict and probe.get("status") == "Ready" and probe.get("protocol_version") == 1 and probe.get("fixture_detected") is False,
        f"{label} does not record a successful, non-fixture probe",
    )
    cleanup = value.get("cleanup")
    expect(type(cleanup) is dict and cleanup.get("owned_processes_after") == 0 and type(cleanup.get("owned_processes_after")) is int, f"{label} does not measure a clean teardown")
    measurements = value.get("measurements")
    expect(type(measurements) is dict, f"{label} has no measurements")
    try:
        contract(measurements)
    except ValueError as error:
        raise ValueError(f"{label}: {error}") from error


# development gate -> the harness check whose recorded run backs it
DEVELOPMENT_CHECKS = {
    "dev_acp_transport": "acp_transport",
    "dev_engine_transactions": "engine_transactions",
    "dev_workflow_fixture": "app_workflow",
    "dev_tools_and_validation": "app_tools_validation",
    "dev_native_ui_workflow": "app_ui_workflow",
    "dev_native_x11_shell": "native_x11_shell",
    "dev_resource_bounds": "dev_evidence",
    "dev_compiler_count": "dev_evidence",
    "dev_cleanup_cycles": "dev_evidence",
    "dev_cli_mcp_parity": "dev_evidence",
    "dev_publication_primitive": "dev_evidence",
    "dev_real_sdk_workflow": "real_sdk_workflow",
}
assert set(DEVELOPMENT_CHECKS) == {name for name, kind in M3_GATES.items() if kind == "development"}


def validate_development_pass(name, files):
    """A development pass is backed by the harness run record it cites: the producing check
    ran, exited 0, did not time out, and every test target it recorded passed."""
    runs = [source for source in files if source.name == "harness-run.json"]
    expect(len(runs) == 1, f"Development pass {name} must cite the harness run record")
    run = evidence_json(runs[0])
    checks = run.get("checks") if run else None
    expect(type(checks) is list, f"Development pass {name}: the harness run record lists no checks")
    wanted = DEVELOPMENT_CHECKS[name]
    check = next((c for c in checks if type(c) is dict and c.get("name") == wanted), None)
    expect(check is not None, f"Development pass {name}: the harness did not run check {wanted}")
    expect(check.get("status") == "pass" and check.get("returncode") == 0, f"Development pass {name}: check {wanted} did not pass in the recorded run")
    results = check.get("test_results")
    expect(
        type(results) is list and results
        and all(type(r) is dict and r.get("outcome") == "ok" and r.get("failed") == 0 for r in results)
        and sum(r.get("passed", 0) for r in results) > 0,
        f"Development pass {name}: check {wanted} recorded no passing test results",
    )
    expect(
        not run.get("owned_processes_alive_at_end"), f"Development pass {name}: the recorded run left harness-owned processes alive",
    )
    log = check.get("log")
    expect(
        type(log) is str and Path(log).name in {source.name for source in files if source.suffix == ".log"},
        f"Development pass {name}: the check's log is not cited",
    )


def validate_m3(path, record):
    required = {"kind", "schema_version", "timestamp", "environment", "gates", "acceptance"}
    if set(record) != required or record["schema_version"] != 1:
        raise ValueError("Invalid M3 qualification schema")
    if type(record["timestamp"]) is not str or not record["timestamp"]:
        raise ValueError("Invalid M3 timestamp")
    environment = record["environment"]
    if type(environment) is not dict:
        raise ValueError("Invalid M3 environment")
    for field in ("scope", "os", "toolchain", "git", "sdk", "harness", "adapter"):
        if field not in environment:
            raise ValueError(f"Missing M3 environment field: {field}")
    if type(environment["scope"]) is not str or not environment["scope"].strip():
        raise ValueError("Invalid M3 environment scope")
    for field in ("os", "toolchain", "git", "sdk", "harness"):
        if type(environment[field]) is not dict or not environment[field]:
            raise ValueError(f"Invalid M3 environment field: {field}")
    if type(environment["harness"].get("arguments")) is not list:
        raise ValueError("The M3 environment must record the harness arguments")
    adapter = environment["adapter"]
    if adapter is not None and type(adapter) is not dict:
        raise ValueError("Invalid M3 adapter identity")
    if set(record["gates"]) != set(M3_GATES):
        raise ValueError("Invalid M3 gates")

    scan_for_secrets(path, "ledger")
    authentic_ok = adapter is not None and (
        type(adapter.get("agent_name")) is str
        and adapter["agent_name"].strip().lower() not in FIXTURE_AGENT_NAMES
        and adapter.get("protocol_version") == 1
        and adapter.get("fixture_detected") is False
        and type(adapter.get("launch_identity")) is str
        and HEX64.fullmatch(adapter["launch_identity"]) is not None
        and type(adapter.get("executable_sha256")) is str
        and HEX64.fullmatch(adapter["executable_sha256"]) is not None
    )
    used_by_development = set()
    authentic_paths = {}
    for name, gate in record["gates"].items():
        if type(gate) is not dict or set(gate) - {"kind", "status", "criteria", "notes", "evidence", "prerequisite"}:
            raise ValueError(f"Invalid M3 gate: {name}")
        if gate.get("kind") != M3_GATES[name]:
            raise ValueError(f"M3 gate {name} must be of kind {M3_GATES[name]}")
        status = gate.get("status")
        if status not in M3_STATUSES:
            raise ValueError(f"Invalid M3 gate status: {name}")
        for field in ("criteria", "notes"):
            if type(gate.get(field)) is not str or not gate[field].strip():
                raise ValueError(f"Missing M3 gate {field}: {name}")
        files = evidence_files(path, name, gate.get("evidence", []))
        for source in files:
            scan_for_secrets(source, f"evidence of {name}")
        if status == "pass" and not files:
            raise ValueError(f"{name} claims a pass without evidence")
        if status in {"not_run", "blocked"}:
            prerequisite = gate.get("prerequisite")
            if type(prerequisite) is not str or len(prerequisite.strip()) < 20:
                raise ValueError(f"{name} is {status} without a stated prerequisite")
        elif "prerequisite" in gate:
            raise ValueError(f"{name} states a prerequisite although it is {status}")
        if gate["kind"] == "development":
            used_by_development.update(files)
            for source in files:
                value = evidence_json(source)
                if value is not None and value.get("evidence_kind") == "authentic":
                    raise ValueError(f"Development gate {name} cites authentic evidence {source.name}")
            if status == "pass":
                validate_development_pass(name, files)
        else:
            authentic_paths[name] = files
            if status == "not_run" and files:
                raise ValueError(f"Authentic gate {name} is not_run but cites evidence")
            if status == "pass":
                if not authentic_ok:
                    raise ValueError(f"Authentic pass {name} requires a non-fixture adapter identity with a launch identity")
                for source in files:
                    validate_authentic_record(name, source, evidence_json(source), adapter)
    claimed = {}
    for name, files in authentic_paths.items():
        if record["gates"][name]["status"] == "pass" and used_by_development.intersection(files):
            raise ValueError(f"Authentic pass {name} cites development evidence")
        if record["gates"][name]["status"] == "pass":
            for source in files:
                if claimed.setdefault(source, name) != name:
                    raise ValueError(f"Authentic evidence {source.name} is cited by both {claimed[source]} and {name}: a record is bound to one gate")

    all_authentic = all(
        gate["status"] == "pass" for gate in record["gates"].values() if gate["kind"] == "authentic"
    )
    acceptance = record["acceptance"]
    if type(acceptance) is not dict or set(acceptance) != {"m3_authenticated", "reason"}:
        raise ValueError("Invalid M3 acceptance")
    if type(acceptance["reason"]) is not str or not acceptance["reason"].strip():
        raise ValueError("Invalid M3 acceptance reason")
    if acceptance["m3_authenticated"] not in {"pass", "not_run"}:
        raise ValueError("Invalid M3 authenticated acceptance status")
    if (acceptance["m3_authenticated"] == "pass") != all_authentic:
        raise ValueError("M3 authenticated acceptance must pass exactly when every authentic gate passes")


def validate(path):
    path = Path(path)
    record = load_record(path)
    kind = record.get("kind")
    if kind is None:
        validate_m0(path, record)
    elif kind == "m2":
        validate_m2(path, record)
    elif kind == "m3":
        validate_m3(path, record)
    else:
        raise ValueError(f"Unsupported qualification kind: {kind}")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("records", nargs="*", type=Path, help="ledgers to validate (default: M0, M2 and M3)")
    args = parser.parse_args()
    for record in args.records or DEFAULT_LEDGERS:
        validate(record)
        print(f"valid: {record}")
    print("Qualification records valid; pending gates remain unmet")


if __name__ == "__main__":
    main()
