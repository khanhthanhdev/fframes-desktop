#!/usr/bin/env python3
"""Native Windows packaged-app presentation, input and source-click qualification.

The Windows counterpart of qualify-linux-native.py. It runs the packaged executable's
`qualify-presentation` command (transactional SDK install, staged template/worker build and
render, then a fresh worker project built after promotion), presents all 1,000 worker frames in
the real GPUI window, and drives it with native Win32 `SendInput` keyboard and pointer events
through the painted geometry the app reports in `presentation.ready.json`.

It does not create a separate account or disable networking: both are host changes the
operator makes deliberately. The environment record states what was and was not isolated, and
a run inside a remote/virtual display is never a physical display or IME qualification.
"""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import sys
import time
import zlib

TYPED_TEXT = "native typing 123"
TIMEOUT_SECONDS = 1800

user32 = ctypes.WinDLL("user32", use_last_error=True) if os.name == "nt" else None
if user32:
    # Per-monitor DPI awareness: window, client and cursor coordinates are physical pixels.
    # A DPI-unaware process sees virtualized coordinates and would misplace every event.
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    user32.WindowFromPoint.argtypes = [wintypes.POINT]
    user32.WindowFromPoint.restype = wintypes.HWND
    user32.GetAncestor.argtypes = [wintypes.HWND, ctypes.c_uint]
    user32.GetAncestor.restype = wintypes.HWND
    user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND] + [ctypes.c_int] * 4 + [ctypes.c_uint]
    user32.GetForegroundWindow.restype = wintypes.HWND

INPUT_MOUSE, INPUT_KEYBOARD = 0, 1
MOUSEEVENTF_MOVE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP = 0x0001, 0x0002, 0x0004
MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_ABSOLUTE = 0x4000, 0x8000
KEYEVENTF_KEYUP, KEYEVENTF_UNICODE = 0x0002, 0x0004
ULONG_PTR = ctypes.c_size_t


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", wintypes.LONG), ("dy", wintypes.LONG), ("mouseData", wintypes.DWORD),
                ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD), ("dwExtraInfo", ULONG_PTR)]


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wintypes.WORD), ("wScan", wintypes.WORD), ("dwFlags", wintypes.DWORD),
                ("time", wintypes.DWORD), ("dwExtraInfo", ULONG_PTR)]


class HARDWAREINPUT(ctypes.Structure):
    _fields_ = [("uMsg", wintypes.DWORD), ("wParamL", wintypes.WORD), ("wParamH", wintypes.WORD)]


class _INPUTUNION(ctypes.Union):
    _fields_ = [("mi", MOUSEINPUT), ("ki", KEYBDINPUT), ("hi", HARDWAREINPUT)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("u", _INPUTUNION)]


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def send(*inputs):
    array = (INPUT * len(inputs))(*inputs)
    if user32.SendInput(len(inputs), array, ctypes.sizeof(INPUT)) != len(inputs):
        raise ctypes.WinError(ctypes.get_last_error())


def find_window(pid):
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def visit(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            length = user32.GetWindowTextLengthW(hwnd)
            title = ctypes.create_unicode_buffer(length + 1)
            user32.GetWindowTextW(hwnd, title, length + 1)
            if "fframes studio" in title.value.lower():
                found.append(hwnd)
        return True

    user32.EnumWindows(visit, 0)
    return found[0] if found else None


def screen_point(hwnd, logical):
    """GPUI reports logical pixels in the client area; Win32 input uses physical screen pixels."""
    scale = user32.GetDpiForWindow(hwnd) / 96.0
    point = wintypes.POINT(round(logical[0] * scale), round(logical[1] * scale))
    if not user32.ClientToScreen(hwnd, ctypes.byref(point)):
        raise ctypes.WinError(ctypes.get_last_error())
    return point.x, point.y


def move_cursor(x, y):
    """SetCursorPos can fail transiently while a remote session repaints; retry, then fall
    back to an absolute SendInput move across the virtual desktop."""
    for _ in range(10):
        if user32.SetCursorPos(x, y):
            return
        time.sleep(0.5)
    left, top = user32.GetSystemMetrics(76), user32.GetSystemMetrics(77)
    width, height = user32.GetSystemMetrics(78), user32.GetSystemMetrics(79)
    dx = round((x - left) * 65535 / max(width - 1, 1))
    dy = round((y - top) * 65535 / max(height - 1, 1))
    send(INPUT(INPUT_MOUSE, _INPUTUNION(mi=MOUSEINPUT(dx, dy, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, 0, 0))))
    position = wintypes.POINT()
    user32.GetCursorPos(ctypes.byref(position))
    if abs(position.x - x) > 2 or abs(position.y - y) > 2:
        raise RuntimeError(f"Cursor did not reach ({x}, {y}); at ({position.x}, {position.y})")


def raise_window(hwnd):
    """Keep the window above others: real pointer events go to whatever is on top."""
    swp_nomove, swp_nosize, swp_showwindow = 0x0002, 0x0001, 0x0040
    user32.SetWindowPos(hwnd, wintypes.HWND(-1), 0, 0, 0, 0, swp_nomove | swp_nosize | swp_showwindow)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.3)


