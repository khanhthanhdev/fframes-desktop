#!/usr/bin/env python3
"""Drive the real Linux Studio M2 playback qualification through native input.

This program deliberately treats app telemetry as observations, not as a control API.
It never labels a virtual PulseAudio run as physical audio timing evidence.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import math
import os
from pathlib import Path
import platform
import random
import signal
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone


FIXTURE_MARKERS = (
    "//! Copied into an ordinary portable Studio project by the integration fixture.",
    "pub struct StudioVideo;",
    'AudioTrack::new(\n                "cue.wav",',
)
WINDOW_TITLE = "fframes Studio"


class QualificationError(RuntimeError):
    pass


def atomic_write(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(fraction * len(ordered)) - 1)]


def rss_kib(pid: int) -> int | None:
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except (FileNotFoundError, ProcessLookupError, ValueError):
        pass
    return None


def process_tree(pid: int) -> list[dict]:
    """Snapshot descendants attributable to the application without host-wide matching."""
    found: list[dict] = []
    pending = [pid]
    seen: set[int] = set()
    while pending:
        current = pending.pop()
        if current in seen:
            continue
        seen.add(current)
        try:
            command = Path(f"/proc/{current}/cmdline").read_bytes().rstrip(b"\0").replace(b"\0", b" ").decode(errors="replace")
            children = Path(f"/proc/{current}/task/{current}/children").read_text().split()
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
        found.append({"pid": current, "command": command})
        pending.extend(int(child) for child in children)
    return found


def slope_mib_per_minute(samples: list[tuple[float, int]]) -> float | None:
    if len(samples) < 2:
        return None
    xs = [(item[0] - samples[0][0]) / 60.0 for item in samples]
    ys = [item[1] / 1024.0 for item in samples]
    x_mean, y_mean = statistics.fmean(xs), statistics.fmean(ys)
    denominator = sum((x - x_mean) ** 2 for x in xs)
    if denominator == 0:
        return None
    return sum((x - x_mean) * (y - y_mean) for x, y in zip(xs, ys)) / denominator


class OwnedProcesses:
    def __init__(self) -> None:
        self.items: list[tuple[subprocess.Popen, list[str]]] = []
        self.history: list[dict] = []

    def start(self, command: list[str], **kwargs) -> subprocess.Popen:
        process = subprocess.Popen(command, start_new_session=True, **kwargs)
        self.items.append((process, command))
        self.history.append({"pid": process.pid, "command": command})
        return process

    @staticmethod
    def stop(process: subprocess.Popen, timeout: float = 5.0) -> None:
        if process.poll() is not None:
            return
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=timeout)

    def stop_one(self, process: subprocess.Popen) -> None:
        self.stop(process)

    def stop_all(self) -> None:
        for process, _ in reversed(self.items):
            self.stop(process)

    def alive(self) -> list[int]:
        return [process.pid for process, _ in self.items if process.poll() is None]


class Driver:
    def __init__(self, args: argparse.Namespace, environment: dict[str, str], owned: OwnedProcesses):
        self.args = args
        self.env = environment
        self.owned = owned
        self.telemetry_path = args.out / "telemetry.json"
        self.window: str | None = None
        self.latest: dict = {}
        self.seek_latencies: list[float] = []
        self.rss_samples: list[tuple[float, int]] = []
        self.telemetry_samples: list[dict] = []
        self.observed_processes: dict[int, dict] = {}
        self.rebuilds = 0
        self.failed_builds = 0
        self.seeks = 0
        self.errors: list[str] = []
        self.started = time.monotonic()
        self.last_sample = 0.0

    def command(self, *parts: str, check: bool = True) -> subprocess.CompletedProcess:
        return subprocess.run(list(parts), env=self.env, check=check, capture_output=True, text=True)

    def discover_window(self, deadline: float) -> str:
        while time.monotonic() < deadline:
            result = self.command("xdotool", "search", "--name", f"^{WINDOW_TITLE}$", check=False)
            candidates = [line for line in result.stdout.splitlines() if line]
            if candidates:
                self.window = candidates[0]
                return self.window
            time.sleep(0.1)
        raise QualificationError("native Studio window did not appear")

    def read(self) -> dict:
        try:
            value = json.loads(self.telemetry_path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            return self.latest
        if not isinstance(value, dict):
            raise QualificationError("telemetry root is not an object")
        self.latest = value
        if not value.get("audio_identity_matches", True):
            raise QualificationError("audio identity or epoch differs from displayed revision")
        images = value.get("image_metrics", {})
        thumbnails = value.get("thumbnail_metrics", {})
        pump = value.get("pump_metrics") or {}
        if images.get("release_failures", 0) or thumbnails.get("release_failures", 0):
            raise QualificationError("native image release failed")
        if images.get("resident", 0) > 2 or images.get("queued", 0) > 2 or thumbnails.get("high_water_entries", 0) > 64 or thumbnails.get("high_water_bytes", 0) > 16 * 1024 * 1024 or pump.get("queue_high_water", 0) > 1 or pump.get("thumbnail_queue_high_water", 0) > 12:
            raise QualificationError("native cache or request queue exceeded its bound")
        now = time.monotonic()
        if now - self.last_sample >= 1.0:
            pid = value.get("pid")
            resident = rss_kib(pid) if isinstance(pid, int) else None
            if resident is not None:
                self.rss_samples.append((now, resident))
            for process in process_tree(pid) if isinstance(pid, int) else []:
                self.observed_processes[process["pid"]] = process
            self.telemetry_samples.append({"monotonic_seconds": now - self.started, "rss_kib": resident, "telemetry": value})
            self.last_sample = now
        return value

    def wait(self, predicate, description: str, timeout: float = 90.0) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.app.poll() is not None:
                raise QualificationError(f"application exited while waiting for {description}")
            value = self.read()
            if value and predicate(value):
                return value
            time.sleep(0.01)
        raise QualificationError(f"timed out waiting for {description}")

    def point(self, x: float, y: float, click: bool = True) -> None:
        assert self.window
        command = ["xdotool", "mousemove", "--window", self.window, str(round(x)), str(round(y))]
        if click:
            command += ["mousedown", "1", "mouseup", "1"]
        self.command(*command)

    def button(self, name: str) -> None:
        box = self.latest.get("buttons", {}).get(name)
        if not isinstance(box, list) or len(box) != 4:
            raise QualificationError(f"missing native button geometry: {name}")
        self.point(box[0] + box[2] / 2, box[1] + box[3] / 2)

    def key(self, key: str) -> None:
        assert self.window
        self.command("xdotool", "key", "--window", self.window, "--clearmodifiers", key)

    def seek(self, fraction: float) -> None:
        bounds = self.latest.get("ruler_bounds")
        if not isinstance(bounds, list) or len(bounds) != 4:
            raise QualificationError("missing timeline ruler geometry")
        before = int(self.latest.get("seek_serial", 0))
        target_x = bounds[0] + max(0.001, min(0.999, fraction)) * bounds[2]
        geometry = self.latest["thumbnail_metrics"]
        # Allow two pixels for X11's border and integer pointer rounding.
        # Derive the allowed nearest frames
        # independently; still require the exact latest serial and displayed frame.
        def cursor(offset):
            return min(self.latest["total_frames"], max(0, math.floor((round(target_x) - bounds[0] + offset + geometry["scroll_x"]) / geometry["pixels_per_second"] * self.latest["fps"] + 0.5)))
        low, high = cursor(-2), cursor(2)
        begun = time.monotonic()
        self.point(target_x, bounds[1] + bounds[3] * 0.55)
        self.wait(
            lambda t: int(t.get("seek_serial", 0)) > before and low <= t.get("position", -1) <= high and t.get("paint_serial") == t.get("seek_serial") and t.get("paint_frame") == min(t["position"], t["total_frames"] - 1),
            f"matching painted seek frame in [{low}, {high}] after serial {before}",
            10,
        )
        self.seek_latencies.append((time.monotonic() - begun) * 1000.0)
        self.seeks += 1

    def rebuild(self, expect_failure: bool) -> None:
        source = self.args.project / "src/lib.rs"
        original = source.read_bytes()
        old_generation = self.latest.get("displayed_generation")
        old_revision = self.latest.get("displayed_revision")
        try:
            if expect_failure:
                text = original.decode()
                if not all(marker in text for marker in FIXTURE_MARKERS):
                    raise QualificationError("fixture changed; refusing build-failure injection")
                source.write_text(text + "\ncompile_error!(\"intentional M2 qualification failure\");\n")
            self.button("build-preview")
            complete = self.wait(lambda t: t.get("preview_status", "").startswith("Error") if expect_failure else t.get("displayed_generation") != old_generation, "build completion", 180)
            if expect_failure:
                if not complete.get("error") or complete.get("displayed_revision") != old_revision:
                    raise QualificationError("failed rebuild did not retain the old displayed revision")
                if complete.get("audio_ready") and not complete.get("audio_identity_matches"):
                    raise QualificationError("failed rebuild left mismatched audio")
                self.failed_builds += 1
            else:
                if complete.get("error") or complete.get("displayed_generation") == old_generation:
                    raise QualificationError("successful rebuild did not install a new generation")
                if complete.get("audio_ready") and not complete.get("audio_identity_matches"):
                    raise QualificationError("successful rebuild installed mismatched audio")
            self.rebuilds += 1
        finally:
            if expect_failure:
                source.write_bytes(original)
                self.wait(lambda t: t.get("current_source_revision") == old_revision and not t.get("busy"), "restored source reconciliation", 15)
                # The failure banner changes layout. Await a native layout/paint
                # turn before using the new geometry for the next pointer event.
                time.sleep(0.1)
                self.read()

    def exercise_controls(self) -> None:
        self.button("play-pause")
        self.wait(lambda t: bool(t.get("playing")), "play")
        self.wait(lambda t: t.get("audio_ready") and t.get("audio_metrics", {}).get("submitted_frames", 0) > 0 and t.get("position", 0) > 0, "active output-clock playback", 15)
        self.button("mute-audio")
        self.wait(lambda t: bool(t.get("muted")), "mute")
        self.button("mute-audio")
        self.wait(lambda t: not bool(t.get("muted")), "unmute")
        self.button("play-pause")
        self.wait(lambda t: not bool(t.get("playing")), "pause")
        self.seek(0.4)
        for control in ("previous-frame", "next-frame"):
            old = self.latest.get("position")
            self.button(control)
            self.wait(lambda t, old=old: t.get("position") != old, control)
        bounds = self.latest["ruler_bounds"]
        self.point(bounds[0] + bounds[2] / 2, bounds[1] + bounds[3] / 2)
        for key in ("Left", "Right", "Home", "End", "plus", "minus", "Prior", "Next"):
            self.key(key)
            time.sleep(0.08)
            self.read()
        self.button("timeline-fit")

        # Device selection, no-device state, and retry are distinct from mute.
        self.button("next-audio-output")
        time.sleep(0.3)
        self.button("no-audio-output")
        self.wait(lambda t: not bool(t.get("audio_ready")), "no-device mode")
        self.button("retry-audio-output")
        self.wait(lambda t: bool(t.get("audio_ready")), "audio retry", 15)

        self.key("Home")
        self.button("play-pause")
        self.wait(lambda t: bool(t.get("playing")), "play to end")
        self.wait(lambda t: not bool(t.get("playing")) and int(t.get("position", -1)) >= int(t.get("total_frames", 0)) - 1, "end", 30)
        self.button("play-pause")
        self.wait(lambda t: bool(t.get("playing")) and int(t.get("position", 1)) < int(t.get("total_frames", 0)) - 1, "replay", 10)
        self.button("play-pause")
        self.wait(lambda t: not bool(t.get("playing")), "replay pause", 10)

    def run_workload(self, app: subprocess.Popen) -> None:
        self.app = app
        self.discover_window(time.monotonic() + 30)
        self.wait(lambda t: t.get("project_path") == str(self.args.project) and t.get("displayed_generation") is not None and t.get("ruler_bounds") and t.get("buttons", {}).get("timeline-fit"), "initial preview", 180)
        self.exercise_controls()
        rng = random.Random(0x4D325345454B)
        warmup = min(20, self.args.seeks)
        for index in range(self.args.seeks):
            if bool(self.latest.get("playing")):
                self.button("play-pause")
                self.wait(lambda t: not bool(t.get("playing")), "paused seek profile")
            # Alternating edge-biased distributions are deterministic and asymmetric.
            fraction = rng.betavariate(0.65, 1.9) if index % 2 == 0 else 1.0 - rng.betavariate(0.8, 2.7)
            self.seek(fraction)
            if self.args.seeks > warmup and index + 1 == warmup:
                self.seek_latencies.clear()
                self.rss_samples.clear()
            due = min(self.args.rebuilds, (index + 1) // 40)
            while self.rebuilds < due:
                playing_rebuild = self.rebuilds % 2 == 0
                if bool(self.latest.get("playing")) != playing_rebuild:
                    self.button("play-pause")
                    self.wait(lambda t, playing_rebuild=playing_rebuild: bool(t.get("playing")) == playing_rebuild, "rebuild transport mode")
                failure = self.args.inject_build_failures and (self.rebuilds + 1) % 10 == 0
                self.rebuild(failure)
            if time.monotonic() - self.started >= self.args.duration_seconds and index + 1 >= self.args.seeks:
                break
        while self.rebuilds < self.args.rebuilds:
            playing_rebuild = self.rebuilds % 2 == 0
            if bool(self.latest.get("playing")) != playing_rebuild:
                self.button("play-pause")
                self.wait(lambda t, playing_rebuild=playing_rebuild: bool(t.get("playing")) == playing_rebuild, "rebuild transport mode")
            failure = self.args.inject_build_failures and (self.rebuilds + 1) % 10 == 0
            self.rebuild(failure)
        remaining = self.args.duration_seconds - (time.monotonic() - self.started)
        while remaining > 0:
            self.read()
            if not self.latest.get("playing"):
                self.button("play-pause")
                self.wait(lambda t: bool(t.get("playing")), "sustained playback", 10)
            time.sleep(min(1.0, remaining))
            remaining = self.args.duration_seconds - (time.monotonic() - self.started)


def wm_delete(display_name: str, window_id: str) -> None:
    """Send WM_DELETE_WINDOW through Xlib; unlike xdotool windowclose this is graceful."""
    library = ctypes.util.find_library("X11")
    if not library:
        raise QualificationError("libX11 is unavailable")
    x11 = ctypes.CDLL(library)
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x11.XOpenDisplay(display_name.encode())
    if not display:
        raise QualificationError("cannot open X display for WM_DELETE_WINDOW")
    try:
        delete = x11.XInternAtom(display, b"WM_DELETE_WINDOW", False)
        protocols = x11.XInternAtom(display, b"WM_PROTOCOLS", False)

        class Data(ctypes.Union):
            _fields_ = [("l", ctypes.c_long * 5)]

        class Client(ctypes.Structure):
            _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                        ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
                        ("format", ctypes.c_int), ("data", Data)]

        event = Client(33, 0, 1, display, int(window_id), protocols, 32, Data())
        event.data.l[0] = delete
        event.data.l[1] = 0
        x11.XSendEvent(display, int(window_id), False, 0, ctypes.byref(event))
        x11.XFlush(display)
    finally:
        x11.XCloseDisplay(display)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--application", required=True, type=Path)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--sdk-home", required=True, type=Path, help="HOME containing the active installed SDK")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--display", default=":93")
    parser.add_argument("--duration-seconds", type=int, default=600)
    parser.add_argument("--seeks", type=int, default=2000)
    parser.add_argument("--rebuilds", type=int, default=50)
    parser.add_argument("--virtual-audio", action="store_true")
    parser.add_argument("--inject-build-failures", action="store_true")
    args = parser.parse_args()
    args.application, args.project, args.sdk_home, args.out = (p.resolve() for p in (args.application, args.project, args.sdk_home, args.out))
    if args.out.exists():
        parser.error("--out must name a fresh directory")
    if min(args.duration_seconds, args.seeks, args.rebuilds) < 0:
        parser.error("duration, seeks, and rebuilds must be non-negative")
    if not args.display.startswith(":") or not args.display[1:].isdigit():
        parser.error("--display must be a local numeric X display")
    if Path(f"/tmp/.X{args.display[1:]}-lock").exists():
        parser.error(f"display {args.display} already has a lock")
    if not args.application.is_file() or not os.access(args.application, os.X_OK):
        parser.error("--application must be an executable file")
    source = args.project / "src/lib.rs"
    if not source.is_file() or not str(args.project).startswith("/tmp/"):
        parser.error("--project must be a disposable fixture under /tmp")
    original_source = source.read_bytes()
    try:
        fixture_text = original_source.decode()
    except UnicodeDecodeError:
        parser.error("fixture source is not UTF-8")
    if not all(marker in fixture_text for marker in FIXTURE_MARKERS):
        parser.error("project source does not exactly identify the M2 disposable fixture")

    args.out.mkdir(parents=True)
    (args.out / "runtime").mkdir(mode=0o700)
    data_home = args.out / "data"
    data_home.mkdir()
    environment = os.environ.copy()
    environment.update({"DISPLAY": args.display, "HOME": str(args.sdk_home), "XDG_DATA_HOME": str(data_home),
                        "XDG_RUNTIME_DIR": str(args.out / "runtime"), "LIBGL_ALWAYS_SOFTWARE": "1"})
    owned = OwnedProcesses()
    driver = Driver(args, environment, owned)
    pulse = xvfb = app = None
    error: str | None = None
    graceful_exit = False
    logs = {}
    try:
        logs["xvfb"] = (args.out / "xvfb.log").open("w")
        xvfb = owned.start(["Xvfb", args.display, "-screen", "0", "1280x1200x24", "-nolisten", "tcp"], stdout=logs["xvfb"], stderr=subprocess.STDOUT)
        time.sleep(0.8)
        if xvfb.poll() is not None:
            raise QualificationError("Xvfb failed to start")
        if args.virtual_audio:
            pulse_socket = args.out / "runtime/pulse-native"
            logs["pulse"] = (args.out / "pulse.log").open("w")
            pulse_command = ["pulseaudio", "-n", "--daemonize=no", "--exit-idle-time=-1", "--disallow-exit=no",
                             "--use-pid-file=no",
                             "-L", f"module-native-protocol-unix socket={pulse_socket} auth-anonymous=1",
                             "-L", "module-null-sink sink_name=m2_null rate=48000 channels=2"]
            pulse = owned.start(pulse_command, env=environment, stdout=logs["pulse"], stderr=subprocess.STDOUT)
            environment["PULSE_SERVER"] = f"unix:{pulse_socket}"
            deadline = time.monotonic() + 10
            while not pulse_socket.exists() and time.monotonic() < deadline and pulse.poll() is None:
                time.sleep(0.05)
            if not pulse_socket.exists():
                raise QualificationError("private PulseAudio server did not create its socket")
        logs["application"] = (args.out / "application.log").open("w")
        command = [str(args.application), "qualify-m2", "--project", str(args.project), "--telemetry", str(driver.telemetry_path)]
        app = owned.start(command, cwd=args.out, env=environment, stdout=logs["application"], stderr=subprocess.STDOUT)
        driver.run_workload(app)
        from PIL import ImageGrab
        ImageGrab.grab(xdisplay=args.display).save(args.out / "native-window.png")
        wm_delete(args.display, driver.window or "0")
        try:
            app.wait(timeout=15)
            graceful_exit = app.returncode == 0
        except subprocess.TimeoutExpired:
            raise QualificationError("application did not exit after WM_DELETE_WINDOW")
    except Exception as caught:  # Preserve partial, truthful evidence on every failure.
        error = f"{type(caught).__name__}: {caught}"
    finally:
        source.write_bytes(original_source)
        if pulse and pulse.poll() is None:
            subprocess.run(["pactl", "-s", environment.get("PULSE_SERVER", ""), "exit"], env=environment, timeout=5, check=False)
            try:
                pulse.wait(timeout=5)
            except subprocess.TimeoutExpired:
                owned.stop_one(pulse)
        if app and app.poll() is None:
            owned.stop_one(app)
        if xvfb and xvfb.poll() is None:
            owned.stop_one(xvfb)
        for stream in logs.values():
            stream.close()

    final = driver.latest
    elapsed = time.monotonic() - driver.started
    data_dir = data_home / "fframes-studio"
    leases = sorted(str(p.relative_to(data_dir)) for p in data_dir.rglob("*") if (p.is_dir() and p.name.startswith("build-")) or (p.is_file() and (p.name.startswith("pcm-") or p.suffix == ".pcm"))) if data_dir.exists() else []
    orphaned = [item for item in driver.observed_processes.values() if Path(f"/proc/{item['pid']}").exists()]
    limits_unmet = args.duration_seconds < 600 or args.seeks < 2000 or args.rebuilds < 50 or driver.seeks < 2000 or driver.rebuilds < 50 or elapsed < 600
    result = {
        "kind": "m2-native-raw", "schema_version": 1, "timestamp": datetime.now(timezone.utc).isoformat(),
        "finished": error is None and graceful_exit and not owned.alive() and not orphaned and not leases,
        "error": error, "limits_unmet": limits_unmet, "physical_audio": False,
        "environment": {"os": platform.system(), "release": platform.release(), "arch": platform.machine(),
                        "display": args.display, "xvfb": True, "scale": 1, "virtual_audio": args.virtual_audio,
                        "pulse_server": environment.get("PULSE_SERVER"), "application": str(args.application),
                        "project": str(args.project), "sdk_home": str(args.sdk_home)},
        "requested": {"duration_seconds": args.duration_seconds, "seeks": args.seeks, "rebuilds": args.rebuilds,
                      "inject_build_failures": args.inject_build_failures},
        "measurements": {"elapsed_seconds": elapsed, "confirmed_seeks": driver.seeks, "rebuilds": driver.rebuilds,
                         "failed_builds": driver.failed_builds, "seek_p95_ms": percentile(driver.seek_latencies, 0.95),
                         "rss_slope_mib_per_min": slope_mib_per_minute(driver.rss_samples),
                         "av_error_ms": None, "timestamp_residual_ms": None,
                         "audio_timing_note": "Unavailable: no physical DAC/output capture was measured.",
                         "final_audio_metrics": final.get("audio_metrics"), "thumbnail_metrics": final.get("thumbnail_metrics"),
                         "image_metrics": final.get("image_metrics"), "pump_metrics": final.get("pump_metrics")},
        "cleanup": {"graceful_application_exit": graceful_exit, "owned_processes_alive": owned.alive(),
                    "orphaned_application_processes": orphaned, "materialized_pcm_leases": leases},
        "owned_processes": owned.history,
        "observed_application_process_tree": sorted(driver.observed_processes.values(), key=lambda item: item["pid"]),
    }
    performance_errors = []
    if not limits_unmet:
        if result["measurements"]["seek_p95_ms"] is None or result["measurements"]["seek_p95_ms"] > 150:
            performance_errors.append("paused seek p95 exceeds 150ms")
        if result["measurements"]["rss_slope_mib_per_min"] is None or result["measurements"]["rss_slope_mib_per_min"] > 2:
            performance_errors.append("RSS slope exceeds 2 MiB/min")
    result["performance_errors"] = performance_errors
    result["finished"] = result["finished"] and not performance_errors
    atomic_write(args.out / "raw-measurements.json", result)
    atomic_write(args.out / "telemetry-samples.json", driver.telemetry_samples)
    if not result["finished"]:
        print(f"M2 qualification did not finish strictly: {error or performance_errors or result['cleanup']}", file=sys.stderr)
        return 1
    print(f"M2 native run finished; limits_unmet={limits_unmet}, physical_audio=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
