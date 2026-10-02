#!/usr/bin/env python3
"""Run the real X11 presentation workload, optionally under an isolated account/network."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def stop(process):
    if process is None or process.poll() is not None:
        return
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def enter(args):
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    # This namespace has no external network interface; loopback serves worker IPC.
    environment = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(args.out / "home"), "LANG": "C.UTF-8", "DISPLAY": args.display,
                   "XDG_RUNTIME_DIR": str(args.out / "runtime"), "LIBGL_ALWAYS_SOFTWARE": "1"}
    if "LIBCLANG_PATH" in os.environ:
        environment["LIBCLANG_PATH"] = os.environ["LIBCLANG_PATH"]
    os.execvpe("setpriv", ["setpriv", "--reuid=65534", "--regid=65534", "--clear-groups", str(args.out / "fframes-studio"),
                            "qualify-presentation", "--bundle", str(args.bundle), "--sdk-home", str(args.out / "home/.fframes/sdk"),
                            "--project", str(args.out / "home/video"), "--output", str(args.out / "presentation.json")], environment)


def qualify(args):
    if args.out.exists():
        raise ValueError("Evidence directory exists; choose a fresh directory")
    if args.isolated_user and os.geteuid() != 0:
        raise ValueError("Isolated-user/network mode requires root for setpriv and unshare")
    if not args.display.startswith(":") or not args.display[1:].isdigit():
        raise ValueError("Use a local numeric X11 display")
    display_number = args.display[1:]
    if Path(f"/tmp/.X{display_number}-lock").exists():
        raise ValueError("Requested display already has an owner; stop/reuse that owner explicitly")
    args.out.mkdir(parents=True)
    for directory in ["home", "runtime"]:
        (args.out / directory).mkdir(mode=0o700)
    shutil.copy2(args.application, args.out / "fframes-studio")
    shutil.copy2(Path(__file__), args.out / "qualify-linux-native.py")
    if args.isolated_user:
        for target in [args.out, args.bundle, args.application]:
            curr = target.resolve()
            while curr != curr.parent:
                try:
                    mode = curr.stat().st_mode
                    if not (mode & 0o001):
                        curr.chmod(mode | 0o005)
                except Exception:
                    pass
                curr = curr.parent
        for root_dir, dirs, files in os.walk(args.bundle):
            for d in dirs:
                p = Path(root_dir) / d
                try:
                    p.chmod(p.stat().st_mode | 0o005)
                except Exception:
                    pass
            for f in files:
                p = Path(root_dir) / f
                try:
                    p.chmod(p.stat().st_mode | 0o004)
                except Exception:
                    pass
        os.chown(args.out, 65534, 65534)
        args.out.chmod(0o775)
        for root_dir, dirs, files in os.walk(args.out):
            for d in dirs:
                p = Path(root_dir) / d
                try:
                    os.chown(p, 65534, 65534)
                    p.chmod(p.stat().st_mode | 0o775)
                except Exception:
                    pass
            for f in files:
                p = Path(root_dir) / f
                try:
                    os.chown(p, 65534, 65534)
                    p.chmod(p.stat().st_mode | 0o775)
                except Exception:
                    pass
    environment = os.environ.copy()
    environment.update(DISPLAY=args.display, XDG_RUNTIME_DIR=str(args.out / "runtime"), LIBGL_ALWAYS_SOFTWARE="1")
    compositor = application = None
    started = time.monotonic()
    typed = clicked = False
    with (args.out / "compositor.log").open("w") as compositor_log, (args.out / "application.log").open("w") as application_log:
        try:
            compositor = subprocess.Popen(["Xvfb", args.display, "-screen", "0", "1280x1200x24", "-nolisten", "tcp"], stdout=compositor_log, stderr=subprocess.STDOUT, start_new_session=True)
            time.sleep(1)
            if compositor.poll() is not None:
                raise RuntimeError("Xvfb startup failed")
            if args.isolated_user:
                # Separate worker/build process groups still belong to this PID
                # namespace and are reaped if the qualification wrapper exits.
                command = ["unshare", "--net", "--pid", "--fork", "--mount-proc", "--kill-child=SIGKILL", sys.executable, str(args.out / "qualify-linux-native.py"), "--enter", "--out", str(args.out), "--bundle", str(args.bundle), "--display", args.display]
            else:
                environment["HOME"] = str(args.out / "home")
                command = [str(args.out / "fframes-studio"), "qualify-presentation", "--bundle", str(args.bundle), "--sdk-home", str(args.out / "home/.fframes/sdk"), "--project", str(args.out / "home/video"), "--output", str(args.out / "presentation.json")]
            application = subprocess.Popen(command, env=environment, cwd=args.out, stdout=application_log, stderr=subprocess.STDOUT, start_new_session=True)
            while application.poll() is None:
                if time.monotonic() - started > 900:
                    raise TimeoutError("Native qualification exceeded 15 minutes")
                search = subprocess.run(["xdotool", "search", "--name", "fframes studio"], env=environment, capture_output=True, text=True)
                ready_path = args.out / "presentation.ready.json"
                if search.returncode == 0 and search.stdout.strip() and ready_path.is_file():
                    window = search.stdout.splitlines()[0]
                    ready = json.loads(ready_path.read_text())
                    if not typed:
                        x, y = [str(round(value)) for value in ready["input_click"]]
                        subprocess.run(["xdotool", "mousemove", "--window", window, x, y, "click", "1", "type", "--clearmodifiers", "native typing 123"], env=environment, check=True)
                        typed = True
                    if not clicked:
                        # Real pointer event through the preview's letterboxed geometry.
                        x, y = [str(round(value)) for value in ready["source_click"]]
                        subprocess.run(["xdotool", "mousemove", "--window", window, x, y, "click", "1"], env=environment, check=True)
                        clicked = True
                        from PIL import ImageGrab
                        ImageGrab.grab(xdisplay=args.display).save(args.out / "native-window.png")
                time.sleep(1)
            if application.returncode != 0:
                log_path = args.out / "application.log"
                if log_path.is_file():
                    sys.stderr.write(f"\n=== application.log ===\n{log_path.read_text()}\n======================\n")
                presentation_path = args.out / "presentation.json"
                if presentation_path.is_file():
                    sys.stderr.write(f"\n=== presentation.json ===\n{presentation_path.read_text()}\n======================\n")
                compositor_log_path = args.out / "compositor.log"
                if compositor_log_path.is_file():
                    sys.stderr.write(f"\n=== compositor.log ===\n{compositor_log_path.read_text()[-4096:]}\n======================\n")
                raise RuntimeError(f"Native application failed ({application.returncode}); inspect application.log")
            record = json.loads((args.out / "presentation.json").read_text())
            if not record["completed"] or record["confirmed_presentations"] != 1000 or record["verified_render_requests"] != 2000:
                raise ValueError("Incomplete native presentation evidence")
            if record["native_input_observed"] != "native typing 123":
                raise ValueError("Native keyboard input was not observed by the registered component")
            if not record["selected_source"] or record["selected_source"]["element_id"] != "intro.title":
                raise ValueError("Native preview click did not select the marked title")
            (args.out / "environment.json").write_text(json.dumps({"display": args.display, "compositor": "Xvfb", "software_renderer_requested": True,
                "isolated_uid": 65534 if args.isolated_user else os.geteuid(), "fresh_home": True, "external_network_disabled": args.isolated_user,
                "owned_pid_namespace": args.isolated_user,
                "application_sha256": digest(args.out / "fframes-studio"),
                "sdk_manifest_sha256": digest(args.bundle / "compatibility.json"),
                "candidate_template_and_worker_build_render": "PASSED",
                "fresh_worker_build_after_promotion": "PASSED",
                "physical_display": False, "ime_composition": "NOT_RUN", "native_keyboard_input": "PASSED", "native_source_click": "PASSED"}, indent=2) + "\n")
            print("Native X11 worker stress, input and source click passed; physical display/IME remain unqualified")
        finally:
            stop(application)
            stop(compositor)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--application", type=Path)
    parser.add_argument("--bundle", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--display", default=":94")
    parser.add_argument("--isolated-user", action="store_true")
    parser.add_argument("--enter", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.out = args.out.resolve()
    args.bundle = args.bundle.resolve()
    if args.enter:
        enter(args)
    else:
        if not args.application:
            parser.error("--application is required")
        args.application = args.application.resolve()
        qualify(args)
