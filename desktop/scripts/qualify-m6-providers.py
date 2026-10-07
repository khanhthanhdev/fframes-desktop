#!/usr/bin/env python3
"""Run bounded M6 development checks or inspect authentic-provider prerequisites.

Development checks use repository fixtures only and never modify the provider ledger. The
authentic mode is deliberately read-only: it reports whether the expected adapter executables
are discoverable, but does not launch them, authenticate, or claim qualification. Authentic
workflow evidence must be collected in a supervised, disposable provider session.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DESKTOP = ROOT / "desktop"
QUALIFICATION = DESKTOP / "qualification"
VALIDATOR_PATH = DESKTOP / "scripts" / "validate-qualification.py"
PROVIDERS = {
    "claude": "claude-agent-acp",
    "codex": "codex-acp",
    "pi": "pi-acp",
    "antigravity": "antigravity-acp",
}
DEFAULT_CHECKS = (
    "provider_handoff_app",
    "provider_handoff_engine",
    "provider_profiles",
    "agent_workflow_ui",
    "workflow_safety",
    "qualification_contract",
    "format",
)


def cargo_test(*arguments: str) -> list[str]:
    return ["cargo", "test", "--locked", "--manifest-path", "desktop/Cargo.toml", *arguments]


def checks() -> dict[str, list[list[str]]]:
    app = ["-p", "fframes-studio"]
    engine = ["-p", "studio-engine"]
    return {
        "provider_handoff_app": [cargo_test(*app, "--test", "provider_handoff")],
        "provider_handoff_engine": [cargo_test(*engine, "--test", "provider_handoff")],
        "provider_profiles": [cargo_test(*app, "--test", "provider_profiles")],
        "agent_workflow_ui": [cargo_test(*app, "--test", "agent_workflow_ui")],
        "workflow_safety": [
            cargo_test("-p", "studio-agent-spike", "--test", "acp_v1"),
            cargo_test(*app, "--test", "agent_workflow"),
            cargo_test(*app, "--test", "agent_tools"),
            cargo_test(*app, "--test", "scoped_editing"),
            cargo_test(*engine, "--test", "task_recovery"),
            cargo_test(*engine, "--test", "edit_transaction"),
        ],
        "qualification_contract": [
            [sys.executable, "desktop/scripts/test-qualification-m6.py"],
            [sys.executable, "desktop/scripts/validate-qualification.py", "desktop/qualification/m6-results.json"],
        ],
        "format": [
            ["cargo", "fmt", "--manifest-path", "desktop/Cargo.toml", "--all", "--", "--check"],
        ],
    }


def normalize_output(output: str) -> str:
    """Remove common credential shapes and machine-specific paths before saving logs."""
    sys.path.insert(0, str(VALIDATOR_PATH.parent))
    import importlib

    validator = importlib.import_module("validate-qualification")
    for _name, pattern in validator.SECRET_PATTERNS:
        output = pattern.sub("[REDACTED]", output)
    private_roots = sorted(
        {str(Path.home()), str(ROOT), str(Path(os.getenv("TMPDIR", "/tmp")).resolve())},
        key=len,
        reverse=True,
    )
    for root in private_roots:
        if root and root != "/":
            output = output.replace(root, "<private-path>")
    output = re.sub(r"(?<![A-Za-z0-9_.~<:/-])/(?:root|home|Users|tmp|var|mnt|private|srv|opt|run|media|workspaces?)(?:/[^\s:'\"]*)?", "<private-path>", output)
    return output[-1024 * 1024 :]


def group_exists(process_group: int) -> bool:
    try:
        os.killpg(process_group, 0)
    except ProcessLookupError:
        return False
    return True


def stop_owned_group(process: subprocess.Popen[bytes]) -> bool:
    """Stop only the session/process group created for one harness command."""
    if process.poll() is not None:
        return False
    process_group = process.pid
    if os.name == "nt":
        try:
            completed = subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                capture_output=True,
                timeout=5,
                check=False,
            )
            process.wait(timeout=3)
            return completed.returncode == 0 and process.poll() is not None
        except (OSError, subprocess.TimeoutExpired):
            return False
    try:
        os.killpg(process_group, signal.SIGTERM)
    except ProcessLookupError:
        return True
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        pass
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline and group_exists(process_group):
        time.sleep(0.05)
    if group_exists(process_group):
        try:
            os.killpg(process_group, signal.SIGKILL)
        except ProcessLookupError:
            return True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline and group_exists(process_group):
            time.sleep(0.05)
    return not group_exists(process_group)


def decode_output(value: str | bytes | None) -> str:
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value or ""


def run_check(name: str, commands: list[list[str]], out_dir: Path, timeout: int) -> dict[str, object]:
    print(f"\n== {name} ==", flush=True)
    command_records = []
    passed = failed = 0
    for index, command in enumerate(commands, start=1):
        print("$ " + " ".join(command), flush=True)
        environment = os.environ.copy()
        # Fixture runs must not inherit credentials that might cause an adapter to leave its
        # scripted route. Provider tools are not invoked by any development check.
        for key in tuple(environment):
            if re.search(r"(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|BEARER_TOKEN)$", key, re.IGNORECASE):
                environment.pop(key, None)
        try:
            process = subprocess.Popen(
                command,
                cwd=ROOT,
                env=environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                start_new_session=os.name != "nt",
                creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
            )
            try:
                stdout, _ = process.communicate(timeout=timeout)
                process_exited = process.returncode is not None
                output = normalize_output(decode_output(stdout))
            except subprocess.TimeoutExpired:
                cleanup_verified = stop_owned_group(process)
                try:
                    stdout, _ = process.communicate(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    stdout, _ = process.communicate()
                    cleanup_verified = False
                output = normalize_output(decode_output(stdout) + "\nTimed out.\n")
                log_path = out_dir / f"{name}-{index}.log"
                log_path.write_text(output, encoding="utf-8")
                failed += 1
                command_records.append({
                    "command": command,
                    "status": "timeout",
                    "exit_code": None,
                    "cleanup_verified": cleanup_verified,
                    "log": log_path.name,
                    "log_sha256": hashlib.sha256(log_path.read_bytes()).hexdigest(),
                })
                print(f"Timed out after {timeout}s; see {log_path}", flush=True)
                break
            log_path = out_dir / f"{name}-{index}.log"
            log_path.write_text(output, encoding="utf-8")
            result = "pass" if process.returncode == 0 and process_exited else "fail"
            if result == "pass":
                passed += 1
            else:
                failed += 1
            print(output[-6000:], end="" if output.endswith("\n") else "\n")
            command_records.append({
                "command": command,
                "status": result,
                "exit_code": process.returncode,
                "process_exited": process_exited,
                "log": log_path.name,
                "log_sha256": hashlib.sha256(log_path.read_bytes()).hexdigest(),
            })
        except OSError as error:
            output = f"Could not start command ({type(error).__name__}).\n"
            log_path = out_dir / f"{name}-{index}.log"
            log_path.write_text(output, encoding="utf-8")
            failed += 1
            command_records.append({
                "command": command,
                "status": "launch_error",
                "exit_code": None,
                "log": log_path.name,
                "log_sha256": hashlib.sha256(log_path.read_bytes()).hexdigest(),
            })
    return {
        "status": "pass" if failed == 0 and passed == len(commands) else "fail",
        "commands": command_records,
        "passed_commands": passed,
        "failed_commands": failed,
    }


def authentic_readiness(providers: list[str]) -> dict[str, dict[str, str]]:
    """Inspect PATH without executing a provider executable or touching account state."""
    observations = {}
    for provider in providers:
        executable = PROVIDERS[provider]
        discovered = shutil.which(executable)
        if discovered:
            observations[provider] = {
                "status": "not_run",
                "executable": executable,
                "detail": "Executable is discoverable; no authenticated handshake or workflow was run.",
            }
        else:
            observations[provider] = {
                "status": "blocked",
                "executable": executable,
                "detail": "Expected adapter executable is not discoverable on PATH; no provider was launched.",
            }
    return observations


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("development", "authentic"), default="development")
    parser.add_argument("--out", type=Path, help="new evidence directory (must not already exist)")
    parser.add_argument("--checks", help="comma-separated development checks; default is all")
    parser.add_argument("--provider", action="append", choices=tuple(PROVIDERS), help="authentic readiness provider (repeatable; default all)")
    parser.add_argument("--timeout", type=int, default=2400, help="timeout per development command in seconds")
    parser.add_argument("--list-checks", action="store_true", help="list development check names and exit")
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("--timeout must be positive")
    if args.checks and args.mode != "development":
        parser.error("--checks is only valid with --mode development")
    if args.provider and args.mode != "authentic":
        parser.error("--provider is only valid with --mode authentic")
    return args


def main() -> int:
    args = parse_args()
    available = checks()
    if args.list_checks:
        for name in available:
            print(name)
        print("\nauthentic readiness (PATH inspection only; no provider launch)")
        return 0

    selected = None
    if args.mode == "development":
        selected = DEFAULT_CHECKS if args.checks is None else tuple(part.strip() for part in args.checks.split(",") if part.strip())
        if not selected:
            print("Select at least one development check.", file=sys.stderr)
            return 2
        unknown = set(selected) - set(available)
        if unknown:
            print(f"Unknown development checks: {', '.join(sorted(unknown))}", file=sys.stderr)
            return 2

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out_dir = args.out or (QUALIFICATION / "evidence" / f"m6-{args.mode}-{stamp}")
    out_dir = out_dir if out_dir.is_absolute() else ROOT / out_dir
    if not out_dir.resolve().is_relative_to((QUALIFICATION / "evidence").resolve()):
        print("Output must be inside desktop/qualification/evidence/.", file=sys.stderr)
        return 2
    try:
        out_dir.mkdir(parents=True, exist_ok=False)
    except FileExistsError:
        print(f"Evidence directory already exists: {out_dir}", file=sys.stderr)
        return 2

    report: dict[str, object] = {
        "schema": "m6-run/1",
        "mode": args.mode,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "environment": {"os": platform.system(), "arch": platform.machine()},
        "ledger_updated": False,
    }
    exit_code = 0
    if args.mode == "development":
        results = {name: run_check(name, available[name], out_dir, args.timeout) for name in selected}
        report["checks"] = results
        report["development_checks_status"] = "pass" if all(item["status"] == "pass" for item in results.values()) else "fail"
        report["authentic_status"] = "not_run"
        report["notice"] = "Fixture/development evidence cannot qualify a real provider or recommendation."
        exit_code = 0 if report["development_checks_status"] == "pass" else 1
    else:
        providers = args.provider or list(PROVIDERS)
        report["providers"] = authentic_readiness(providers)
        report["authentic_status"] = "not_run"
        report["notice"] = "Readiness inventory only: no executable was launched, no credentials were read, and no gate or ledger status was changed."
        exit_code = 0

    report["finished_at"] = datetime.now(timezone.utc).isoformat()
    report_path = out_dir / "run.json"
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"\nWrote {report_path}")
    if args.mode == "development":
        print(f"Selected development checks: {report['development_checks_status']}; authentic qualification: not run.")
    else:
        for provider, observation in report["providers"].items():
            print(f"{provider}: {observation['status']} — {observation['detail']}")
        print("Authentic provider qualification: not run.")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
