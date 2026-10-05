#!/usr/bin/env python3
"""Linux M3 agent-transaction qualification harness.

Runs the repository-available FIXTURE workflow checks, records them as DEVELOPMENT evidence
under ``qualification/evidence/linux-m3-<date>/`` and (re)writes the hashed ledger
``qualification/m3-results.json``. It also records every AUTHENTIC gate. Authentic gates are
``not_run`` with the exact missing prerequisite; the single authentic observation this harness
can make truthfully is the real-adapter handshake, enabled only by ``--adapter``. Fixture
(development) evidence can never mark an authentic gate ``pass``; the ledger validator
(``validate-qualification.py``) rejects such a ledger as well.

Exact arguments (``--help`` prints the same list; every argument is optional)::

    qualify-m3-agent.py [--out DIR] [--date YYYY-MM-DD] [--ledger FILE] [--replace]
                        [--checks NAME[,NAME...]] [--list-checks] [--cargo-timeout SECONDS]
                        [--real-sdk-active DIR]
                        [--adapter PATH [--adapter-arg ARG ...] [--auth-env NAME ...]]

    --out DIR              evidence directory, under <ledger dir>/evidence (default:
                           <ledger dir>/evidence/linux-m3-<date>, i.e. qualification/evidence/...)
    --date YYYY-MM-DD      date used in the default directory name (default: today, local time)
    --ledger FILE          ledger to write (default qualification/m3-results.json)
    --replace              replace an existing evidence directory, but only one this harness
                           created (it holds the ``.m3-harness-evidence`` marker)
    --checks LIST          run only these checks (see --list-checks); the others are recorded
                           as not run for this invocation
    --list-checks          print the available checks and exit
    --cargo-timeout SECS   per-cargo-invocation timeout (default 2400)
    --real-sdk-active DIR  an installed SDK directory (``compatibility.json`` inside): records
                           its version and also runs the ignored real-SDK workflow tests
                           (real Cargo compile + real preview worker + SCRIPTED agent, still
                           development evidence) and, when ``Xvfb`` and ``xdotool`` are on PATH,
                           the production GPUI shell on a private Xvfb (``native_x11_shell``;
                           otherwise that gate is not_run naming the missing program)
    --adapter PATH         a REAL ACP adapter executable: runs the authentic handshake probe
                           (initialize, ACP v1 negotiation, auth readiness through a scratch
                           session, advertised capabilities). No prompt is sent and no edit is
                           made. Repository fixtures are refused.
    --adapter-arg ARG      one argument for the adapter (repeatable, in order)
    --auth-env NAME        NAME of an environment variable that carries the adapter's
                           credentials (repeatable). Only the NAME is recorded; the value is
                           forwarded to the adapter probe alone and is redacted from everything
                           the harness writes. It is removed from every fixture check.

What the harness guarantees
- Deterministic endpoint: (the ``native_x11_shell`` test additionally picks the first FREE X
  display number in :130-:199 and never reuses an existing ``/tmp/.X<n>-lock`` or socket.) The single-instance unix socket ``/tmp/fframes-m3-qualify-<uid>.sock``
  is the only fixed endpoint it uses. It refuses to start if that path exists (another run, or a
  stale socket the operator must remove). It binds no TCP port; the checks use private temporary
  directories and sockets.
- Ownership: every child runs in its own session and carries ``FFRAMES_M3_HARNESS_ID`` in its
  environment. Ownership is an identity, not a number: a process group is owned from its spawn
  until it is verified empty (then it is retired, so an unrelated group that later reuses the
  number is never considered), and a process is signalled only after its (pid, start time) is
  re-read and it is proven ours (the marker, or membership of the group of its still-live
  leader), through a pidfd where the kernel offers one. Leftovers are recorded and then
  stopped; anything else, including unmarked processes in a numerically matching group, is
  recorded as left alone. Nothing else on the host is touched.
- Artifact leases: one private work directory (``TMPDIR`` of the children) is created and
  removed at the end; leftovers per check are recorded.
- Redaction happens before serialization: values of ``--auth-env`` variables and the credential
  patterns shared with the validator are replaced by ``[REDACTED]`` in every log and JSON file,
  and absolute private paths are normalized to placeholders (``<repo>``, ``<home>``,
  ``<sdk-active>``, ``<work>``, ``<evidence>``, ``<tmp>``, anything else ``<private-path>``)
  so evidence never names this machine's layout; hashes are taken over the normalized bytes.
  Environment variable values are never written (only names); prompts are never recorded.
- The real-adapter probe record follows the ``m3-authentic/1`` contract (``validate-qualification.py``):
  bound to its gate, platform and the adapter's exact launch identity. Nothing else here can
  produce authentic evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

DESKTOP = Path(__file__).resolve().parents[1]
ROOT = DESKTOP.parent
MARKER_FILE = ".m3-harness-evidence"
ENV_MARKER = "FFRAMES_M3_HARNESS_ID"
MAX_LOG_BYTES = 1024 * 1024
FIXTURE_AGENT_NAMES = {"scripted-agent", "protocol-peer", "acp-peer", "fixture", "test-agent"}
FIXTURE_TEXT_MARKERS = ("TEST FIXTURE ONLY", "Scripted ACP", "ACP v1 peer fixture")


class HarnessError(RuntimeError):
    pass


def load_validator():
    spec = importlib.util.spec_from_file_location("validate_qualification", Path(__file__).with_name("validate-qualification.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


VALIDATOR = load_validator()


# ---- redaction -----------------------------------------------------------------------------------------------------


class Redactor:
    """Replaces configured credential values, credential-shaped text and private absolute
    paths before serialization.

    Paths: known roots (the repository, the evidence and work directories, the SDK, the
    user's home, the toolchain homes, the temporary directory) become explicit placeholders;
    anything else that still looks like a private absolute path becomes ``<private-path>``.
    Evidence therefore never names this machine's private layout, and hashes are taken over
    the normalized bytes."""

    PRIVATE_TOKEN = re.compile("(?:" + VALIDATOR.PRIVATE_PATH.pattern + r")[^\s\"'`<>|;,)\]}]*")

    def __init__(self, secrets: list[str], roots: list[tuple[Path | str, str]] | None = None):
        self.secrets = sorted({s for s in secrets if s}, key=len, reverse=True)
        self.roots: list[tuple[str, str]] = []
        self.add_roots(roots or [])

    def add_roots(self, roots: list[tuple[Path | str, str]]) -> None:
        known = {prefix for prefix, _ in self.roots}
        for root, placeholder in roots:
            for variant in {str(root), str(Path(root).resolve())}:
                if len(variant) > 1 and variant not in known:
                    self.roots.append((variant.rstrip("/"), placeholder))
                    known.add(variant)
        # Longest first: a nested root (the SDK inside the repository) wins over its parent.
        self.roots.sort(key=lambda item: len(item[0]), reverse=True)

    def text(self, value: str) -> str:
        for secret in self.secrets:
            value = value.replace(secret, "[REDACTED]")
        for _, pattern in VALIDATOR.SECRET_PATTERNS:
            value = pattern.sub("[REDACTED]", value)
        for prefix, placeholder in self.roots:
            value = value.replace(prefix, placeholder)
        return self.PRIVATE_TOKEN.sub("<private-path>", value)

    def json(self, value):
        if isinstance(value, str):
            return self.text(value)
        if isinstance(value, list):
            return [self.json(item) for item in value]
        if isinstance(value, dict):
            return {self.text(str(key)): self.json(item) for key, item in value.items()}
        return value


def path_roots(sdk_dir: Path | None) -> list[tuple[Path | str, str]]:
    """The placeholders for this machine's roots (see [`Redactor`])."""
    home = Path.home()
    roots: list[tuple[Path | str, str]] = []
    if sdk_dir is not None:
        roots.append((sdk_dir, "<sdk-active>"))
        if len(sdk_dir.parents) >= 3:  # <home>/.fframes/sdk/active: the SDK's own home directory
            roots.append((sdk_dir.parents[2], "<sdk-home>"))
    roots += [
        (ROOT, "<repo>"),
        (os.environ.get("CARGO_HOME") or home / ".cargo", "<cargo-home>"),
        (os.environ.get("RUSTUP_HOME") or home / ".rustup", "<rustup-home>"),
        (tempfile.gettempdir(), "<tmp>"),
        (home, "<home>"),
    ]
    return roots


# ---- owned processes, endpoints and artifacts -----------------------------------------------------------------------------


@dataclass(frozen=True)
class Identity:
    """A process as the kernel knows it: a PID alone can be reused, a PID with the start time
    it had when it was observed cannot."""

    pid: int
    starttime: int


class ProcFs:
    """Process-table reads and the one way a signal is ever sent. Injectable: tests substitute
    a table in which identifiers are reused."""

    def __init__(self, root: Path = Path("/proc")):
        self.root = root

    def pids(self) -> list[int]:
        return [int(entry.name) for entry in self.root.iterdir() if entry.name.isdigit()]

    def stat(self, pid: int) -> tuple[str, int, int, int] | None:
        """(state, ppid, pgrp, starttime) or None when the process is gone."""
        try:
            text = (self.root / str(pid) / "stat").read_text()
            # pid (comm) state ppid pgrp ...; comm may contain spaces/parens: split after the last ")".
            fields = text.rsplit(")", 1)[1].split()
            return fields[0], int(fields[1]), int(fields[2]), int(fields[19])
        except (OSError, IndexError, ValueError):
            return None

    def identity(self, pid: int) -> Identity | None:
        stat = self.stat(pid)
        return Identity(pid, stat[3]) if stat else None

    def has_marker(self, pid: int, marker: bytes) -> bool:
        try:
            return marker in (self.root / str(pid) / "environ").read_bytes().split(b"\0")
        except OSError:
            return False

    def command(self, pid: int) -> str:
        try:
            return (self.root / str(pid) / "cmdline").read_bytes().rstrip(b"\0").replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            return ""

    def members(self, pgid: int) -> list[Identity]:
        """Live (non-zombie) members of a process group, with their identities."""
        found = []
        for pid in self.pids():
            stat = self.stat(pid)
            if stat and stat[0] != "Z" and stat[2] == pgid:
                found.append(Identity(pid, stat[3]))
        return found

    def pin(self, pid: int):
        """A pidfd for `pid` where the kernel offers one (it pins the process between the
        identity check and the signal), else None."""
        try:
            return os.pidfd_open(pid)
        except (AttributeError, OSError):
            return None

    def deliver(self, pid: int, sig: int, pinned) -> bool:
        try:
            if pinned is not None:
                signal.pidfd_send_signal(pinned, sig)
            else:
                os.kill(pid, sig)
        except (ProcessLookupError, PermissionError):
            return False
        return True

    def signal(self, target: Identity, sig: int, marker: bytes, leader: Identity | None = None) -> bool:
        """Signals `target` only if, right now, that very process (same start time) is still
        there and is ours: it carries the harness marker, or it is a member of the process
        group of the verified-live `leader`. Returns whether a signal was delivered."""
        pinned = self.pin(target.pid)
        try:
            stat = self.stat(target.pid)
            if stat is None or stat[3] != target.starttime:
                return False
            proven = self.has_marker(target.pid, marker)
            if not proven and leader is not None:
                proven = self.identity(leader.pid) == leader and stat[2] == leader.pid
            return proven and self.deliver(target.pid, sig, pinned)
        finally:
            if pinned is not None:
                os.close(pinned)


PROCESSES = ProcFs()


def process_group_members(pgid: int) -> list[int]:
    return [member.pid for member in PROCESSES.members(pgid)]


class Owned:
    """Every process the harness started, and the only ones it may ever signal.

    Ownership is an identity, never a number: a group is owned from its spawn until it has
    been verified empty (then it is retired, so a later unrelated group that reuses the
    number can never be touched), and a single process is signalled only after its
    (pid, start time) is re-read and it is proven ours (the harness marker in its environment,
    or membership of the group of a leader still alive with the start time recorded at
    spawn). Anything else found in a numerically matching group is recorded as
    ``unowned_members`` and left alone."""

    def __init__(self, harness_id: str, redactor: Redactor, table: ProcFs | None = None):
        self.harness_id = harness_id
        self.marker = f"{ENV_MARKER}={harness_id}".encode()
        self.redactor = redactor
        self.table = table or PROCESSES
        self.history: list[dict] = []
        # pgid -> identity of the group leader as spawned (None: it had already exited).
        self.groups: dict[int, Identity | None] = {}
        self.stopped_leftovers: list[dict] = []
        self.unowned_members: list[dict] = []

    def environment(self, base: dict[str, str]) -> dict[str, str]:
        env = dict(base)
        env[ENV_MARKER] = self.harness_id
        return env

    def adopt(self, pgid: int) -> None:
        """Registers the group `pgid` that this harness just created (its leader is `pgid`)."""
        self.groups[pgid] = self.table.identity(pgid)

    def run(self, label: str, argv: list[str], *, cwd: Path, env: dict[str, str], output: Path, timeout: float) -> dict:
        record = {"label": label, "argv": [self.redactor.text(a) for a in argv], "cwd": self.redactor.text(str(cwd)), "started": now()}
        began = time.monotonic()
        with output.open("wb") as sink:
            process = subprocess.Popen(argv, cwd=cwd, env=self.owned_env(env), stdout=sink, stderr=subprocess.STDOUT, start_new_session=True)
            self.adopt(process.pid)
            record["pid"] = process.pid
            try:
                record["returncode"] = process.wait(timeout=timeout)
                record["timed_out"] = False
            except subprocess.TimeoutExpired:
                record["timed_out"] = True
                self.stop_group(process.pid)
                record["returncode"] = process.wait()
        record["seconds"] = round(time.monotonic() - began, 3)
        record["leftovers"] = self.reap_leftovers(process.pid)
        self.history.append(record)
        return record

    def owned_env(self, env: dict[str, str]) -> dict[str, str]:
        return self.environment(env)

    def owned_members(self, pgid: int) -> list[Identity]:
        """Live members of an owned group that are proven ours; the rest are recorded."""
        leader = self.groups.get(pgid)
        proven = []
        for member in self.table.members(pgid):
            stat = self.table.stat(member.pid)
            if stat is None or stat[3] != member.starttime:
                continue
            if self.table.has_marker(member.pid, self.marker) or (
                leader is not None and self.table.identity(leader.pid) == leader
            ):
                proven.append(member)
            elif not any(item["pid"] == member.pid and item["pgid"] == pgid for item in self.unowned_members):
                self.unowned_members.append({"pid": member.pid, "pgid": pgid, "command": self.redactor.text(self.table.command(member.pid))[:200]})
        return proven

    def retire_if_empty(self, pgid: int) -> bool:
        """Forgets a group once it has no live member at all: its number is not ours any more."""
        if pgid in self.groups and not self.table.members(pgid):
            del self.groups[pgid]
            return True
        return False

    def stop_group(self, pgid: int) -> None:
        """SIGTERM then SIGKILL every proven-ours member of a group the harness created, each
        by identity (never the group as a whole, never a member that is not ours)."""
        if pgid not in self.groups:
            raise HarnessError(f"refusing to signal process group {pgid}: not harness-owned")
        leader = self.groups[pgid]
        for sig, grace in ((signal.SIGTERM, 3.0), (signal.SIGKILL, 5.0)):
            deadline = time.monotonic() + grace
            while True:
                members = self.owned_members(pgid)
                if not members:
                    break
                for member in members:
                    self.table.signal(member, sig, self.marker, leader)
                if time.monotonic() >= deadline:
                    break
                time.sleep(0.05)
            if not self.owned_members(pgid):
                break
        self.retire_if_empty(pgid)

    def marked_identities(self) -> list[Identity]:
        """Live processes that inherited the harness marker (including setsid escapees)."""
        found = []
        for pid in self.table.pids():
            if pid == os.getpid() or not self.table.has_marker(pid, self.marker):
                continue
            stat = self.table.stat(pid)
            if stat and stat[0] != "Z":
                found.append(Identity(pid, stat[3]))
        return found

    def marked_pids(self) -> list[int]:
        return [identity.pid for identity in self.marked_identities()]

    def reap_leftovers(self, pgid: int) -> list[dict]:
        """Stop anything of ours that outlived its command and report it. Only identities that
        are proven ours are signalled, each re-validated immediately before its signal."""
        leader = self.groups.get(pgid)
        targets = {identity: None for identity in self.owned_members(pgid)}
        targets.update({identity: None for identity in self.marked_identities()})
        leftovers = []
        for identity in sorted(targets, key=lambda i: i.pid):
            leftovers.append({"pid": identity.pid, "command": self.redactor.text(self.table.command(identity.pid))[:200]})
            self.table.signal(identity, signal.SIGKILL, self.marker, leader)
        if leftovers:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and any(self.table.identity(i.pid) == i and not self.is_zombie(i.pid) for i in targets):
                time.sleep(0.05)
            self.stopped_leftovers.extend(leftovers)
        self.retire_if_empty(pgid)
        return leftovers

    def is_zombie(self, pid: int) -> bool:
        stat = self.table.stat(pid)
        return stat is None or stat[0] == "Z"

    def shutdown(self) -> list[int]:
        for pgid in sorted(self.groups):
            self.stop_group(pgid)
        for identity in self.marked_identities():
            self.table.signal(identity, signal.SIGKILL, self.marker)
        time.sleep(0.1)
        for pgid in sorted(self.groups):
            self.retire_if_empty(pgid)
        return self.marked_pids()


def is_zombie(pid: int) -> bool:
    stat = PROCESSES.stat(pid)
    return stat is None or stat[0] == "Z"


class EndpointGuard:
    """The harness's only fixed endpoint: a unix socket proving a single running instance."""

    def __init__(self):
        self.path = Path(f"/tmp/fframes-m3-qualify-{os.getuid()}.sock")
        self.sock: socket.socket | None = None

    def acquire(self) -> None:
        if os.path.lexists(self.path):
            raise HarnessError(f"endpoint {self.path} is occupied (another harness run, or a stale socket): refusing to start; remove it yourself once you have verified that no run is active")
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            self.sock.bind(str(self.path))
        except OSError as error:
            self.sock.close()
            self.sock = None
            raise HarnessError(f"cannot claim endpoint {self.path}: {error}") from error
        os.chmod(self.path, 0o600)
        self.sock.listen(1)

    def release(self) -> None:
        if self.sock is not None:
            self.sock.close()
            self.sock = None
            try:
                self.path.unlink()
            except FileNotFoundError:
                pass


# ---- small helpers ---------------------------------------------------------------------------------------------------------------------


def now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def sha256_file(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def capture(argv: list[str], cwd: Path = ROOT) -> str | None:
    try:
        done = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return done.stdout.strip() if done.returncode == 0 else None


def mount_of(path: Path) -> dict | None:
    best = None
    resolved = str(path.resolve())
    try:
        table = Path("/proc/self/mountinfo").read_text().splitlines()
    except OSError:
        return None
    for line in table:
        left, _, right = line.partition(" - ")
        point = left.split(" ")[4]
        kind, source = right.split(" ")[:2]
        if resolved == point or resolved.startswith(point.rstrip("/") + "/"):
            if best is None or len(point) >= len(best["mount_point"]):
                best = {"mount_point": point, "type": kind, "source": source}
    return best


def os_release() -> str | None:
    try:
        for line in Path("/etc/os-release").read_text().splitlines():
            if line.startswith("PRETTY_NAME="):
                return line.split("=", 1)[1].strip('"')
    except OSError:
        pass
    return None


def mem_total_kib() -> int | None:
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1])
    except (OSError, ValueError):
        pass
    return None