def click(hwnd, logical):
    raise_window(hwnd)
    x, y = screen_point(hwnd, logical)
    target = user32.WindowFromPoint(wintypes.POINT(x, y))
    if user32.GetAncestor(target, 2) != hwnd:  # GA_ROOT
        raise RuntimeError(f"Point ({x}, {y}) is not inside the studio window")
    move_cursor(x, y)
    time.sleep(0.1)
    send(INPUT(INPUT_MOUSE, _INPUTUNION(mi=MOUSEINPUT(0, 0, 0, MOUSEEVENTF_LEFTDOWN, 0, 0))),
         INPUT(INPUT_MOUSE, _INPUTUNION(mi=MOUSEINPUT(0, 0, 0, MOUSEEVENTF_LEFTUP, 0, 0))))
    time.sleep(0.2)


def type_text(text):
    for character in text:
        code = ord(character)
        send(INPUT(INPUT_KEYBOARD, _INPUTUNION(ki=KEYBDINPUT(0, code, KEYEVENTF_UNICODE, 0, 0))),
             INPUT(INPUT_KEYBOARD, _INPUTUNION(ki=KEYBDINPUT(0, code, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP, 0, 0))))
        time.sleep(0.03)


def screenshot(hwnd, output):
    """Captures only the studio window's own composed content (PrintWindow with
    PW_RENDERFULLCONTENT), never other windows or the desktop, and writes a PNG."""
    gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
    handle = ctypes.c_void_p
    user32.GetWindowDC.restype = handle
    user32.ReleaseDC.argtypes = [wintypes.HWND, handle]
    user32.PrintWindow.argtypes = [wintypes.HWND, handle, ctypes.c_uint]
    gdi32.CreateCompatibleDC.restype = gdi32.CreateCompatibleBitmap.restype = gdi32.SelectObject.restype = handle
    gdi32.CreateCompatibleDC.argtypes = [handle]
    gdi32.CreateCompatibleBitmap.argtypes = [handle, ctypes.c_int, ctypes.c_int]
    gdi32.SelectObject.argtypes = [handle, handle]
    gdi32.DeleteObject.argtypes = [handle]
    gdi32.DeleteDC.argtypes = [handle]
    gdi32.GetDIBits.argtypes = [handle, handle, ctypes.c_uint, ctypes.c_uint, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint]
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    width, height = rect.right - rect.left, rect.bottom - rect.top
    window_dc = user32.GetWindowDC(hwnd)
    memory_dc = gdi32.CreateCompatibleDC(window_dc)
    bitmap = gdi32.CreateCompatibleBitmap(window_dc, width, height)
    gdi32.SelectObject(memory_dc, bitmap)
    try:
        if not user32.PrintWindow(hwnd, memory_dc, 2):
            raise ctypes.WinError(ctypes.get_last_error())
        header = (ctypes.c_uint32 * 10)(40, width, ctypes.c_uint32(-height).value, 1 | (32 << 16), 0, 0, 0, 0, 0, 0)
        pixels = ctypes.create_string_buffer(width * height * 4)
        if gdi32.GetDIBits(memory_dc, bitmap, 0, height, pixels, header, 0) != height:
            raise RuntimeError("GetDIBits failed")
    finally:
        gdi32.DeleteObject(bitmap)
        gdi32.DeleteDC(memory_dc)
        user32.ReleaseDC(hwnd, window_dc)
    raw = bytearray()
    for row in range(height):
        bgra = pixels.raw[row * width * 4:(row + 1) * width * 4]
        rgb = bytearray(width * 3)
        rgb[0::3], rgb[1::3], rgb[2::3] = bgra[2::4], bgra[1::4], bgra[0::4]
        raw += b"\x00" + rgb

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    Path(output).write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                             + chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b""))


