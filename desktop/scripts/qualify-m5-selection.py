#!/usr/bin/env python3
"""Run M5 semantic canvas/source-retrieval development checks and record bounded evidence.

Example:
    python3 desktop/scripts/qualify-m5-selection.py \
        --sdk-bundle desktop/target/m5-managed-sdk-bundle

This runner deliberately leaves the authenticated provider, physical-device, Windows and macOS
gates unmet. A passing scripted/native-development run is not an authentic qualification.
"""
import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
QUALIFICATION = ROOT / "desktop" / "qualification"
VALIDATOR = ROOT / "desktop" / "scripts" / "validate-qualification.py"
LEDGER = QUALIFICATION / "m5-results.json"
SUMMARY = re.compile(r"test result: ok\. (\d+) passed; (\d+) failed")
PASSED_TEST = re.compile(r"^test ([^\s]+) \.\.\. ok$", re.MULTILINE)


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def command(*parts):
    return ["cargo", "test", "--locked", *parts]


def test_groups():
    app = ["--manifest-path", "desktop/Cargo.toml", "-p", "fframes-studio"]
    engine = ["--manifest-path", "desktop/Cargo.toml", "-p", "studio-engine"]
    project = ["--manifest-path", "desktop/Cargo.toml", "-p", "studio-project"]
    return {
        "dev_identity_and_geometry": [
            command("--manifest-path", "fframes-studio-runtime/Cargo.toml", "--test", "editor_metadata"),
            command("--manifest-path", "Cargo.toml", "-p", "fframes", "--test", "editor_identity"),
            command("--manifest-path", "Cargo.toml", "-p", "fframes-studio-protocol"),
        ],
        "dev_displayed_canvas_selection": [
            command(*engine, "--test", "canvas_selection"),
            command(*app, "--test", "canvas_selection"),
        ],
        "dev_source_retrieval_and_tools": [
            command(*project, "--test", "source_index"),
            command(*app, "--test", "selection_tools"),
        ],
        "dev_frozen_canvas_task_scope": [
            command(*engine, "--test", "agent_task_scope"),
            command(*app, "--test", "agent_tools", "frame_selection_resolve_is_bounded_sorted_and_deduplicated"),
        ],
        "dev_resource_bounds_and_cleanup": [
            command("--manifest-path", "fframes-studio-runtime/Cargo.toml", "--lib"),
            command(*app, "--lib", "source_indexes_retain_four_latest_revisions_and_reuse_matching_revision"),
            command(*app, "--test", "m5_resource_qualification", "--", "--exact", "m5_resource_measurements_are_bounded", "--nocapture"),
            command(*project, "--test", "source_index", "malformed_syntax_and_cancelled_build_are_bounded_diagnostics"),
            command(*app, "--test", "agent_workflow", "closing_the_workflow_mid_task_reaps_everything"),
            command(*app, "--test", "agent_tools", "shutdown_cancels_joins_and_removes_socket_and_capabilities"),
        ],
        "dev_full_title_edit_apply_undo_reopen": [
            command(
                *app,
                "--test",
                "agent_workflow",
                "--",
                "--ignored",
                "--exact",
                "selected_title_prompt_apply_undo_and_reopen_round_trips_identity",
                "--nocapture",
            ),
        ],
    }


def evidence_entry(path):
    return {
        "path": path.relative_to(QUALIFICATION).as_posix(),
        "sha256": sha256(path),
    }


def run_group(name, commands, evidence_directory, environment, sdk_identity=None):
    print(f"\n== {name} ==", flush=True)
    all_tests = []
    passed = 0
    failed = 0
    exit_code = 0
    command_text = []
    for args in commands:
        text = " ".join(args)
        command_text.append(text)
        print(f"$ {text}", flush=True)
        result = subprocess.run(args, cwd=ROOT, env=environment, text=True, capture_output=True)
        print(result.stdout, end="", flush=True)
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr, flush=True)
        test_names = PASSED_TEST.findall(result.stdout)
        all_tests.extend(test_names)
        summaries = SUMMARY.findall(result.stdout)
        passed += sum(int(ok) for ok, _ in summaries)
        failed += sum(int(bad) for _, bad in summaries)
        if result.returncode != 0:
            failed = max(failed, 1)
            exit_code = result.returncode

    status = "pass" if exit_code == 0 and passed > 0 and failed == 0 else "fail"
    proof = {
        "schema": "m5-development/1",
        "evidence_kind": "development",
        "fixture_only": True,
        "gate": name,
        "result": status,
        "exit_code": exit_code,
        "command": " && ".join(command_text),
        "tests": all_tests,
        "passed": passed,
        "failed": failed,
    }
    if sdk_identity is not None:
        proof.update(sdk_identity)
    proof_path = evidence_directory / f"{name}.json"
    proof_path.write_text(json.dumps(proof, indent=2) + "\n")
    return status, [evidence_entry(proof_path)]