def locked_package_version(name: str) -> str | None:
    try:
        text = (DESKTOP / "Cargo.lock").read_text()
    except OSError:
        return None
    match = re.search(rf'\[\[package\]\]\nname = "{re.escape(name)}"\nversion = "([^"]+)"', text)
    return match.group(1) if match else None


def tree_fingerprint() -> tuple[str | None, int | None]:
    """A digest of the working tree relative to HEAD (the commit alone does not name a dirty tree)."""
    diff = subprocess.run(["git", "diff", "HEAD", "--binary", "--", ".", ":(exclude)desktop/qualification"], cwd=ROOT, capture_output=True)
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", "-z", "--", ".", ":(exclude)desktop/qualification"], cwd=ROOT, capture_output=True
    )
    if diff.returncode != 0 or untracked.returncode != 0:
        return None, None
    digest = hashlib.sha256(diff.stdout)
    count = 0
    for name in sorted(filter(None, untracked.stdout.split(b"\0"))):
        path = ROOT / name.decode(errors="replace")
        if path.is_file() and not path.is_symlink():
            digest.update(name + b"\0" + sha256_file(path).encode())
            count += 1
    status = capture(["git", "status", "--porcelain", "--", ".", ":(exclude)desktop/qualification"])
    return digest.hexdigest(), len(status.splitlines()) if status is not None else None