def normalize_paths(text, replacements):
    """Private absolute paths become placeholders (in plain, JSON-escaped and slash forms);
    any other drive path is reduced to `<path>` so evidence never names a person's machine."""
    for prefix, placeholder in sorted(replacements.items(), key=lambda item: -len(item[0])):
        for form in (prefix, prefix.replace("\\", "\\\\"), prefix.replace("\\", "/")):
            text = re.sub(re.escape(form), lambda _: placeholder, text, flags=re.IGNORECASE)
    return re.sub(r"(?<![A-Za-z0-9])[A-Za-z]:(?:\\\\|\\|/)[^\s\"'<>|]*", "<path>", text)


def source_identity():
    root = Path(__file__).resolve().parents[2]
    commit = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    diff = subprocess.run(["git", "-C", str(root), "diff", "HEAD", "--", "desktop"], capture_output=True).stdout
    return {"commit": commit or None, "desktop_worktree_diff_sha256": hashlib.sha256(diff).hexdigest() if diff else None}


def owned_processes(pid):
    """Live descendants of `pid` (the worker, compilers) observed through CIM."""
    script = ("$all = Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name;"
              "$all | ConvertTo-Json -Compress")
    rows = json.loads(subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
                                     check=True, capture_output=True, text=True, timeout=120).stdout or "[]")
    if isinstance(rows, dict):
        rows = [rows]
    seen, frontier = {pid}, [pid]
    children = []
    while frontier:
        parent = frontier.pop()
        for row in rows:
            if row["ParentProcessId"] == parent and row["ProcessId"] not in seen:
                seen.add(row["ProcessId"])
                frontier.append(row["ProcessId"])
                children.append(row)
    return children