def verify_bundle(bundle):
    manifest_path = bundle / "compatibility.json"
    manifest = json.loads(manifest_path.read_text())
    for artifact in manifest.get("artifacts", []):
        relative = artifact["url"].removeprefix("file://")
        path = bundle / relative
        if not path.is_file() or sha256(path) != artifact["sha256"]:
            raise ValueError(f"Managed SDK artifact failed hash verification: {artifact.get('name', 'unknown')}")
    return manifest, sha256(manifest_path)


def run():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sdk-bundle", required=True, type=Path, help="verified M5 managed SDK bundle")
    parser.add_argument(
        "--evidence-directory",
        type=Path,
        help="evidence subdirectory (default: evidence/m5-linux-native-YYYYMMDD)",
    )
    parser.add_argument("--skip-native", action="store_true", help="do not run the X11 managed-SDK UI test")
    args = parser.parse_args()

    bundle = args.sdk_bundle.resolve()
    manifest, manifest_digest = verify_bundle(bundle)
    evidence_directory = args.evidence_directory or (
        QUALIFICATION / "evidence" / f"m5-linux-native-{datetime.now(timezone.utc):%Y%m%d}"
    )
    evidence_directory = evidence_directory.resolve()
    if QUALIFICATION / "evidence" not in evidence_directory.parents:
        raise ValueError("Evidence directory must be under desktop/qualification/evidence")
    evidence_directory.mkdir(parents=True, exist_ok=True)

    record = json.loads(LEDGER.read_text())
    record["timestamp"] = datetime.now(timezone.utc).isoformat(timespec="seconds")
    record["environment"] = {
        "scope": "Managed SDK development qualification on this Linux host; scripted fixture/native X11 only, not an authenticated provider or physical-device claim.",
        "os": platform.system().lower(),
        "arch": platform.machine(),
        "sdk_id": manifest["sdk_id"],
        "sdk_manifest_sha256": manifest_digest,
    }

    environment = os.environ.copy()
    environment["SDK_BUNDLE"] = str(bundle)
    environment["M5_RENDER_EVIDENCE"] = str(evidence_directory)
    environment["M5_NATIVE_DATA"] = str(ROOT / "desktop" / "target" / "m5-native-app-data")
    resource_measurements = evidence_directory / "m5-resource-measurements.json"
    resource_measurements.unlink(missing_ok=True)
    environment["M5_RESOURCE_EVIDENCE"] = str(resource_measurements)
    sdk_identity = {
        "sdk_id": manifest["sdk_id"],
        "sdk_manifest_sha256": manifest_digest,
    }
    status_by_gate = {}
    groups = test_groups()
    for name, commands in groups.items():
        status, evidence = run_group(name, commands, evidence_directory, environment, sdk_identity)
        if name == "dev_resource_bounds_and_cleanup" and status == "pass":
            if not resource_measurements.is_file():
                raise ValueError("M5 resource test did not produce its measurements artifact")
            measurements = json.loads(resource_measurements.read_text())
            measurements["rustc_version"] = subprocess.run(
                ["rustc", "--version"], cwd=ROOT, check=True, text=True, capture_output=True
            ).stdout.strip()
            measurements["sdk_id"] = manifest["sdk_id"]
            measurements["sdk_manifest_sha256"] = manifest_digest
            resource_measurements.write_text(json.dumps(measurements, indent=2) + "\n")
            evidence.append(evidence_entry(resource_measurements))
        status_by_gate[name] = status
        record["gates"][name]["status"] = status
        record["gates"][name]["notes"] = (
            "Bounded test evidence recorded by the M5 runner." if status == "pass" else "One or more bounded M5 tests failed; inspect the current runner output."
        )
        record["gates"][name].pop("prerequisite", None)
        record["gates"][name]["evidence"] = evidence

    managed_name = "dev_managed_sdk_title_lookup"
    managed_command = command(
        "--manifest-path", "desktop/Cargo.toml", "-p", "fframes-studio", "--test", "real_sdk_selection",
        "--", "--ignored", "--exact", "generated_title_identity_geometry_and_source_anchor_survive_managed_render", "--nocapture",
    )
    managed_status, managed_evidence = run_group(
        managed_name, [managed_command], evidence_directory, environment, sdk_identity
    )
    if managed_status == "pass":
        for relative in (
            "m5-managed-title-frame/0.png",
            "m5-managed-title-strip.png",
            "m5-managed-inspect-summary.json",
        ):
            artifact = evidence_directory / relative
            if not artifact.is_file():
                raise ValueError(f"Managed-SDK CLI evidence is missing: {relative}")
            managed_evidence.append(evidence_entry(artifact))
    status_by_gate[managed_name] = managed_status
    record["gates"][managed_name]["status"] = managed_status
    record["gates"][managed_name]["notes"] = (
        "Generated starter title rendered and its explicit source anchor resolved against immutable Rust source."
        if managed_status == "pass"
        else "Managed-SDK title rendering or source-anchor lookup failed; inspect the current runner output."
    )
    record["gates"][managed_name].pop("prerequisite", None)
    record["gates"][managed_name]["evidence"] = managed_evidence

    native_name = "dev_native_linux_canvas"
    if args.skip_native:
        record["gates"][native_name]["status"] = "not_run"
        record["gates"][native_name]["notes"] = "Native UI check skipped by request; no native canvas claim is made."
        record["gates"][native_name]["evidence"] = []
        record["gates"][native_name]["prerequisite"] = "Run the ignored native managed-SDK selection test with Xvfb, xdotool, xwd and ffmpeg."
    else:
        native_command = command(
            "--manifest-path", "desktop/Cargo.toml", "-p", "fframes-studio", "--test", "x11_shell",
            "--", "--ignored", "the_native_shell_selects_the_managed_starter_title_and_keeps_rectangle_scope_nonsemantic",
            "--exact", "--nocapture",
        )
        environment["M5_NATIVE_EVIDENCE"] = str(evidence_directory)
        native_status, native_evidence = run_group(
            native_name, [native_command], evidence_directory, environment, sdk_identity
        )
        status_by_gate[native_name] = native_status
        for screenshot_name in ("m5-managed-title-selected.png", "m5-nonsemantic-rectangle-scope.png"):
            screenshot = evidence_directory / screenshot_name
            if native_status == "pass" and screenshot.is_file():
                native_evidence.append(evidence_entry(screenshot))
        record["gates"][native_name]["status"] = native_status
        record["gates"][native_name]["notes"] = (
            "Xvfb native input selected the measured managed-title bounds and exercised zoom, pan, cycling and rectangle scope. Screenshots are included as evidence."
            if native_status == "pass"
            else "Native managed-SDK canvas interaction failed; inspect the current runner output."
        )
        record["gates"][native_name].pop("prerequisite", None)
        record["gates"][native_name]["evidence"] = native_evidence

    development_gates = [gate for gate in record["gates"].values() if gate["kind"] == "development"]
    authentic_gates = [gate for gate in record["gates"].values() if gate["kind"] == "authentic"]
    development_pass = all(gate["status"] == "pass" for gate in development_gates)
    full_pass = development_pass and all(gate["status"] == "pass" for gate in authentic_gates)
    pending_development = [
        name for name, gate in record["gates"].items()
        if gate["kind"] == "development" and gate["status"] != "pass"
    ]
    record["acceptance"] = {
        "development": "pass" if development_pass else "not_run",
        "full": "pass" if full_pass else "not_run",
        "reason": (
            "Every development and authentic gate passed."
            if full_pass
            else (
                "Development gates remain pending: " + ", ".join(pending_development) + "."
                if pending_development
                else "M5 development gates pass. Full acceptance remains pending authentic provider, physical-device, Windows and macOS evidence."
            )
        ),
    }
    LEDGER.write_text(json.dumps(record, indent=2) + "\n")
    validation = subprocess.run([sys.executable, str(VALIDATOR), str(LEDGER)], cwd=ROOT, text=True)
    if validation.returncode != 0:
        return validation.returncode
    return 0 if all(status == "pass" for status in status_by_gate.values()) and not args.skip_native else 1


if __name__ == "__main__":
    try:
        sys.exit(run())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"M5 qualification runner: {error}", file=sys.stderr)
        sys.exit(2)