def environment_record(args: argparse.Namespace, out: Path, sdk: dict, adapter: dict | None, redactor: Redactor, argv: list[str]) -> dict:
    rustc = capture(["rustc", "-vV"], DESKTOP) or ""
    host = next((line.split(": ", 1)[1] for line in rustc.splitlines() if line.startswith("host: ")), None)
    fingerprint, dirty = tree_fingerprint()
    uname = platform.uname()
    channel = None
    try:
        channel = re.search(r'channel = "([^"]+)"', (DESKTOP / "rust-toolchain.toml").read_text()).group(1)
    except (OSError, AttributeError):
        pass
    return {
        "scope": (
            "Linux software-only development run: scripted ACP peer, fake SDK tree/preview worker and the engine's real controller "
            "(plus an installed SDK only when --real-sdk-active is given). Authentic gates need a real authenticated adapter, "
            "physical devices or other operating systems and are recorded separately."
        ),
        "os": {
            "system": uname.system,
            "release": uname.release,
            "version": uname.version,
            "machine": uname.machine,
            "distribution": os_release(),
            "cpu_count": os.cpu_count(),
            "memory_kib": mem_total_kib(),
            "evidence_filesystem": mount_of(out.parent if not out.exists() else out),
        },
        "toolchain": {
            "rustc": rustc.splitlines()[0] if rustc else None,
            "rustc_commit": next((l.split(": ", 1)[1] for l in rustc.splitlines() if l.startswith("commit-hash: ")), None),
            "host": host,
            "cargo": capture(["cargo", "-V"], DESKTOP),
            "pinned_channel": channel,
            "xvfb": shutil.which("Xvfb"),
            "xdotool": shutil.which("xdotool"),
            "python": platform.python_version(),
            "acp_sdk_crate": locked_package_version("agent-client-protocol"),
            "cargo_lock_sha256": sha256_file(DESKTOP / "Cargo.lock") if (DESKTOP / "Cargo.lock").exists() else None,
        },
        "git": {
            "commit": capture(["git", "rev-parse", "HEAD"]),
            "branch": capture(["git", "rev-parse", "--abbrev-ref", "HEAD"]),
            "dirty_paths": dirty,
            "tree_fingerprint_sha256": fingerprint,
            "fingerprint_note": "digest of `git diff HEAD` plus untracked files, excluding desktop/qualification",
        },
        "sdk": sdk,
        "adapter": adapter,
        "harness": {
            "script": "desktop/scripts/qualify-m3-agent.py",
            "arguments": [redactor.text(a) for a in argv],
            "auth_env_names": list(args.auth_env),
            "started": now(),
        },
    }


def sdk_record(directory: Path | None) -> dict:
    if directory is None:
        return {"provided": False, "note": "no installed SDK given: the fixture checks use a fake SDK tree; real-SDK tests were not run"}
    manifest = directory / "compatibility.json"
    try:
        data = json.loads(manifest.read_text())
    except (OSError, ValueError) as error:
        raise HarnessError(f"--real-sdk-active {directory}: unreadable compatibility.json ({error})") from error
    toolchain = data.get("rust_toolchain")
    return {
        "provided": True,
        "directory_name": directory.name,
        "sdk_id": data.get("sdk_id"),
        "fframes_version": data.get("fframes_version"),
        "cargo_fframes_version": data.get("cargo_fframes_version"),
        "target_triple": data.get("target_triple"),
        "rust_toolchain": toolchain.get("channel") if isinstance(toolchain, dict) else toolchain,
        "ffmpeg_tag": (data.get("ffmpeg") or {}).get("tag"),
        "compatibility_sha256": sha256_file(manifest),
    }


# ---- checks ----------------------------------------------------------------------------------------------------------------------------------


@dataclass
class Check:
    name: str
    gates: list[str]
    argv: list[str]
    targets: list[str]
    description: str
    needs_file: str | None = None
    needs_sdk: bool = False
    needs_tools: list[str] = field(default_factory=list)
    measurement_files: list[str] = field(default_factory=list)
    measurement_env: bool = False
    env: dict[str, str] = field(default_factory=dict)


def cargo_test(package: str, tests: list[str], tail: list[str] | None = None) -> list[str]:
    argv = ["cargo", "test", "--locked", "-p", package]
    for name in tests:
        argv += ["--test", name]
    return argv + (["--"] + tail if tail else [])


