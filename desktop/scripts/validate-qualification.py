#!/usr/bin/env python3
"""Check qualification claims and referenced evidence without inventing native results."""
import argparse
import hashlib
import json
from pathlib import Path

GATES = {"managed_compilation", "gpui_startup", "renderer_worker", "acp_task", "selection_anchor"}


def validate(path):
    record = json.loads(path.read_text())
    if record["schema_version"] != 1 or set(record["gates"]) != GATES:
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
            if not gate.get("evidence"):
                raise ValueError(f"{name} claims a pass without evidence")
            for evidence in gate["evidence"]:
                source = (path.parent / evidence["path"]).resolve()
                if not source.is_file():
                    raise ValueError(f"Missing qualification evidence: {source}")
                with source.open("rb") as stream:
                    checksum = hashlib.file_digest(stream, "sha256").hexdigest()
                if checksum != evidence["sha256"]:
                    raise ValueError(f"Evidence changed: {source}")
    if record["target_platform"]["status"] not in {"QUALIFIED", "BLOCKED", "PENDING"}:
        raise ValueError("Invalid platform status")
    if record["target_platform"]["status"] == "QUALIFIED" and not all_passed:
        raise ValueError("Platform cannot qualify with unmet gates")
    for metric, value in record["metrics"].items():
        if type(value) not in {int, float} or value < 0:
            raise ValueError(f"Invalid measured metric: {metric}")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=Path)
    args = parser.parse_args()
    validate(args.record)
    print("Qualification record valid; pending gates remain unmet")


if __name__ == "__main__":
    main()
