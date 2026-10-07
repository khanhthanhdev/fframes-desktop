#!/usr/bin/env python3
"""Check qualification claims and referenced evidence without inventing native results.

Usage: validate-qualification.py [LEDGER ...]

With no argument the shipped ledgers (M0, M2, M3, M4, M5) are validated. M3 rules
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
from datetime import datetime
from pathlib import Path

QUALIFICATION = Path(__file__).resolve().parents[1] / "qualification"
DEFAULT_LEDGERS = [
    QUALIFICATION / "m0-results.json",
    QUALIFICATION / "m2-results.json",
    QUALIFICATION / "m3-results.json",
    QUALIFICATION / "m4-results.json",
    QUALIFICATION / "m5-results.json",
    QUALIFICATION / "m6-results.json",
]

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
M4_GATES = {
    "dev_scoped_task_context": "development",
    "dev_scoped_candidate_coverage": "development",
    "dev_stale_queue_refusal": "development",
    "dev_native_linux_scope_ui": "development",
    "dev_managed_sdk_preset_render": "development",
    "auth_provider_scoped_edit": "authentic",
    "auth_windows": "authentic",
    "auth_macos": "authentic",
}
M4_STATUSES = {"pass", "fail", "blocked", "not_run"}
M5_GATES = {
    "dev_identity_and_geometry": "development",
    "dev_displayed_canvas_selection": "development",
    "dev_source_retrieval_and_tools": "development",
    "dev_frozen_canvas_task_scope": "development",
    "dev_managed_sdk_title_lookup": "development",
    "dev_native_linux_canvas": "development",
    "dev_resource_bounds_and_cleanup": "development",
    "dev_full_title_edit_apply_undo_reopen": "development",
    "auth_provider_title_edit_undo_reopen": "authentic",
    "auth_physical_display_input": "authentic",
    "auth_windows": "authentic",
    "auth_macos": "authentic",
}
M5_STATUSES = {"pass", "fail", "blocked", "not_run"}
M6_PROVIDERS = {"claude", "codex", "pi", "antigravity"}
M6_GATES = {
    "create_edit_export",
    "selected_context_edit",
    "compiler_error_repair",
    "completion_and_streaming",
    "permission_and_errors",
    "interruption_and_cleanup",
    "restoration_and_fallback",
    "tool_parity",
    "image_context",
    "twenty_cycle_cleanup",
    "controlled_handoff",
    "platform_linux",
    "platform_windows",
    "platform_macos",
    "platform_physical_device",
}
M6_MANDATORY_SCENARIO_GATES = {
    "create_edit_export",
    "selected_context_edit",
    "compiler_error_repair",
    "completion_and_streaming",
    "permission_and_errors",
    "interruption_and_cleanup",
    "restoration_and_fallback",
    "tool_parity",
    "image_context",
    "twenty_cycle_cleanup",
    "controlled_handoff",
    "platform_linux",
}
M6_STATUSES = {"pass", "fail", "blocked", "not_run"}
M6_CAPABILITIES = {
    "load_session",
    "resume_session",
    "prompt_image",
    "prompt_audio",
    "prompt_embedded_context",
    "mcp_stdio",
    "mcp_http",
    "mcp_sse",
}
M6_GATE_BOOLEAN_MEASUREMENTS = {
    "create_edit_export": {
        "create_completed",
        "selected_title_edit_completed",
        "inspect_passed",
        "preview_verified",
        "apply_undo_reopen_passed",
        "cli_export_passed",
    },
    "selected_context_edit": {"frozen_scope_verified", "out_of_scope_unchanged"},
    "compiler_error_repair": {"repair_succeeded"},
    "completion_and_streaming": {
        "stream_responsive",
        "follow_up_completed",
        "quiet_turn_quiescent",
    },
    "permission_and_errors": {
        "approve_handled",
        "decline_handled",
        "protocol_error_recovered",
    },
    "restoration_and_fallback": {
        "resume_same_cwd",
        "fresh_grants",
        "unsupported_fallback",
        "interrupted_prompt_not_replayed",
    },
    "image_context": {"visual_verified", "image_disabled_fallback_verified"},
    "controlled_handoff": {
        "draft_retained",
        "source_fence_passed",
        "queue_transfer_explicit",
    },
    "platform_linux": {"platform_workflow_passed"},
    "platform_windows": {"platform_workflow_passed"},
    "platform_macos": {"platform_workflow_passed"},
    "platform_physical_device": {
        "physical_device",
        "display_input_exercised",
        "audio_device_exercised",
    },
}
M6_TOOL_NAMES = {
    "project_context",
    "timeline",
    "render_frame",
    "render_strip",
    "inspect",
    "build_status",
    "selection_context",
    "source_lookup",
    "style_context",
}
MAX_M5_METADATA_BYTES = 1024 * 1024
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


def validate_m4(path, record):
    required = {"kind", "schema_version", "timestamp", "environment", "gates", "acceptance"}
    if set(record) != required or record["schema_version"] != 1:
        raise ValueError("Invalid M4 qualification schema")
    if type(record["timestamp"]) is not str or not record["timestamp"].strip():
        raise ValueError("Invalid M4 timestamp")
    environment = record["environment"]
    if type(environment) is not dict or set(environment) != {"scope"}:
        raise ValueError("Invalid M4 environment")
    if type(environment["scope"]) is not str or not environment["scope"].strip():
        raise ValueError("Invalid M4 environment scope")
    if type(record["gates"]) is not dict or set(record["gates"]) != set(M4_GATES):
        raise ValueError("Invalid M4 gates")

    scan_for_secrets(path, "ledger")
    all_passed = True
    for name, kind in M4_GATES.items():
        gate = record["gates"][name]
        if type(gate) is not dict or set(gate) - {
            "kind", "status", "criteria", "notes", "evidence", "prerequisite"
        }:
            raise ValueError(f"Invalid M4 gate: {name}")
        if gate.get("kind") != kind:
            raise ValueError(f"M4 gate {name} must be of kind {kind}")
        status = gate.get("status")
        if status not in M4_STATUSES:
            raise ValueError(f"Invalid M4 gate status: {name}")
        for field in ("criteria", "notes"):
            if type(gate.get(field)) is not str or not gate[field].strip():
                raise ValueError(f"Missing M4 gate {field}: {name}")
        files = evidence_files(path, name, gate.get("evidence", []))
        for source in files:
            scan_for_secrets(source, f"evidence of {name}")

        if status == "pass":
            if not files:
                raise ValueError(f"{name} claims a pass without evidence")
            schema = "m4-development/1" if kind == "development" else "m4-authentic/1"
            proofs = [evidence_json(source) for source in files]
            proofs = [value for value in proofs if value is not None and value.get("schema") == schema]
            if not proofs:
                raise ValueError(f"{name} has no structured {kind} pass evidence")
            for proof in proofs:
                if (
                    proof.get("evidence_kind") != kind
                    or proof.get("gate") != name
                    or proof.get("result") != "pass"
                ):
                    raise ValueError(f"M4 pass evidence is not bound to {name}")
                if kind == "development":
                    if type(proof.get("exit_code")) is not int or proof["exit_code"] != 0:
                        raise ValueError(f"Development pass {name} has no successful recorded check")
                    if type(proof.get("fixture_only")) is not bool:
                        raise ValueError(f"Development pass {name} must label fixture evidence")
                    if type(proof.get("command")) is not str or not proof["command"].strip():
                        raise ValueError(f"Development pass {name} has no check command")
                else:
                    adapter = proof.get("adapter")
                    platform = proof.get("platform")
                    if (
                        proof.get("fixture_only") is not False
                        or type(adapter) is not dict
                        or type(adapter.get("name")) is not str
                        or not adapter["name"].strip()
                        or adapter["name"].strip().lower() in FIXTURE_AGENT_NAMES
                        or adapter.get("protocol_version") != 1
                        or type(adapter.get("launch_identity")) is not str
                        or HEX64.fullmatch(adapter["launch_identity"]) is None
                        or type(platform) is not dict
                        or not all(type(platform.get(key)) is str and platform[key].strip() for key in ("system", "arch"))
                        or proof.get("cleanup_empty") is not True
                    ):
                        raise ValueError(f"Authentic pass {name} lacks real adapter, platform or clean teardown evidence")
        if status in {"not_run", "blocked"}:
            prerequisite = gate.get("prerequisite")
            if type(prerequisite) is not str or len(prerequisite.strip()) < 20:
                raise ValueError(f"{name} is {status} without a stated prerequisite")
            if files:
                raise ValueError(f"Unrun M4 gate {name} must not cite pass evidence")
        elif "prerequisite" in gate:
            raise ValueError(f"{name} states a prerequisite although it is {status}")
        all_passed &= status == "pass"

    acceptance = record["acceptance"]
    if type(acceptance) is not dict or set(acceptance) != {"m4_complete", "reason"}:
        raise ValueError("Invalid M4 acceptance")
    if acceptance["m4_complete"] not in {"pass", "not_run"}:
        raise ValueError("Invalid M4 completion status")
    if type(acceptance["reason"]) is not str or not acceptance["reason"].strip():
        raise ValueError("Invalid M4 acceptance reason")
    if (acceptance["m4_complete"] == "pass") != all_passed:
        raise ValueError("M4 completion must pass exactly when every M4 gate passes")


M5_REQUIRED_TESTS = {
    "dev_identity_and_geometry": (
        "active_preview_binds_transformed_video_pixel_geometry_to_exact_frame_and_seek",
        "annotated_groups_keep_stable_keys_transforms_order_and_video_pixel_bounds",
        "semantic_keys_are_reversible_and_repeated_instances_do_not_alias",
    ),
    "dev_displayed_canvas_selection": (
        "accepted_frame_keeps_identity_geometry_and_image_pairing_atomic",
        "selection_uses_the_painted_transform_and_cycles_repeated_instances",
    ),
    "dev_source_retrieval_and_tools": (
        "syntax_spans_are_hash_bound_utf8_and_deterministic",
        "selection_retrieval_tools_share_dispatch_and_revision_fences",
        "selection_retrieval_tools_reject_partial_or_ambiguous_selection_queries",
    ),
    "dev_frozen_canvas_task_scope": (
        "canvas_packets_freeze_exact_object_or_nonsemantic_rectangle_evidence",
        "frame_selection_resolve_is_bounded_sorted_and_deduplicated",
    ),
    "dev_managed_sdk_title_lookup": (
        "generated_title_identity_geometry_and_source_anchor_survive_managed_render",
    ),
    "dev_native_linux_canvas": (
        "the_native_shell_selects_the_managed_starter_title_and_keeps_rectangle_scope_nonsemantic",
    ),
    "dev_resource_bounds_and_cleanup": (
        "source_indexes_retain_four_latest_revisions_and_reuse_matching_revision",
        "m5_resource_measurements_are_bounded",
        "malformed_syntax_and_cancelled_build_are_bounded_diagnostics",
        "closing_the_workflow_mid_task_reaps_everything",
        "shutdown_cancels_joins_and_removes_socket_and_capabilities",
        "metadata_byte_limit_rejects_overflow_without_publishing_partial_objects",
        "object_count_limit_is_reachable_with_compact_semantic_keys",
    ),
    "dev_full_title_edit_apply_undo_reopen": (
        "selected_title_prompt_apply_undo_and_reopen_round_trips_identity",
    ),
}


def _m5_authentic_contract(name, proof):
    adapter = proof.get("adapter")
    platform = proof.get("platform")
    if (
        proof.get("evidence_kind") != "authentic"
        or proof.get("fixture_only") is not False
        or proof.get("gate") != name
        or proof.get("result") != "pass"
        or type(adapter) is not dict
        or type(adapter.get("name")) is not str
        or not adapter["name"].strip()
        or adapter["name"].strip().lower() in FIXTURE_AGENT_NAMES
        or adapter.get("protocol_version") != 1
        or type(adapter.get("launch_identity")) is not str
        or HEX64.fullmatch(adapter["launch_identity"]) is None
        or adapter.get("fixture_detected") is not False
        or type(platform) is not dict
        or not all(type(platform.get(key)) is str and platform[key].strip() for key in ("system", "arch"))
        or proof.get("cleanup_empty") is not True
    ):
        raise ValueError(f"M5 authentic pass {name} lacks real adapter, platform or clean teardown evidence")

    measurements = proof.get("measurements")
    selection = measurements.get("selection") if type(measurements) is dict else None
    source_anchor = measurements.get("source_anchor") if type(measurements) is dict else None
    if (
        type(selection) is not dict
        or selection.get("semantic_object_selected") is not True
        or selection.get("displayed_frame_identity_matched") is not True
        or type(source_anchor) is not dict
        or source_anchor.get("anchor_resolved") is not True
        or source_anchor.get("source_hash_verified") is not True
    ):
        raise ValueError(
            f"M5 authentic pass {name} lacks semantic selection and source-anchor evidence"
        )

    if name == "auth_provider_title_edit_undo_reopen":
        transaction = proof.get("transaction")
        required_steps = ["prompt", "candidate_validation", "apply", "undo", "reopen"]
        if (
            type(transaction) is not list
            or [step.get("name") for step in transaction if type(step) is dict] != required_steps
            or any(step.get("passed") is not True for step in transaction if type(step) is dict)
        ):
            raise ValueError("M5 authentic provider pass lacks the complete title transaction")
    elif name == "auth_physical_display_input":
        if (
            type(measurements) is not dict
            or measurements.get("physical_device") is not True
            or measurements.get("display_input_exercised") is not True
        ):
            raise ValueError("M5 physical-input pass lacks physical-device measurements")
    else:
        expected_system = "Windows" if name == "auth_windows" else "Darwin"
        if platform.get("system") != expected_system:
            raise ValueError(f"M5 authentic pass {name} records the wrong platform")


def validate_m5(path, record):
    required = {"kind", "schema_version", "timestamp", "environment", "gates", "acceptance"}
    if set(record) != required or record.get("kind") != "m5" or record.get("schema_version") != 1:
        raise ValueError("Invalid M5 qualification schema")
    if type(record["timestamp"]) is not str or not record["timestamp"].strip():
        raise ValueError("Invalid M5 timestamp")
    try:
        datetime.fromisoformat(record["timestamp"].replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("Invalid M5 timestamp") from error
    environment = record["environment"]
    if type(environment) is not dict or set(environment) != {
        "scope", "os", "arch", "sdk_id", "sdk_manifest_sha256"
    }:
        raise ValueError("Invalid M5 environment")
    if any(type(environment.get(key)) is not str or not environment[key].strip() for key in ("scope", "os", "arch", "sdk_id")):
        raise ValueError("Invalid M5 environment identity")
    if HEX64.fullmatch(environment["sdk_manifest_sha256"]) is None:
        raise ValueError("Invalid M5 SDK manifest digest")
    if type(record["gates"]) is not dict or set(record["gates"]) != set(M5_GATES):
        raise ValueError("Invalid M5 gates")

    scan_for_secrets(path, "ledger")
    all_development = True
    all_gates = True
    for name, kind in M5_GATES.items():
        gate = record["gates"][name]
        if type(gate) is not dict or set(gate) - {
            "kind", "status", "criteria", "notes", "evidence", "prerequisite"
        }:
            raise ValueError(f"Invalid M5 gate: {name}")
        if gate.get("kind") != kind:
            raise ValueError(f"M5 gate {name} must be of kind {kind}")
        status = gate.get("status")
        if status not in M5_STATUSES:
            raise ValueError(f"Invalid M5 gate status: {name}")
        for field in ("criteria", "notes"):
            if type(gate.get(field)) is not str or not gate[field].strip():
                raise ValueError(f"Missing M5 gate {field}: {name}")
        files = evidence_files(path, name, gate.get("evidence", []))
        for source in files:
            scan_for_secrets(source, f"evidence of {name}")

        if status == "pass":
            if not files:
                raise ValueError(f"{name} claims a pass without evidence")
            proofs = [evidence_json(source) for source in files]
            if kind == "development":
                proofs = [proof for proof in proofs if proof is not None and proof.get("schema") == "m5-development/1"]
                if not proofs:
                    raise ValueError(f"{name} has no structured M5 development pass evidence")
                for proof in proofs:
                    test_names = proof.get("tests")
                    if (
                        proof.get("evidence_kind") != "development"
                        or proof.get("fixture_only") is not True
                        or proof.get("gate") != name
                        or proof.get("result") != "pass"
                        or type(proof.get("exit_code")) is not int
                        or proof["exit_code"] != 0
                        or type(proof.get("command")) is not str
                        or not proof["command"].strip()
                        or type(proof.get("passed")) is not int
                        or proof["passed"] < 1
                        or type(proof.get("failed")) is not int
                        or proof["failed"] != 0
                        or type(test_names) is not list
                        or any(type(test) is not str for test in test_names)
                        or not all(
                            any(required_test in test for test in test_names)
                            for required_test in M5_REQUIRED_TESTS[name]
                        )
                    ):
                        raise ValueError(f"M5 development pass evidence is incomplete or not bound to {name}")
                    if name in {
                        "dev_managed_sdk_title_lookup",
                        "dev_native_linux_canvas",
                        "dev_resource_bounds_and_cleanup",
                        "dev_full_title_edit_apply_undo_reopen",
                    } and (
                        proof.get("sdk_id") != environment["sdk_id"]
                        or proof.get("sdk_manifest_sha256") != environment["sdk_manifest_sha256"]
                    ):
                        raise ValueError(f"M5 SDK evidence for {name} is not bound to the recorded manifest")
                if name == "dev_resource_bounds_and_cleanup":
                    measurements = [
                        evidence_json(source)
                        for source in files
                        if evidence_json(source) is not None
                        and evidence_json(source).get("schema") == "m5-resource-measurements/1"
                    ]
                    if len(measurements) != 1:
                        raise ValueError("M5 resource pass must cite exactly one measurement record")
                    measurement = measurements[0]
                    source_measurement = measurement.get("source")
                    frame_measurement = measurement.get("frame")
                    limits = measurement.get("limits")
                    integer = lambda value: type(value) is int and value >= 0
                    if (
                        measurement.get("fixture_only") is not True
                        or measurement.get("sdk_id") != environment["sdk_id"]
                        or measurement.get("sdk_manifest_sha256") != environment["sdk_manifest_sha256"]
                        or type(measurement.get("rustc_version")) is not str
                        or not measurement["rustc_version"].strip()
                        or type(source_measurement) is not dict
                        or type(frame_measurement) is not dict
                        or type(limits) is not dict
                        or limits != {"source_files": 256, "source_bytes": 8 * 1024 * 1024, "frame_objects": 4096}
                        or source_measurement.get("file_count") != 256
                        or not integer(source_measurement.get("indexed_bytes"))
                        or source_measurement["indexed_bytes"] > limits["source_bytes"]
                        or not integer(source_measurement.get("build_micros"))
                        or not integer(source_measurement.get("lookup_samples"))
                        or source_measurement["lookup_samples"] < 1
                        or not integer(source_measurement.get("lookup_p95_micros"))
                        or not integer(frame_measurement.get("width"))
                        or frame_measurement["width"] == 0
                        or not integer(frame_measurement.get("height"))
                        or frame_measurement["height"] == 0
                        or not integer(frame_measurement.get("object_count"))
                        or not 1 <= frame_measurement["object_count"] <= limits["frame_objects"]
                        or not integer(frame_measurement.get("serialized_metadata_bytes"))
                        or not integer(frame_measurement.get("metadata_budget_bytes"))
                        or frame_measurement["metadata_budget_bytes"] != MAX_M5_METADATA_BYTES - 4096
                        or frame_measurement["serialized_metadata_bytes"] > frame_measurement["metadata_budget_bytes"]
                        or frame_measurement.get("worker_overflow_policy") != "reject_without_partial_objects"
                        or not integer(frame_measurement.get("serialized_bytes_at_object_limit"))
                        or not integer(frame_measurement.get("objects_excluded_by_metadata_byte_limit"))
                        or frame_measurement["objects_excluded_by_metadata_byte_limit"]
                        != limits["frame_objects"] - frame_measurement["object_count"]
                        or not integer(frame_measurement.get("hit_test_samples"))
                        or frame_measurement["hit_test_samples"] < 1
                        or not integer(frame_measurement.get("hit_test_p95_micros"))
                    ):
                        raise ValueError("M5 resource measurements are missing, unbounded or not tied to the SDK")
                if name == "dev_native_linux_canvas" and sum(source.suffix == ".png" for source in files) < 2:
                    raise ValueError("M5 native canvas pass must cite both inspected interaction screenshots")
                if name == "dev_managed_sdk_title_lookup":
                    required_artifacts = {
                        "m5-managed-title-frame/0.png",
                        "m5-managed-title-strip.png",
                        "m5-managed-inspect-summary.json",
                    }
                    cited_artifacts = {
                        source.relative_to(path.parent.resolve()).as_posix() for source in files
                    }
                    if not all(
                        any(path.endswith(f"/{required}") for path in cited_artifacts)
                        for required in required_artifacts
                    ):
                        raise ValueError("M5 managed title pass must cite frame, inspect and strip artifacts")
            else:
                proofs = [proof for proof in proofs if proof is not None and proof.get("schema") == "m5-authentic/1"]
                if not proofs:
                    raise ValueError(f"{name} has no structured M5 authentic pass evidence")
                for proof in proofs:
                    _m5_authentic_contract(name, proof)

        if status in {"not_run", "blocked"}:
            prerequisite = gate.get("prerequisite")
            if type(prerequisite) is not str or len(prerequisite.strip()) < 20:
                raise ValueError(f"{name} is {status} without a stated prerequisite")
            if files:
                raise ValueError(f"Unrun M5 gate {name} must not cite evidence")
        elif "prerequisite" in gate:
            raise ValueError(f"{name} states a prerequisite although it is {status}")
        if kind == "development":
            all_development &= status == "pass"
        all_gates &= status == "pass"

    acceptance = record["acceptance"]
    if type(acceptance) is not dict or set(acceptance) != {"development", "full", "reason"}:
        raise ValueError("Invalid M5 acceptance")
    if acceptance["development"] not in {"pass", "not_run"} or acceptance["full"] not in {"pass", "not_run"}:
        raise ValueError("Invalid M5 acceptance status")
    if type(acceptance["reason"]) is not str or not acceptance["reason"].strip():
        raise ValueError("Invalid M5 acceptance reason")
    if (acceptance["development"] == "pass") != all_development:
        raise ValueError("M5 development acceptance must pass exactly when every development gate passes")
    if (acceptance["full"] == "pass") != all_gates:
        raise ValueError("M5 full acceptance must pass exactly when every development and authentic gate passes")

def _extract_provider_qualification_summary(path, provider_id, provider):
    p_id = provider["id"]
    gates = provider.get("gates", {})

    proofs = {}
    for gate_name, gate in gates.items():
        if gate.get("status") == "pass" and gate.get("evidence"):
            files = [evidence_json((path.parent / ev["path"]).resolve()) for ev in gate["evidence"]]
            proofs[gate_name] = [p for p in files if p is not None]

    env_name = None
    for p_list in proofs.values():
        for proof in p_list:
            m = proof.get("measurements", {})
            if "environment_name" in m and type(m["environment_name"]) is str and m["environment_name"].strip():
                env_name = m["environment_name"].strip()
                break
        if env_name:
            break

    if not env_name:
        raise ValueError(f"Provider {provider_id} lacks a named measured environment in qualification evidence")

    export_proof = (proofs.get("create_edit_export") or [{}])[0]
    export_m = export_proof.get("measurements", {})
    latency_p95 = export_m.get("latency_p95_ms")
    if type(latency_p95) not in (int, float) or latency_p95 <= 0:
        raise ValueError(f"Provider {provider_id} lacks measured latency_p95_ms in create_edit_export")

    repair_proof = (proofs.get("compiler_error_repair") or [{}])[0]
    repair_m = repair_proof.get("measurements", {})
    repair_succeeded = 1.0 if repair_m.get("repair_succeeded") is True else 0.0

    tool_proof = (proofs.get("tool_parity") or [{}])[0]
    tool_m = tool_proof.get("measurements", {})
    tools_succeeded = tool_m.get("tools_succeeded")
    if type(tools_succeeded) is not int or tools_succeeded < 0:
        raise ValueError(f"Provider {provider_id} lacks measured tools_succeeded in tool_parity")

    visual_proof = (proofs.get("image_context") or [{}])[0]
    visual_m = visual_proof.get("measurements", {})
    visual_verified = 1.0 if visual_m.get("visual_verified") is True else 0.0

    stress_proof = (proofs.get("twenty_cycle_cleanup") or [{}])[0]
    stress_m = stress_proof.get("measurements", {})
    peak_rss_kib = stress_m.get("peak_rss_kib")
    if type(peak_rss_kib) not in (int, float) or peak_rss_kib <= 0:
        raise ValueError(f"Provider {provider_id} lacks measured peak_rss_kib in twenty_cycle_cleanup")

    failures = 0
    return {
        "provider_id": p_id,
        "environment_name": env_name,
        "failures": failures,
        "repair_success": repair_succeeded,
        "tools_succeeded": tools_succeeded,
        "visual_verified": visual_verified,
        "latency_p95_ms": float(latency_p95),
        "peak_rss_kib": float(peak_rss_kib),
    }


def _validate_m6_gate_measurements(provider_id, gate_name, proof):
    measurements = proof.get("measurements")
    if type(measurements) is not dict:
        raise ValueError(f"{provider_id}.{gate_name} authentic evidence has no measurements object")

    required_booleans = M6_GATE_BOOLEAN_MEASUREMENTS.get(gate_name, set())
    for name in required_booleans:
        if measurements.get(name) is not True:
            raise ValueError(f"{provider_id}.{gate_name} evidence is missing passing measurement {name}")

    if gate_name == "create_edit_export":
        revision_digest = measurements.get("export_revision_sha256")
        latency = measurements.get("latency_p95_ms")
        if type(revision_digest) is not str or HEX64.fullmatch(revision_digest) is None:
            raise ValueError(f"{provider_id}.{gate_name} evidence lacks an immutable export revision digest")
        if type(latency) not in (int, float) or latency <= 0:
            raise ValueError(f"{provider_id}.{gate_name} evidence lacks measured latency_p95_ms")
        if measurements.get("claimed_native_app_export") is True:
            raise ValueError(f"{provider_id} claims native app export in M6; export UI remains M7")
    elif gate_name == "compiler_error_repair":
        attempts = measurements.get("repair_attempts")
        if type(attempts) is not int or not 0 <= attempts <= 1:
            raise ValueError(f"{provider_id}.{gate_name} evidence exceeds the one-repair budget")
    elif gate_name == "tool_parity":
        results = measurements.get("tool_results")
        route = measurements.get("tool_route")
        if type(results) is not dict or set(results) != M6_TOOL_NAMES or any(value != "pass" for value in results.values()):
            raise ValueError(f"{provider_id}.{gate_name} evidence must pass all nine Studio tools")
        if measurements.get("tools_succeeded") != len(M6_TOOL_NAMES):
            raise ValueError(f"{provider_id}.{gate_name} evidence has an incorrect successful tool count")
        if route not in {"mcp", "cli"}:
            raise ValueError(f"{provider_id}.{gate_name} evidence must name its measured MCP or CLI route")
    elif gate_name == "interruption_and_cleanup":
        _writer_contract(measurements)
    elif gate_name == "twenty_cycle_cleanup":
        cycles = measurements.get("cycles_completed")
        peak_rss = measurements.get("peak_rss_kib")
        if type(cycles) is not int or cycles < 20:
            raise ValueError(f"{provider_id}.{gate_name} evidence must measure at least twenty cycles")
        if type(peak_rss) not in (int, float) or peak_rss <= 0:
            raise ValueError(f"{provider_id}.{gate_name} evidence lacks measured peak_rss_kib")
    elif gate_name == "controlled_handoff":
        if measurements.get("writer_overlap_count") != 0 or measurements.get("post_stop_writes") != 0:
            raise ValueError(f"{provider_id}.{gate_name} evidence records writer overlap or post-stop writes")


def validate_m6(path, record):
    required = {"kind", "schema_version", "timestamp", "environment", "providers", "ranking", "acceptance"}
    if set(record) != required or record.get("kind") != "m6" or record.get("schema_version") != 1:
        raise ValueError("Invalid M6 qualification schema")
    if type(record["timestamp"]) is not str or not record["timestamp"].strip():
        raise ValueError("Invalid M6 timestamp")
    try:
        datetime.fromisoformat(record["timestamp"].replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("Invalid M6 timestamp") from error
    environment = record["environment"]
    if type(environment) is not dict or set(environment) != {
        "scope", "os", "arch", "sdk_id", "sdk_manifest_sha256"
    }:
        raise ValueError("Invalid M6 environment")
    if any(type(environment.get(key)) is not str or not environment[key].strip() for key in ("scope", "os", "arch", "sdk_id")):
        raise ValueError("Invalid M6 environment identity")
    if HEX64.fullmatch(environment["sdk_manifest_sha256"]) is None:
        raise ValueError("Invalid M6 SDK manifest digest")
    if type(record["providers"]) is not dict or set(record["providers"]) != set(M6_PROVIDERS):
        raise ValueError("Invalid M6 providers matrix: must contain claude, codex, pi, and antigravity")

    scan_for_secrets(path, "ledger")

    all_cited_evidence = set()
    qualified_providers = set()
    for provider_id, provider in record["providers"].items():
        if type(provider) is not dict:
            raise ValueError(f"Provider {provider_id} is not an object")
        prov_required = {
            "id", "label", "distribution", "upstream_version", "auth_route",
            "mcp_route", "status", "prerequisites", "probe_observation", "capabilities", "gates"
        }
        prov_optional = {"launch_identity", "adapter_name"}
        if not prov_required.issubset(set(provider)) or set(provider) - (prov_required | prov_optional):
            raise ValueError(f"Provider {provider_id} has invalid fields")
        if provider["id"] != provider_id:
            raise ValueError(f"Provider {provider_id} id mismatch: {provider['id']}")
        if provider["status"] not in {"qualified", "experimental", "blocked", "not_run"}:
            raise ValueError(f"Invalid provider status for {provider_id}: {provider['status']}")
        if type(provider["prerequisites"]) is not str or len(provider["prerequisites"].strip()) < 20:
            raise ValueError(f"Provider {provider_id} missing explicit prerequisites")
        probe_obs = provider["probe_observation"]
        probe_required = {"status", "detail", "runtime_available", "observed_unix_seconds", "evidence"}
        if (
            type(probe_obs) is not dict
            or not probe_required.issubset(set(probe_obs))
            or set(probe_obs) - (probe_required | {"searched"})
        ):
            raise ValueError(f"Provider {provider_id} invalid probe_observation")
        if (
            type(probe_obs["status"]) is not str
            or not probe_obs["status"].strip()
            or type(probe_obs["detail"]) is not str
            or not probe_obs["detail"].strip()
        ):
            raise ValueError(f"Provider {provider_id} invalid probe status or detail")
        if "searched" in probe_obs and (
            type(probe_obs["searched"]) is not list
            or any(type(item) is not str for item in probe_obs["searched"])
        ):
            raise ValueError(f"Provider {provider_id} probe searched must be an array of strings")
        if probe_obs["runtime_available"] is not None and type(probe_obs["runtime_available"]) is not bool:
            raise ValueError(f"Provider {provider_id} probe runtime_available must be boolean or null")
        if type(probe_obs["observed_unix_seconds"]) is not int or probe_obs["observed_unix_seconds"] < 1700000000:
            raise ValueError(f"Provider {provider_id} invalid observed_unix_seconds")
        probe_ev = probe_obs["evidence"]
        if type(probe_ev) is not dict or set(probe_ev) != {"path", "sha256"}:
            raise ValueError(f"Provider {provider_id} invalid probe evidence shape")
        validate_evidence(path, f"{provider_id}.probe", {"evidence": [probe_ev]})
        probe_file = (path.parent / probe_ev["path"]).resolve()
        scan_for_secrets(probe_file, f"probe evidence of {provider_id}")
        probe_record = evidence_json(probe_file)
        if (
            probe_record is None
            or probe_record.get("schema") != "m6-probe/1"
            or probe_record.get("evidence_kind") != "probe"
            or probe_record.get("provider") != provider_id
            or type(probe_record.get("observed_unix_seconds")) is not int
            or probe_record["observed_unix_seconds"] < 1700000000
        ):
            raise ValueError(f"Provider {provider_id} probe evidence record invalid")
        capabilities = provider["capabilities"]
        if capabilities is not None and (
            type(capabilities) is not dict
            or set(capabilities) != M6_CAPABILITIES
            or any(type(value) is not bool for value in capabilities.values())
        ):
            raise ValueError(f"Provider {provider_id} capabilities must be null or the complete boolean capability set")
        launch_identity = provider.get("launch_identity")
        if launch_identity is not None:
            if type(launch_identity) is not str or HEX64.fullmatch(launch_identity) is None:
                raise ValueError(f"Invalid launch identity digest for {provider_id}")
        adapter_name = provider.get("adapter_name")
        if adapter_name is not None and (type(adapter_name) is not str or not adapter_name.strip()):
            raise ValueError(f"Invalid adapter name for {provider_id}")

        gates = provider.get("gates")
        if type(gates) is not dict or set(gates) != set(M6_GATES):
            raise ValueError(f"Invalid gates for provider {provider_id}")

        provider_all_mandatory_passed = True
        for gate_name, gate in gates.items():
            if type(gate) is not dict:
                raise ValueError(f"Gate {provider_id}.{gate_name} is not an object")
            gate_required = {"kind", "status", "criteria", "notes"}
            gate_optional = {"evidence", "prerequisite"}
            if not gate_required.issubset(set(gate)) or set(gate) - (gate_required | gate_optional):
                raise ValueError(f"Gate {provider_id}.{gate_name} missing required fields")
            status = gate.get("status")
            kind = gate.get("kind")
            if status not in M6_STATUSES:
                raise ValueError(f"Invalid gate status for {provider_id}.{gate_name}: {status}")
            if kind != "authentic":
                raise ValueError(f"{provider_id}.{gate_name} must be of kind authentic (provider gates are authentic-only)")
            if (
                type(gate["criteria"]) is not str
                or not gate["criteria"].strip()
                or type(gate["notes"]) is not str
                or not gate["notes"].strip()
            ):
                raise ValueError(f"{provider_id}.{gate_name} criteria and notes must be non-empty strings")

            if status in {"not_run", "blocked"}:
                prereq = gate.get("prerequisite")
                if type(prereq) is not str or len(prereq.strip()) < 20:
                    raise ValueError(f"{provider_id}.{gate_name} is {status} without prerequisite")
                if gate.get("evidence"):
                    raise ValueError(f"{provider_id}.{gate_name} is {status} but cites evidence")
            elif status == "pass":
                files = evidence_files(path, f"{provider_id}.{gate_name}", gate.get("evidence", []))
                for source in files:
                    if source in all_cited_evidence:
                        raise ValueError(f"Evidence file cited by multiple gates: {source.name}")
                    all_cited_evidence.add(source)
                    scan_for_secrets(source, f"evidence of {provider_id}.{gate_name}")
                if not files:
                    raise ValueError(f"{provider_id}.{gate_name} claims a pass without evidence")
                if launch_identity is None or adapter_name is None:
                    raise ValueError(f"{provider_id}.{gate_name} pass requires a launch identity and observed adapter name")
                if "prerequisite" in gate:
                    raise ValueError(f"{provider_id}.{gate_name} states a prerequisite although it passed")
                if kind == "authentic":
                    proofs = [evidence_json(source) for source in files]
                    for proof in proofs:
                        if proof is None:
                            raise ValueError(f"{provider_id}.{gate_name} has invalid JSON evidence")
                        if proof.get("evidence_kind") != "authentic" or proof.get("fixture_only") is not False:
                            raise ValueError(f"{provider_id}.{gate_name} authentic gate cites development or fixture evidence")
                        if proof.get("result") != "pass":
                            raise ValueError(f"{provider_id}.{gate_name} authentic evidence is not a pass")
                        if proof.get("provider_id") != provider_id or proof.get("gate") != gate_name:
                            raise ValueError(f"{provider_id}.{gate_name} evidence is bound to another provider or gate")
                        adapter = proof.get("adapter")
                        if type(adapter) is not dict:
                            raise ValueError(f"{provider_id}.{gate_name} authentic evidence missing adapter object")
                        raw_agent_name = adapter.get("agent_name") or adapter.get("name") or ""
                        if type(raw_agent_name) is not str or not raw_agent_name.strip():
                            raise ValueError(f"{provider_id}.{gate_name} adapter identity has no name")
                        agent_name = raw_agent_name.strip().lower()
                        if agent_name in FIXTURE_AGENT_NAMES:
                            raise ValueError(f"{provider_id}.{gate_name} authentic evidence uses fixture adapter")
                        if agent_name != adapter_name.lower():
                            raise ValueError(f"{provider_id}.{gate_name} evidence adapter does not match the observed provider adapter")
                        if adapter.get("protocol_version") != 1:
                            raise ValueError(f"{provider_id}.{gate_name} evidence did not negotiate ACP v1")
                        if proof.get("cleanup_empty") is not True:
                            raise ValueError(f"{provider_id}.{gate_name} authentic evidence lacks clean teardown")
                        cleanup = proof.get("cleanup")
                        if type(cleanup) is not dict or cleanup.get("owned_processes_after") != 0:
                            raise ValueError(f"{provider_id}.{gate_name} authentic evidence did not measure clean teardown (owned_processes_after != 0)")
                        if adapter.get("launch_identity") != launch_identity:
                            raise ValueError(f"{provider_id}.{gate_name} launch identity does not match provider")
                        expected_platform = "Windows" if gate_name == "platform_windows" else ("Darwin" if gate_name == "platform_macos" else "Linux")
                        proof_platform = proof.get("platform", {})
                        if type(proof_platform) is not dict or proof_platform.get("system") != expected_platform:
                            raise ValueError(f"{provider_id}.{gate_name} authentic evidence records wrong platform: {proof_platform.get('system')}")
                        _validate_m6_gate_measurements(provider_id, gate_name, proof)
            else:
                files = evidence_files(path, f"{provider_id}.{gate_name}", gate.get("evidence", []))
                for source in files:
                    scan_for_secrets(source, f"evidence of {provider_id}.{gate_name}")
                if not files:
                    raise ValueError(f"{provider_id}.{gate_name} failed without evidence")

            if gate_name in M6_MANDATORY_SCENARIO_GATES:
                if kind != "authentic" or status != "pass":
                    provider_all_mandatory_passed = False

        if provider_all_mandatory_passed:
            if provider["status"] != "qualified":
                raise ValueError(f"Provider {provider_id} passed all mandatory gates but status is not qualified")
            qualified_providers.add(provider_id)
        else:
            if provider["status"] == "qualified":
                raise ValueError(f"Provider {provider_id} claims qualified status but has unmet mandatory gates")

    ranking = record["ranking"]
    if type(ranking) is not dict or set(ranking) != {"status", "recommended", "experimental", "notes"}:
        raise ValueError("Invalid M6 ranking block")
    if type(ranking["notes"]) is not str or not ranking["notes"].strip():
        raise ValueError("Ranking notes must be a non-empty string")
    if type(ranking["experimental"]) is not list or any(type(pid) is not str for pid in ranking["experimental"]):
        raise ValueError("Ranking experimental must be a list of provider IDs")
    if ranking["status"] not in {"insufficient_evidence", "recommended"}:
        raise ValueError(f"Invalid ranking status: {ranking['status']}")
    recommended = ranking["recommended"]
    if type(recommended) is not list or any(type(provider_id) is not str for provider_id in recommended):
        raise ValueError("Ranking recommended must be a list")
    if len(recommended) != len(set(recommended)):
        raise ValueError("Ranking recommended contains duplicates")
    for r in recommended:
        if r not in qualified_providers:
            raise ValueError(f"Cannot recommend unqualified provider: {r}")

    if len(qualified_providers) < 2:
        if ranking["status"] != "insufficient_evidence":
            raise ValueError("Ranking status must be insufficient_evidence when fewer than 2 providers qualify")
        if recommended:
            raise ValueError("Ranking cannot recommend providers when fewer than 2 qualify")
    else:
        if ranking["status"] != "recommended":
            raise ValueError("Ranking status must be recommended when at least two providers qualify")
        summaries = [
            _extract_provider_qualification_summary(path, pid, record["providers"][pid])
            for pid in qualified_providers
        ]
        environments = {s["environment_name"] for s in summaries}
        if len(environments) > 1:
            raise ValueError(f"Incomparable ranking: providers measured on different environments: {environments}")

        ranked_summaries = sorted(
            summaries,
            key=lambda s: (
                s["failures"],
                -s["repair_success"],
                -s["tools_succeeded"],
                -s["visual_verified"],
                s["latency_p95_ms"],
                s["peak_rss_kib"],
                s["provider_id"],
            ),
        )
        expected_recommended = [s["provider_id"] for s in ranked_summaries[:2]]
        if recommended != expected_recommended:
            raise ValueError(
                f"Ranking recommendation mismatch: deterministic ranking requires {expected_recommended}, but ledger recommended {recommended}"
            )
    experimental = set(ranking["experimental"])
    expected_experimental = set(M6_PROVIDERS) - set(recommended)
    if experimental != expected_experimental:
        raise ValueError("Ranking experimental must list all providers not in recommended")

    acceptance = record["acceptance"]
    if type(acceptance) is not dict or set(acceptance) != {"development", "authentic", "full", "reason"}:
        raise ValueError("Invalid M6 acceptance block")
    for field in ("development", "authentic", "full"):
        if acceptance[field] not in {"pass", "not_run", "fail"}:
            raise ValueError(f"Invalid acceptance status for {field}")
    if acceptance["authentic"] == "pass" and len(qualified_providers) < 2:
        raise ValueError("M6 authentic acceptance requires at least two qualified providers")
    if acceptance["authentic"] == "pass" and ranking["status"] != "recommended":
        raise ValueError("M6 authentic acceptance requires a two-provider recommendation")
    if type(acceptance["reason"]) is not str or not acceptance["reason"].strip():
        raise ValueError("M6 acceptance reason must be a non-empty string")
    if acceptance["full"] == "pass" and (acceptance["development"] != "pass" or acceptance["authentic"] != "pass"):
        raise ValueError("M6 full acceptance requires both development and authentic pass")


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
    elif kind == "m4":
        validate_m4(path, record)
    elif kind == "m5":
        validate_m5(path, record)
    elif kind == "m6":
        validate_m6(path, record)
    else:
        raise ValueError(f"Unsupported qualification kind: {kind}")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "records", nargs="*", type=Path, help="ledgers to validate (default: M0, M2, M3, M4, M5, and M6)"
    )
    args = parser.parse_args()
    for record in args.records or DEFAULT_LEDGERS:
        validate(record)
        print(f"valid: {record}")
    print("Qualification records valid; pending gates remain unmet")



if __name__ == "__main__":
    main()