CHECKS = [
    Check(
        "acp_transport", ["dev_acp_transport"], cargo_test("studio-agent-spike", ["acp_v1", "acp_v1_review"]),
        ["tests/acp_v1.rs", "tests/acp_v1_review.rs"], "ACP v1 driver against the scripted peer (transport only)",
    ),
    Check(
        "engine_transactions", ["dev_engine_transactions"],
        cargo_test("studio-engine", ["agent_task", "candidate_validation", "edit_transaction", "promotion_install", "review_history", "review_publication", "task_recovery"]),
        [f"tests/{n}.rs" for n in ("agent_task", "candidate_validation", "edit_transaction", "promotion_install", "review_history", "review_publication", "task_recovery")],
        "engine task, candidate validation, recoverable Apply/Undo and crash recovery",
    ),
    Check(
        "app_workflow", ["dev_workflow_fixture"], cargo_test("fframes-studio", ["agent_workflow"]), ["tests/agent_workflow.rs"],
        "AgentWorkflow against the scripted peer, fake preview worker and real controller",
    ),
    Check(
        "app_tools_validation", ["dev_tools_and_validation"],
        cargo_test("fframes-studio", ["agent_tools", "build_sharing", "candidate_runner", "promotion_handoff"]),
        [f"tests/{n}.rs" for n in ("agent_tools", "build_sharing", "candidate_runner", "promotion_handoff")],
        "project tools broker/CLI/MCP, shared build service, candidate validation and preview handoff",
    ),
    Check(
        "app_ui_workflow", ["dev_native_ui_workflow"], cargo_test("fframes-studio", ["agent_workflow_ui"]), ["tests/agent_workflow_ui.rs"],
        "native conversation panel / shell workflow test", needs_file="app/tests/agent_workflow_ui.rs",
    ),
    Check(
        "dev_evidence",
        ["dev_resource_bounds", "dev_compiler_count", "dev_cleanup_cycles", "dev_cli_mcp_parity", "dev_publication_primitive"],
        cargo_test("fframes-studio", ["m3_development_evidence"], ["--nocapture"]), ["tests/m3_development_evidence.rs"],
        "resource bounds, compiler count, 20+20 cleanup cycles, CLI/MCP parity, publication primitive",
        measurement_files=["resource-bounds.json", "compiler-count.json", "cycles.json", "cli-mcp-parity.json", "publication-primitive.json", "summary.json"],
        measurement_env=True,
    ),
    Check(
        "real_sdk_workflow", ["dev_real_sdk_workflow"],
        cargo_test("fframes-studio", ["agent_workflow", "real_sdk_promotion"], ["--ignored", "real_sdk"]),
        ["tests/agent_workflow.rs", "tests/real_sdk_promotion.rs"],
        "real Cargo compile + real preview worker with a SCRIPTED agent (ignored tests)", needs_sdk=True,
    ),
    Check(
        "native_x11_shell", ["dev_native_x11_shell"],
        cargo_test("fframes-studio", ["x11_shell"], ["--ignored", "--nocapture"]), ["tests/x11_shell.rs"],
        "production GPUI shell as a real process on an owned Xvfb (xdotool input, SCRIPTED agent, real SDK, software rendering)",
        needs_file="app/tests/x11_shell.rs", needs_sdk=True, needs_tools=["Xvfb", "xdotool"],
    ),
]
CHECKS_BY_NAME = {c.name: c for c in CHECKS}