def qualify(args):
    if os.name != "nt":
        raise SystemExit("qualify-windows-native.py runs only on Windows")
    if args.out.exists():
        raise ValueError("Evidence directory already exists; use a fresh --out")
    package = args.package
    executable = package / "bin" / "fframes-studio.exe"
    bundle = package / "sdk"
    for required in (executable, bundle / "compatibility.json"):
        if not required.is_file():
            raise ValueError(f"Packaged artifact is incomplete: {required}")
    args.out.mkdir(parents=True)
    home = args.out / "home"
    home.mkdir()
    # A user session's system variables only, with a fresh home and Cargo/rustup locations:
    # no developer cache, Rust install or Visual Studio developer prompt participates.
    environment = {key: os.environ[key] for key in ("SystemRoot", "SystemDrive", "WINDIR", "ComSpec", "PATHEXT",
                                                     "ProgramData", "ProgramFiles", "ProgramFiles(x86)",
                                                     "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "OS")
                   if key in os.environ}
    environment.update({
        "PATH": os.pathsep.join([str(package / "bin"), os.path.join(os.environ["SystemRoot"], "System32"),
                                 os.environ["SystemRoot"], os.path.join(os.environ["SystemRoot"], "System32", "WindowsPowerShell", "v1.0")]),
        "USERPROFILE": str(home), "HOME": str(home),
        "APPDATA": str(home / "AppData/Roaming"), "LOCALAPPDATA": str(home / "AppData/Local"),
        "TEMP": str(home / "Temp"), "TMP": str(home / "Temp"),
        "CARGO_HOME": str(home / ".cargo"), "RUSTUP_HOME": str(home / ".rustup"),
    })
    for directory in ("AppData/Roaming", "AppData/Local", "Temp"):
        (home / directory).mkdir(parents=True)
    command = [str(executable), "qualify-presentation", "--bundle", str(bundle), "--sdk-home", str(home / ".fframes/sdk"),
               "--project", str(home / "video"), "--output", str(args.out / "presentation.json")]
    started = time.monotonic()
    typed = clicked = False
    peak_children = []
    with (args.out / "application.log").open("w") as log:
        application = subprocess.Popen(command, env=environment, cwd=args.out, stdout=log, stderr=subprocess.STDOUT,
                                       creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
        try:
            while application.poll() is None:
                if time.monotonic() - started > TIMEOUT_SECONDS:
                    raise TimeoutError("Native qualification exceeded 30 minutes")
                ready_path = args.out / "presentation.ready.json"
                hwnd = find_window(application.pid)
                if hwnd and ready_path.is_file():
                    ready = json.loads(ready_path.read_text())
                    if not typed:
                        # The click activates the window and focuses the input; keystrokes
                        # go to the foreground window, so wait until it really is ours.
                        for _ in range(5):
                            click(hwnd, ready["input_click"])
                            time.sleep(0.5)
                            if user32.GetForegroundWindow() == hwnd:
                                break
                        else:
                            raise RuntimeError("The studio window never became the foreground window")
                        type_text(TYPED_TEXT)
                        typed = True
                    if not clicked:
                        # Real pointer event through the preview's letterboxed geometry.
                        click(hwnd, ready["source_click"])
                        clicked = True
                        screenshot(hwnd, args.out / "native-window.png")
                        peak_children = owned_processes(application.pid)
                time.sleep(1)
        finally:
            if application.poll() is None:
                application.kill()
                application.wait()
    elapsed = time.monotonic() - started
    if application.returncode != 0:
        sys.stderr.write((args.out / "application.log").read_text(errors="replace")[-8192:])
        raise RuntimeError(f"Native application failed ({application.returncode}); inspect application.log")
    record = json.loads((args.out / "presentation.json").read_text())
    if not record["completed"] or record["confirmed_presentations"] != 1000 or record["verified_render_requests"] != 2000:
        raise ValueError("Incomplete native presentation evidence")
    if record["native_input_observed"] != TYPED_TEXT:
        raise ValueError(f"Native keyboard input was not observed: {record['native_input_observed']!r}")
    if not record["selected_source"] or record["selected_source"]["element_id"] != "intro.title":
        raise ValueError("Native preview click did not select the marked title")
    leftovers = owned_processes(application.pid)
    session = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command",
                              "(Get-Process -Id $PID).SessionId; (Get-CimInstance Win32_VideoController).Name -join '; '"],
                             capture_output=True, text=True, timeout=60).stdout.split("\n")
    (args.out / "environment.json").write_text(json.dumps({
        "os": platform.platform(), "machine": platform.machine(),
        "session_id": session[0].strip() if session else None,
        "display_adapters": session[1].strip() if len(session) > 1 else None,
        "display": "interactive Windows desktop session (remote/virtual adapter)",
        "physical_display": False, "ime_composition": "NOT_RUN",
        "fresh_home": True, "fresh_cargo_and_rustup_homes": True,
        "separate_account": False, "external_network_disabled": False,
        "application_sha256": digest(executable),
        "sdk_manifest_sha256": digest(bundle / "compatibility.json"),
        "candidate_template_and_worker_build_render": "PASSED",
        "fresh_worker_build_after_promotion": "PASSED",
        "native_keyboard_input": "PASSED", "native_source_click": "PASSED",
        "descendants_while_presenting": len(peak_children),
        "descendants_after_close": len(leftovers),
        "elapsed_seconds": round(elapsed, 3),
        "source": source_identity(),
        "harness_sha256": digest(__file__),
    }, indent=2) + "\n", newline="\n")
    replacements = {str(home): "<home>", str(args.out): "<work>", str(package): "<package>",
                    str(Path.home()): "<user-home>"}
    # These contain spaces, which end the generic drive-path pattern in normalize_paths.
    for variable in ("ProgramFiles(x86)", "ProgramFiles", "ProgramData", "SystemRoot"):
        if os.environ.get(variable):
            replacements[os.environ[variable]] = f"<{variable.lower()}>"
    for name in ("application.log", "presentation.json", "presentation.ready.json", "environment.json"):
        evidence = args.out / name
        if evidence.is_file():
            evidence.write_text(normalize_paths(evidence.read_text(errors="replace"), replacements), newline="\n")
    if leftovers:
        raise RuntimeError(f"Processes outlived the application: {leftovers}")
    print("Native Windows worker presentation, input and source click passed; physical display/IME and isolation remain unqualified")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--package", required=True, type=Path, help="Extracted package directory (contains bin/ and sdk/)")
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    args.package = args.package.resolve()
    args.out = args.out.resolve()
    qualify(args)
