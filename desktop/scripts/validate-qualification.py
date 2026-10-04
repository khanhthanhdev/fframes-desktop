#!/usr/bin/env python3
"""Check qualification claims and referenced evidence without inventing native results."""
import argparse
import hashlib
import json
import math
import string
from pathlib import Path

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


def validate(path):
    record = load_record(path)
    kind = record.get("kind")
    if kind is None:
        validate_m0(path, record)
    elif kind == "m2":
        validate_m2(path, record)
    else:
        raise ValueError(f"Unsupported qualification kind: {kind}")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=Path)
    args = parser.parse_args()
    validate(args.record)
    print("Qualification record valid; pending gates remain unmet")


if __name__ == "__main__":
    main()