RESULT_LINE = re.compile(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored")
RUNNING_LINE = re.compile(r"^\s*Running (\S+)")


def parse_cargo_output(text: str) -> dict:
    current = None
    results = []
    for line in text.splitlines():
        running = RUNNING_LINE.match(line)
        if running:
            current = running.group(1)
            continue
        found = RESULT_LINE.search(line)
        if found:
            results.append(
                {"target": current, "outcome": found.group(1), "passed": int(found.group(2)), "failed": int(found.group(3)), "ignored": int(found.group(4))}
            )
    return {"results": results}


def bounded_log(raw: str) -> str:
    data = raw.encode(errors="replace")
    if len(data) <= MAX_LOG_BYTES:
        return raw
    head, tail = data[: MAX_LOG_BYTES // 4], data[-(MAX_LOG_BYTES * 3 // 4) :]
    note = f"\n[... {len(data) - len(head) - len(tail)} bytes of the middle omitted by the harness ...]\n"
    return head.decode(errors="ignore") + note + tail.decode(errors="ignore")


@dataclass
class CheckResult:
    check: Check
    status: str  # pass | fail | not_run
    reason: str
    log: str | None = None
    record: dict | None = None
    parsed: dict | None = None
    prerequisite: str | None = None


class Runner:
    def __init__(self, args, out: Path, work: Path, owned: Owned, redactor: Redactor, base_env: dict[str, str]):
        self.args, self.out, self.work, self.owned, self.redactor, self.base_env = args, out, work, owned, redactor, base_env
        self.leftovers: list[dict] = []

    def run(self, check: Check, selected: bool, sdk: Path | None) -> CheckResult:
        if not selected:
            return CheckResult(
                check, "not_run", f"check `{check.name}` was not selected in this invocation (--checks)",
                prerequisite=f"Include `{check.name}` in --checks (or omit --checks) and rerun the harness; this invocation did not select it.",
            )
        if check.needs_file and not (DESKTOP / check.needs_file).exists():
            return CheckResult(
                check, "not_run", f"test target {check.needs_file} does not exist in this tree",
                prerequisite=f"The test target {check.needs_file} must exist and pass; it was absent from this tree when the harness ran.",
            )
        if check.needs_sdk and sdk is None:
            return CheckResult(
                check, "not_run", "no installed SDK was given (--real-sdk-active DIR)",
                prerequisite="An installed SDK directory (compatibility.json inside) passed as --real-sdk-active DIR; none was given for this run.",
            )
        missing_tools = [tool for tool in check.needs_tools if shutil.which(tool) is None]
        if missing_tools:
            return CheckResult(
                check, "not_run", f"required program(s) not found on PATH: {', '.join(missing_tools)}",
                prerequisite=f"Install {', '.join(missing_tools)} (the test drives a private X server) and rerun the harness with --real-sdk-active DIR.",
            )
        env = dict(self.base_env)
        env["TMPDIR"] = str(self.work)
        env.update(check.env)
        if check.needs_sdk:
            env["SDK_ACTIVE"] = str(sdk)
        if check.measurement_env:
            env["FFRAMES_M3_EVIDENCE_OUT"] = str(self.out)
        raw = self.work / f"{check.name}.raw"
        record = self.owned.run(check.name, check.argv, cwd=DESKTOP, env=env, output=raw, timeout=self.args.cargo_timeout)
        text = raw.read_text(errors="replace")
        raw.unlink()
        leftover_work = sorted(p.name for p in self.work.iterdir())
        for name in leftover_work:
            target = self.work / name
            shutil.rmtree(target, ignore_errors=True) if target.is_dir() else target.unlink(missing_ok=True)
        record["work_directory_leftovers"] = leftover_work
        log_path = self.out / "logs" / f"{check.name}.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        header = f"$ {' '.join(record['argv'])}\n# cwd: desktop   returncode: {record['returncode']}   timed_out: {record['timed_out']}   seconds: {record['seconds']}\n\n"
        log_path.write_text(self.redactor.text(header + bounded_log(text)))
        parsed = parse_cargo_output(text)
        parsed["compile_failed"] = bool(re.search(r"^error(\[E\d+\])?:", text, re.M)) and not parsed["results"]
        ran = {Path(r["target"]).as_posix() for r in parsed["results"] if r["target"]}
        missing = [t for t in check.targets if not any(item.endswith(t) for item in ran)]
        failed = [r for r in parsed["results"] if r["outcome"] != "ok" or r["failed"]]
        passed = sum(r["passed"] for r in parsed["results"])
        if record["timed_out"]:
            status, reason = "fail", f"timed out after {self.args.cargo_timeout}s"
        elif record["returncode"] != 0:
            status, reason = "fail", f"cargo exited {record['returncode']}" + (f"; failing targets: {[r['target'] for r in failed]}" if failed else "")
        elif failed or missing or passed == 0:
            status, reason = "fail", f"missing targets {missing}" if missing else "no passing tests recorded"
        else:
            status, reason = "pass", f"{passed} tests passed in {len(parsed['results'])} targets"
        parsed["passed_total"] = passed
        return CheckResult(check, status, reason, f"logs/{check.name}.log", record, parsed)


# ---- measurement-derived gates ------------------------------------------------------------------------------------------------------


def load_measurement(out: Path, name: str) -> dict | None:
    try:
        return json.loads((out / name).read_text())
    except (OSError, ValueError):
        return None


def normalize_measurements(out: Path, redactor: Redactor) -> list[str]:
    """Measurement files are written by the Rust tests straight into the evidence directory,
    outside the harness's redaction. Rewrite each one through the redactor (credential
    patterns, private absolute paths) before anything is judged or hashed."""
    rewritten = []
    for path in sorted(out.rglob("*.json")):
        try:
            value = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        clean = redactor.json(value)
        if clean != value:
            path.write_text(json.dumps(clean, indent=2, sort_keys=True) + "\n")
            rewritten.append(path.relative_to(out).as_posix())
    return rewritten


def judge_measurements(out: Path, ran: CheckResult) -> dict[str, tuple[str, str, list[str]]]:
    """gate -> (status, note, evidence files) from the Rust test's JSON measurements."""
    log = [ran.log] if ran.log else []
    gates: dict[str, tuple[str, str, list[str]]] = {}
    if ran.status != "pass":
        why = f"the measurement test did not pass ({ran.reason})"
        status = "not_run" if ran.status == "not_run" else "fail"
        for gate in ran.check.gates:
            gates[gate] = (status, why, log)
        return gates

    def need(file: str):
        return load_measurement(out, file)

    bounds, compiler, cycles, parity, primitive = (
        need("resource-bounds.json"), need("compiler-count.json"), need("cycles.json"), need("cli-mcp-parity.json"), need("publication-primitive.json")
    )
    full = all(m and m.get("profile") == "full" for m in (bounds, cycles))

    def verdict(gate, ok, good, bad, files):
        gates[gate] = ("pass" if ok else "fail", good if ok else bad, files + log)

    if bounds:
        rows, size = bounds["resident_rows"], bounds["resident_bytes"]
        transcript, queue = bounds["transcript"], bounds["event_queue"]
        ok = (
            full and rows["within"] and size["within_limit_plus_newest_row"] and transcript["older_rows_paged_from_disk"]
            and transcript["rows_persisted_total"] > rows["limit"] and transcript["text_bytes_total"] > size["limit"]
            and queue["task_phase"] == "Accepted" and queue["queue_overflow_failure"] is False and queue["events_scripted"] > queue["driver_queue_limit"]
            and all(v == 0 for k, v in bounds["cleanup"]["ownership"].items() if k not in ("resident_rows", "build_cached_entries"))
        )
        verdict(
            "dev_resource_bounds", ok,
            f"Streamed {transcript['rows_persisted_total']} rows ({transcript['text_bytes_total']} text bytes, {queue['events_scripted']} scripted events against a {queue['driver_queue_limit']}-event queue); "
            f"peak resident {rows['observed_peak']}/{rows['limit']} rows and {size['observed_peak']}/{size['limit']} bytes (rule: at most one newest row over the byte budget); older rows paged from disk; no queue overflow; nothing owned after close. "
            "Fixture scripted peer only; playback/scrub concurrency and GPU layout cost were not measured.",
            "resource bounds exceeded or the long transcript did not exercise them", ["resource-bounds.json", "summary.json"],
        )
    else:
        gates["dev_resource_bounds"] = ("fail", "resource-bounds.json missing", log)
    if compiler and cycles:
        accepted = [c for c in cycles["cycles"] if c["kind"] == "accepted_edit"]
        ok = (
            compiler["equal_key"]["compiles_started"] == 1 and compiler["equal_key"]["one_compile_per_equal_key"]
            and compiler["distinct_key"]["compiles_started_total"] == 2 and accepted and all(c["compiles_started_in_cycle"] == 1 for c in accepted)
            and compiler["after_project_close"]["cached_entries"] == 0
        )
        verdict(
            "dev_compiler_count", ok,
            f"{compiler['equal_key']['subscribers']} equal-key requests (UI, tool, validation, tool, cache hit) started {compiler['equal_key']['compiles_started']} compile "
            f"({compiler['equal_key']['joins']} joins); a different revision started exactly one more; each of {len(accepted)} accepted edits compiled once through the shared service ({cycles['compiles_started_total']} compiles in all cycles, repair attempts included).",
            "more than one compile per equal build key, or a missing measurement", ["compiler-count.json", "cycles.json"],
        )
    else:
        gates["dev_compiler_count"] = ("fail", "compiler-count.json or cycles.json missing", log)
    if cycles:
        totals = cycles["totals"]
        after = cycles["after_close"]
        clean = lambda view: all(view[k] == 0 for k in ("owned_processes", "workflow_owned_processes", "broker_grants", "tool_workers", "capability_files", "build_leased_entries", "build_in_flight"))
        ok = (
            full and totals["accepted"] >= 20 and totals["failed"] + totals["cancelled"] >= 20 and all(clean(c["ownership_after_cycle"]) for c in cycles["cycles"])
            and clean(after["ownership"]) and after["adapter_pids_alive"] == [] and after["build_cached_entries"] == 0 and after["runtime_dir_entries"] == 0
        )
        verdict(
            "dev_cleanup_cycles", ok,
            f"{totals['accepted']} accepted edits and {totals['failed']} failed + {totals['cancelled']} cancelled tasks (repair exhausted, provider crash, Stop while editing, Stop while waiting for permission): "
            f"zero owned processes, broker capabilities, tool workers, build leases and capability files after every cycle; after close also zero materializations, runtime-dir entries and live adapter processes ({cycles['adapter_processes_started']} adapters started). Scripted peer only.",
            "a cycle left an owned resource, or fewer than 20 + 20 cycles ran", ["cycles.json"],
        )
    else:
        gates["dev_cleanup_cycles"] = ("fail", "cycles.json missing", log)
    if parity:
        compared = parity["compared"]
        ok = (
            len(compared) >= 4 and all(c["identical_after_dropping_volatile_fields"] for c in compared) and parity["frame_parity"]["identical"] is True
            and parity["capability_dead_after_task"] is True and sorted(parity["volatile_fields_dropped"]) == ["expires_at_unix", "id", "path"]
        )
        verdict(
            "dev_cli_mcp_parity", ok,
            f"project_context, timeline, inspect and render_frame answered identically through studio-tools and studio-mcp against the same broker (only the per-call artifact id, path and expiry stamp dropped; rendered PNG hash identical); capability died with the task. "
            "Fake preview worker; no comparison with the project CLI on a real SDK, no real MCP client.",
            "CLI and MCP replies differ", ["cli-mcp-parity.json"],
        )
    else:
        gates["dev_cli_mcp_parity"] = ("fail", "cli-mcp-parity.json missing", log)
    if primitive:
        fs = primitive.get("project_filesystem") or {}
        ready = primitive["apply_gate"] == "ready" and primitive["selected_mechanism"] == "Renameat2" and primitive["link_only_filesystem_gate"]["blocked"]
        note = (
            f"Linux {primitive['kernel']}, project filesystem {fs.get('type')} ({fs.get('source')}): the Apply gate selected {primitive['selected_mechanism']}; a link-only filesystem blocks Apply (link fallback refused for live names)."
            if ready else f"Apply gate {primitive['apply_gate']} on {fs.get('type')}: {primitive.get('blocked_reason')}"
        )
        gates["dev_publication_primitive"] = ("pass" if ready else "blocked", note, ["publication-primitive.json"] + log)
    else:
        gates["dev_publication_primitive"] = ("fail", "publication-primitive.json missing", log)
    return gates


# ---- authentic gates -----------------------------------------------------------------------------------------------------------------------


NO_ADAPTER = "a real ACP adapter executable and a signed-in provider account (none exists on this machine)"
AUTHENTIC = {
    "auth_adapter_probe_v1": (
        "A REAL ACP adapter initializes, negotiates protocol v1, reports Ready auth through a scratch session (no prompt) and its capabilities are recorded.",
        f"Run this harness with --adapter PATH [--adapter-arg ARG ...] [--auth-env NAME ...]: needs {NO_ADAPTER}.",
    ),
    "auth_two_edit_undo_restart": (
        "In a real generated/imported project with a real authenticated adapter: brief -> playable result A, second edit -> B, Undo to A, restart, committed acceptance recovered; timeline/frame/audio/revision identities and source/Git/draft inventories checked at every step.",
        f"{NO_ADAPTER}, an installed SDK and an operator-driven native session. No automated driver for a live provider session exists in this repository; this harness cannot run the gate and a scripted-peer run is never a substitute.",
    ),
    "auth_writer_process_group": (
        "The real adapter's writer processes are proven to stay inside the spawned process group (including after Stop and a provider crash), so WriterOwnership may be ProcessGroupContained for that adapter.",
        f"{NO_ADAPTER}, observed through its real process tree. Until then ownership stays Unknown/Detached and automatic Apply is not qualified for any provider; the scripted peer proves the engine's gate only.",
    ),
    "auth_compiler_error_repair": (
        "An authentic task deliberately introduces a compiler error: structured repair context reaches the agent and exactly one repair attempt runs.",
        f"{NO_ADAPTER}, an installed SDK and an operator-run brief that provokes a compile error.",
    ),
    "auth_interaction_and_failure_modes": (
        "With the real adapter: permission requests, clarification, Stop in every phase, provider crash, external source conflict, and interrupted publication/Undo with restart leave recoverable data and no stuck writer.",
        f"{NO_ADAPTER}; the conflict and interrupted-publication runs also need an operator able to edit the project externally and kill the app mid-publication.",
    ),
    "auth_mcp_cli_support": (
        "The real adapter accepts the stdio MCP server in session/new and/or runs the studio-tools command, and project tools work through that route.",
        f"{NO_ADAPTER} that honors stdio MCP servers (and one that can run shell commands for the studio-tools route); MCP_QUALIFICATION is unqualified against any provider until then.",
    ),
    "auth_twenty_cycle_cleanup": (
        "At least 20 edit or failed-task cycles with the real adapter leave no owned process, broker capability or materialization/PCM lease after close.",
        f"{NO_ADAPTER} and provider quota for 20 real edit/failure cycles.",
    ),
    "auth_visual_ime": (
        "The native conversation panel is verified with real IME composition, focus and Tab order, transcript scroll while streaming, and Space/timeline shortcuts not stolen from the focused prompt.",
        "A native desktop session with a real input method (IME) and an operator; the Linux Xvfb software-rendering runs do not exercise IME or GPU presentation.",
    ),
    "auth_physical_audio": (
        "Promoted-revision audio handoff and the M2 output-clock behavior are measured on a physical audio output device and real display presentation.",
        "A physical audio output device and display on a native desktop; real_cpal_* tests need a real CPAL output device and none exists on this machine.",
    ),
    "auth_windows": (
        "The M3 workflow, packaging and process containment (Job Objects) are exercised on Windows.",
        "A Windows host; non-Linux code is compiled by inspection only and no Windows build was run.",
    ),
    "auth_macos": (
        "The M3 workflow, packaging and process containment are exercised on macOS.",
        "A macOS host; non-Linux code is compiled by inspection only and no macOS build was run.",
    ),
}


def looks_like_fixture(adapter: str, adapter_args: list[str]) -> str | None:
    for item in [adapter, *adapter_args]:
        path = Path(item)
        if not path.is_file():
            continue
        resolved = path.resolve()
        if path.name in {"acp-agent.py", "acp-peer.py"}:
            return f"{item} is a repository ACP fixture"
        try:
            resolved.relative_to(ROOT)
            if "tests" in resolved.parts or "fixtures" in resolved.parts:
                return f"{item} lives in the repository's test tree"
        except ValueError:
            pass
        try:
            head = resolved.read_bytes()[:8192].decode(errors="ignore")
        except OSError:
            continue
        for marker in FIXTURE_TEXT_MARKERS:
            if marker in head:
                return f"{item} declares itself a fixture ({marker!r})"
    return None


def adapter_launch(resolved: str, adapter_args: list[str], auth_env: list[str]) -> dict:
    """The exact launch identity (see `launch_identity_digest`): executable bytes, argument
    words, files that arguments name, and the sign-in variable names. Paths are hashed, never
    recorded."""
    executable_sha256 = sha256_file(Path(resolved))
    arg_files = [(index, sha256_file(Path(arg))) for index, arg in enumerate(adapter_args) if os.path.isabs(arg) and os.path.isfile(arg)]
    return {
        "executable_sha256": executable_sha256,
        "launch_identity": VALIDATOR.launch_identity_digest(executable_sha256, adapter_args, auth_env, arg_files),
    }


def status_name(report: dict) -> str:
    status = report.get("status")
    if isinstance(status, str):
        return status
    if isinstance(status, dict) and len(status) == 1:
        return next(iter(status))
    return "Unknown"


def preflight_adapter(args) -> str | None:
    """Resolve and vet --adapter before anything runs: fixtures and missing files are refused."""
    if not args.adapter:
        return None
    resolved = shutil.which(args.adapter) if os.sep not in args.adapter else args.adapter
    if not resolved or not Path(resolved).is_file():
        raise HarnessError(f"--adapter {args.adapter!r} is not an existing file")
    reason = looks_like_fixture(resolved, args.adapter_arg)
    if reason:
        raise HarnessError(f"refusing --adapter: {reason}. Fixtures can never be recorded as an authentic adapter.")
    if not os.access(resolved, os.X_OK):
        raise HarnessError(f"--adapter {resolved} is not executable")
    return str(Path(resolved).resolve())


def run_adapter_probe(args, out: Path, work: Path, owned: Owned, redactor: Redactor, base_env: dict[str, str]) -> dict:
    resolved = args.adapter_resolved
    env = dict(base_env)
    env.update({"TMPDIR": str(work), "FFRAMES_M3_ADAPTER": str(resolved), "FFRAMES_M3_ADAPTER_ARGS": json.dumps(args.adapter_arg), "FFRAMES_M3_AUTH_ENV": ",".join(args.auth_env)})
    for name in args.auth_env:  # the probe is the only child that sees the credential variables
        if name in os.environ:
            env[name] = os.environ[name]
    probe_dir = work / "probe"
    probe_dir.mkdir()
    env["FFRAMES_M3_EVIDENCE_OUT"] = str(probe_dir)
    argv = ["cargo", "test", "--locked", "-p", "fframes-studio", "--test", "m3_development_evidence", "authentic_adapter_probe", "--", "--ignored", "--exact", "--nocapture"]
    raw = work / "adapter-probe.raw"
    record = owned.run("adapter_probe", argv, cwd=DESKTOP, env=env, output=raw, timeout=args.cargo_timeout)
    text = raw.read_text(errors="replace")
    raw.unlink()
    log = out / "logs" / "adapter_probe.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    header = f"$ {' '.join(record['argv'])}\n# returncode: {record['returncode']}   timed_out: {record['timed_out']}   seconds: {record['seconds']}\n\n"
    log.write_text(redactor.text(header + bounded_log(text)))
    produced = probe_dir / "adapter-probe.json"
    if record["returncode"] != 0 or not produced.is_file():
        return {"status": "fail", "note": f"the probe test did not complete (cargo exited {record['returncode']})", "adapter": None, "files": ["logs/adapter_probe.log"]}
    probe = redactor.json(json.loads(produced.read_text()))
    report = probe["report"]
    initialized = report.get("initialized") or {}
    name = status_name(report)
    agent_name = initialized.get("agent_name")
    fixture = agent_name is not None and str(agent_name).strip().lower() in FIXTURE_AGENT_NAMES
    launch = adapter_launch(resolved, args.adapter_arg, args.auth_env)
    identity = {
        "requested_executable": Path(resolved).name,
        "executable_sha256": launch["executable_sha256"],
        "launch_identity": launch["launch_identity"],
        "platform": {"system": platform.system(), "machine": platform.machine()},
        "arguments": [redactor.text(a) for a in args.adapter_arg],
        "resolved_executable_sha256": probe.get("resolved_executable_sha256"),
        "agent_name": agent_name,
        "agent_version": initialized.get("agent_version"),
        "protocol_version": initialized.get("protocol_version"),
        "auth_methods": initialized.get("auth_methods", []),
        "auth_route": ("environment variables: " + ", ".join(args.auth_env)) if args.auth_env else "adapter-managed sign-in (no environment variables forwarded)",
        "capabilities": initialized.get("capabilities"),
        "probe_status": name,
        "fixture_detected": fixture,
    }
    decision = {"probe_status": name, "fixture_detected": fixture}
    # The m3-authentic/1 record (see validate-qualification.py): bound to this gate, this
    # platform and this exact launch; only a Ready, protocol-v1, non-fixture probe with a
    # clean teardown can ever satisfy its contract.
    probe.update(
        {
            "schema": VALIDATOR.EVIDENCE_SCHEMA,
            "gate": "auth_adapter_probe_v1",
            "platform": identity["platform"],
            "adapter": {"agent_name": agent_name, "protocol_version": initialized.get("protocol_version"), "launch_identity": launch["launch_identity"]},
            "probe": {"status": name, "protocol_version": initialized.get("protocol_version"), "fixture_detected": fixture},
            "cleanup": {"owned_processes_after": len(owned.marked_pids())},
            "measurements": {"auth_status": "ready" if name == "Ready" else name.lower(), "capabilities": initialized.get("capabilities") or {}},
        }
    )
    if fixture:
        probe["evidence_kind"], probe["fixture_only"] = "development", True
        status, note = "fail", f"the adapter identifies itself as the repository fixture {agent_name!r}: recorded as development evidence only"
    elif name == "Ready" and initialized.get("protocol_version") == 1:
        probe["evidence_kind"], probe["fixture_only"] = "authentic", False
        status = "pass"
        note = (
            f"{agent_name} {identity['agent_version']} negotiated ACP v1 and created a scratch session (auth route: {identity['auth_route']}); capabilities recorded. "
            "No prompt was sent, no edit was made and no writer containment is claimed: only the handshake is qualified."
        )
    elif name in {"MissingExecutable", "MissingRuntime", "AuthRequired", "AuthUnknown", "AuthRejected"}:
        probe["evidence_kind"], probe["fixture_only"] = "authentic", False
        status = "blocked"
        note = f"probe status {name}"
    else:
        probe["evidence_kind"], probe["fixture_only"] = "authentic", False
        status, note = "fail", f"probe status {name}"
    probe["decision"] = {**decision, "gate_status": status}
    (out / "adapter-probe.json").write_text(json.dumps(redactor.json(probe), indent=2, sort_keys=True) + "\n")
    prerequisite = {
        "AuthRequired": "Sign the provider account in (provider-owned flow or the --auth-env variable) and rerun.",
        "AuthUnknown": "A session could not prove authentication: sign in to the provider account and rerun.",
        "AuthRejected": "The provider rejected the credentials: supply a valid account/credential and rerun.",
        "MissingExecutable": "Install the adapter executable and pass its path via --adapter.",
        "MissingRuntime": "Install the runtime the adapter needs (for example Node.js) and rerun.",
    }.get(name)
    return {"status": status, "note": note, "adapter": identity, "files": ["adapter-probe.json", "logs/adapter_probe.log"], "prerequisite": prerequisite}


# ---- README and ledger ---------------------------------------------------------------------------------------------------------------


def readme_text(results: list[CheckResult], adapter_outcome: dict | None, args) -> str:
    lines = [
        "# Linux M3 agent-transaction evidence",
        "",
        "Written by `desktop/scripts/qualify-m3-agent.py`. Development evidence only: it proves the workflow, engine, tools and cleanup behavior of this repository against a SCRIPTED ACP peer, a fake SDK tree / preview worker (and an installed SDK for the optional real-SDK checks). It does not qualify a provider, an adapter, an account, a writer process-group model, a physical device, IME input, Windows or macOS.",
        "",
        "## Commands recorded here",
        "",
    ]
    for item in results:
        state = {"pass": "PASS", "fail": "FAIL", "not_run": "NOT RUN"}[item.status]
        lines.append(f"- `{item.check.name}` [{state}] {item.reason}: `{' '.join(item.check.argv)}`" if item.status != "not_run" else f"- `{item.check.name}` [NOT RUN] {item.reason}")
    lines += [
        "",
        "## Files",
        "",
        "- `environment.json`: OS, kernel, toolchain, git, SDK and adapter identity (environment variable names only, never values).",
        "- `harness-run.json`: harness arguments, endpoint guard, every owned PID, per-check exit status and leftovers.",
        "- `logs/*.log`: redacted, size-bounded cargo output per check.",
        "- `resource-bounds.json`, `compiler-count.json`, `cycles.json`, `cli-mcp-parity.json`, `publication-primitive.json`, `summary.json`: measurements written by `app/tests/m3_development_evidence.rs` (full profile).",
    ]
    if adapter_outcome is not None:
        lines.append("- `adapter-probe.json`: the authentic real-adapter handshake record.")
    lines += [
        "",
        "## Limits",
        "",
        "- No ACP adapter or credentials exist on the development machine unless `--adapter` was supplied; every other authentic gate is `not_run` in `../../m3-results.json` with its prerequisite.",
        "- Event-queue bound: the driver exposes no queue-depth gauge; the measurement is the absence of an overflow failure while the scripted event count exceeds the 256-event queue (the peer paces its tool-card flood: an unpaced burst of non-coalescing events deliberately seals the producer).",
        "- Playback/scrub concurrency, GPU layout cost, physical audio and IME were not measured.",
        "- Re-run: `python3 scripts/qualify-m3-agent.py --replace` from `desktop/`, then `python3 scripts/validate-qualification.py`.",
    ]
    return "\n".join(lines) + "\n"


def gate_entry(kind, status, criteria, notes, files, prerequisite=None):
    entry = {"kind": kind, "status": status, "criteria": criteria, "notes": notes, "evidence": files}
    if prerequisite:
        entry["prerequisite"] = prerequisite
    return entry


DEV_CRITERIA = {
    "dev_acp_transport": "ACP v1 driver tests (acp_v1, acp_v1_review) pass against the scripted peer: negotiation, streaming, permissions, cancellation, bounds, redaction.",
    "dev_engine_transactions": "Engine agent-task, candidate-validation, edit-transaction, promotion, history and crash-recovery suites pass.",
    "dev_workflow_fixture": "The AgentWorkflow integration suite passes against the scripted peer, fake preview worker and real controller.",
    "dev_tools_and_validation": "Project tool broker/CLI/MCP, shared build service, candidate runner and preview handoff suites pass.",
    "dev_native_ui_workflow": "The native conversation panel / shell workflow test target (agent_workflow_ui: controls, permission and clarification cards, paging inside the resident limits, UI preview-handoff sink and acknowledgement) passes against the scripted peer.",
    "dev_native_x11_shell": "The production GPUI shell on an owned Xvfb (scripted peer, real SDK, software rendering) sends a prompt through native input after a baseline preview stepped to a non-zero playhead; the media-changing edit is adopted via adopt_promotion and displayed with matching video/audio revision and a changed audio digest, a fresh transport epoch, the playhead position/frame/PCM window retained and the awaiting label cleared; Undo restores the original audio through another matching hand-off under yet another epoch. Writer containment is the explicit test-only injection, not a qualification. NOT a provider, physical-display, physical-audio or IME qualification.",
    "dev_resource_bounds": "Under a long streamed transcript resident rows <= 400 and bytes <= 4 MiB (+ the newest row), older rows are paged, the 256-event queue never overflows and nothing is owned after close.",
    "dev_compiler_count": "The shared BuildService starts exactly one compile per equal build key (UI/tool/validation subscribers), one per distinct key and per accepted edit.",
    "dev_cleanup_cycles": "At least 20 accepted edits and 20 failed/cancelled tasks leave zero owned processes, broker capabilities, build leases and materializations, checked after every cycle and after close.",
    "dev_cli_mcp_parity": "studio-tools and studio-mcp return identical project_context/timeline/inspect/render_frame results against the same broker capability.",
    "dev_publication_primitive": "The Linux Apply gate selects atomic renameat2(RENAME_NOREPLACE) on the project filesystem and refuses a link-only filesystem.",
    "dev_real_sdk_workflow": "The ignored real-SDK workflow/promotion tests pass with a real Cargo compile and real preview worker (scripted agent).",
}


def build_ledger(results, probe, environment, args, files_for_hashes) -> dict:
    by_gate: dict[str, tuple[str, str, list[str]]] = {}
    prerequisites: dict[str, str] = {}
    for item in results:
        if item.prerequisite:
            prerequisites.update({gate: item.prerequisite for gate in item.check.gates})
        if item.check.measurement_files:
            by_gate.update(judge_measurements(Path(args.out_dir), item))
        else:
            for gate in item.check.gates:
                if item.status == "not_run":
                    by_gate[gate] = ("not_run", item.reason, [])
                else:
                    by_gate[gate] = (item.status, item.reason, [item.log] if item.log else [])
    gates = {}
    for name, (status, note, files) in by_gate.items():
        prerequisite = None
        if status in {"not_run", "blocked"}:
            prerequisite = prerequisites.get(name) or note
        evidence = [e for e in dict.fromkeys(files + ["environment.json", "harness-run.json"]) if status != "not_run" or e in {"environment.json", "harness-run.json"}]
        gates[name] = gate_entry("development", status, DEV_CRITERIA[name], note, [files_for_hashes(e) for e in evidence], prerequisite)
    for name, (criteria, prerequisite) in AUTHENTIC.items():
        if name == "auth_adapter_probe_v1" and probe is not None:
            gates[name] = gate_entry("authentic", probe["status"], criteria, probe["note"], [files_for_hashes(e) for e in probe["files"]], None if probe["status"] in {"pass", "fail"} else (probe.get("prerequisite") or prerequisite))
            if probe["status"] == "pass":
                gates[name]["evidence"] = [files_for_hashes("adapter-probe.json")]
        else:
            gates[name] = gate_entry(
                "authentic", "not_run", criteria,
                "Not run on this machine: the prerequisite below is missing. No fixture or development evidence is substituted for it.",
                [], prerequisite,
            )
    ordered = {k: gates[k] for k in VALIDATOR.M3_GATES}
    unmet = [k for k, g in ordered.items() if g["kind"] == "authentic" and g["status"] != "pass"]
    all_pass = not unmet
    return {
        "kind": "m3",
        "schema_version": 1,
        "timestamp": now(),
        "environment": environment,
        "gates": ordered,
        "acceptance": {
            "m3_authenticated": "pass" if all_pass else "not_run",
            "reason": (
                "Every authentic gate passed." if all_pass
                else f"M3 authenticated acceptance is NOT claimed: {len(unmet)} authentic gate(s) are not pass ({', '.join(unmet)}). Development (fixture) gates prove the workflow code only."
            ),
        },
    }


# ---- main ------------------------------------------------------------------------------------------------------------------------------------


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path, default=None, help="evidence directory (default qualification/evidence/linux-m3-<date>)")
    parser.add_argument("--date", default=None, help="YYYY-MM-DD used in the default evidence directory name (default: today, local time)")
    parser.add_argument("--ledger", type=Path, default=DESKTOP / "qualification/m3-results.json", help="ledger file to write")
    parser.add_argument("--replace", action="store_true", help="replace an evidence directory this harness created earlier")
    parser.add_argument("--checks", default=None, help="comma-separated checks to run (see --list-checks); default all")
    parser.add_argument("--list-checks", action="store_true", help="print the available checks and exit")
    parser.add_argument("--cargo-timeout", type=float, default=2400.0, help="per-cargo-invocation timeout in seconds")
    parser.add_argument("--real-sdk-active", type=Path, default=None, help="installed SDK directory: also run the ignored real-SDK workflow tests")
    parser.add_argument("--adapter", default=None, help="path of a REAL ACP adapter: runs the authentic handshake probe")
    parser.add_argument("--adapter-arg", action="append", default=[], help="adapter argument (repeatable)")
    parser.add_argument("--auth-env", action="append", default=[], metavar="NAME", help="NAME of the credential environment variable (repeatable; value never recorded)")
    args = parser.parse_args(argv)
    if args.adapter_arg and not args.adapter:
        parser.error("--adapter-arg requires --adapter")
    if args.auth_env and not args.adapter:
        parser.error("--auth-env requires --adapter")
    for name in args.auth_env:
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            parser.error(f"--auth-env expects a variable NAME, got {name!r}")
    date = args.date or datetime.now().astimezone().strftime("%Y-%m-%d")
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
        parser.error("--date expects YYYY-MM-DD")
    args.ledger = args.ledger.resolve()
    args.out_dir = (args.out or args.ledger.parent / f"evidence/linux-m3-{date}").resolve()
    return args


def prepare_output(args) -> Path:
    out = args.out_dir
    ledger_evidence = args.ledger.parent / "evidence"
    if ledger_evidence.resolve() not in out.parents:
        raise HarnessError(f"--out must live under {ledger_evidence} so the ledger can reference it with relative paths")
    if out.exists():
        if not args.replace:
            raise HarnessError(f"{out} exists; pass --replace to replace an evidence directory created by this harness")
        if not (out / MARKER_FILE).is_file():
            raise HarnessError(f"{out} was not created by this harness (no {MARKER_FILE}); refusing to delete it")
        shutil.rmtree(out)
    out.mkdir(parents=True)
    (out / MARKER_FILE).write_text("created by desktop/scripts/qualify-m3-agent.py\n")
    return out


def main(argv: list[str] | None = None) -> int:
    raw_argv = sys.argv[1:] if argv is None else argv
    args = parse_args(raw_argv)
    if args.list_checks:
        for check in CHECKS:
            print(f"{check.name:22} {check.description}")
        return 0
    if platform.system() != "Linux":
        print("qualify-m3-agent.py only runs on Linux; other platforms stay not_run", file=sys.stderr)
        return 2
    selected = set(CHECKS_BY_NAME) if not args.checks else {c.strip() for c in args.checks.split(",") if c.strip()}
    if selected - set(CHECKS_BY_NAME):
        print(f"unknown check(s): {', '.join(sorted(selected - set(CHECKS_BY_NAME)))}", file=sys.stderr)
        return 2

    secrets = [os.environ[n] for n in args.auth_env if os.environ.get(n)]
    redactor = Redactor(secrets, path_roots(args.real_sdk_active.resolve() if args.real_sdk_active else None))
    guard = EndpointGuard()
    harness_id = uuid.uuid4().hex
    owned = Owned(harness_id, redactor)
    work: Path | None = None
    out: Path | None = None
    interrupted = []

    def on_signal(signum, _frame):
        interrupted.append(signum)
        raise KeyboardInterrupt

    try:
        args.adapter_resolved = preflight_adapter(args)
        guard.acquire()
    except HarnessError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    signal.signal(signal.SIGTERM, on_signal)
    try:
        out = prepare_output(args)
        work = Path(tempfile.mkdtemp(prefix="fm3-"))
        redactor.add_roots([(work, "<work>"), (out, "<evidence>")])
        sdk_dir = args.real_sdk_active.resolve() if args.real_sdk_active else None
        sdk = sdk_record(sdk_dir)
        base_env = {k: v for k, v in os.environ.items() if k not in args.auth_env and not k.startswith("FFRAMES_M3_")}
        started = time.monotonic()
        runner = Runner(args, out, work, owned, redactor, base_env)
        results = [runner.run(check, check.name in selected, sdk_dir) for check in CHECKS]
        normalized_files = normalize_measurements(out, redactor)
        probe = None
        if args.adapter:
            probe = run_adapter_probe(args, out, work, owned, redactor, base_env)
        adapter_identity = probe["adapter"] if probe else None
        environment = environment_record(args, out, sdk, adapter_identity, redactor, raw_argv)
        environment = redactor.json(environment)
        leftovers_alive = owned.shutdown()
        run_record = {
            "harness_id": harness_id,
            "endpoint_guard": {"unix_socket": str(guard.path), "tcp_ports": [], "note": "no TCP port is bound; the endpoint was free and was released at exit"},
            "artifact_leases": {"work_directory": str(work), "evidence_directory": str(out), "removed_at_exit": ["work_directory"]},
            "owned_processes": owned.history,
            "leftover_processes_stopped": owned.stopped_leftovers,
            "owned_processes_alive_at_end": leftovers_alive,
            "unowned_group_members_left_alone": owned.unowned_members,
            "measurement_files_path_normalized": normalized_files,
            "checks": [
                {"name": r.check.name, "status": r.status, "reason": r.reason, "log": r.log, "returncode": r.record["returncode"] if r.record else None,
                 "seconds": r.record["seconds"] if r.record else None, "test_results": r.parsed["results"] if r.parsed else None}
                for r in results
            ],
            "total_seconds": round(time.monotonic() - started, 3),
            "finished": now(),
        }
        (out / "environment.json").write_text(json.dumps(redactor.json(environment), indent=2, sort_keys=True) + "\n")
        (out / "harness-run.json").write_text(json.dumps(redactor.json(run_record), indent=2, sort_keys=True) + "\n")
        (out / "README.md").write_text(redactor.text(readme_text(results, probe, args)))

        def hashed(relative: str) -> dict:
            return {"path": (out / relative).relative_to(args.ledger.parent).as_posix(), "sha256": sha256_file(out / relative)}

        ledger = build_ledger(results, probe, environment, args, hashed)
        if leftovers_alive:
            raise HarnessError(f"harness-owned processes survived shutdown: {leftovers_alive}")
        text = json.dumps(redactor.json(ledger), indent=2) + "\n"
        temporary = args.ledger.with_suffix(".json.tmp")
        temporary.write_text(text)
        os.replace(temporary, args.ledger)
        VALIDATOR.validate(args.ledger)
    except KeyboardInterrupt:
        print("interrupted: stopping harness-owned processes", file=sys.stderr)
        owned.shutdown()
        return 130
    except HarnessError as error:
        print(f"error: {error}", file=sys.stderr)
        owned.shutdown()
        return 2
    finally:
        owned.shutdown()
        if work is not None:
            shutil.rmtree(work, ignore_errors=True)
        guard.release()

    print(f"evidence: {out}")
    print(f"ledger:   {args.ledger}")
    for name, gate in ledger["gates"].items():
        print(f"  {gate['status']:8} {gate['kind']:12} {name}")
    print(f"M3 authenticated acceptance: {ledger['acceptance']['m3_authenticated']}")
    failed = [n for n, g in ledger["gates"].items() if g["status"] == "fail"]
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
