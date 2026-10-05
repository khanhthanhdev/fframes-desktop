#!/usr/bin/env python3
"""Build a native internal spike artifact without implying native UI qualification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


APP_BINARY = "fframes-studio"
# The agent is told `studio-tools --capability <file> <tool>` and MCP uses `studio-mcp`; the app
# finds both next to its own executable (`agent_tools::sibling_binary` in app/src/agent_tools.rs).
HELPER_BINARIES = ("studio-tools", "studio-mcp")
PACKAGED_BINARIES = (APP_BINARY, "studio_setup", *HELPER_BINARIES)


def run(args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def install_binaries(source, destination, names, suffix):
    for name in names:
        shutil.copy2(Path(source) / (name + suffix), destination)


def sibling_binary(app_binary, name, suffix):
    """The same lookup as `agent_tools::sibling_binary`: a file next to the running executable."""
    path = Path(app_binary).parent / (name + suffix)
    return path if path.is_file() else None


def verify_helpers(app_binary, suffix):
    missing = [name for name in HELPER_BINARIES if sibling_binary(app_binary, name, suffix) is None]
    if missing:
        raise ValueError(f"Project-tool helpers are not next to the application executable: {', '.join(missing)}")


def native_dependencies(bin_dir):
    tool = "ldd" if platform.system() == "Linux" else "otool"
    sections = []
    for name in (APP_BINARY, *HELPER_BINARIES):
        command = [tool, str(bin_dir / name)] if tool == "ldd" else [tool, "-L", str(bin_dir / name)]
        dependencies = run(command, capture_output=True).stdout
        if "not found" in dependencies:
            raise ValueError(f"{name} has unresolved runtime dependencies")
        sections.append(f"# {name}\n{dependencies}")
    return "\n".join(sections)


def write_inventory(output, target, sdk_included):
    files = []
    for path in sorted(output.rglob("*")):
        if path.is_file():
            with path.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            files.append({"path": path.relative_to(output).as_posix(), "size_bytes": path.stat().st_size, "sha256": digest})
    (output / "inventory.json").write_text(json.dumps({"target": target, "sdk_included": sdk_included, "files": files}, indent=2) + "\n")


def verify_sdk(bundle):
    manifest = json.loads((bundle / "compatibility.json").read_text())
    for artifact in manifest["artifacts"]:
        relative = artifact["url"].removeprefix("file://")
        path = (bundle / relative).resolve()
        if not path.is_relative_to(bundle.resolve()):
            raise ValueError("SDK artifact escapes bundle")
        if path.stat().st_size != artifact["size_bytes"]:
            raise ValueError(f"SDK artifact size mismatch: {path.name}")
        with path.open("rb") as stream:
            checksum = hashlib.file_digest(stream, "sha256").hexdigest()
        if checksum != artifact["sha256"]:
            raise ValueError(f"SDK checksum mismatch: {path.name}")
    if not {"toolchain", "ffmpeg", "framework"}.issubset({a["destination_subdir"] for a in manifest["artifacts"]}):
        raise ValueError("SDK bundle lacks toolchain, native libraries or offline framework graph")
    return manifest


def package(output, sdk_bundle=None):
    output = output.resolve()
    if output.exists():
        raise ValueError("Package destination exists; select a fresh output directory")
    target = next(line.split(": ", 1)[1] for line in run(["rustc", "-vV"], capture_output=True).stdout.splitlines() if line.startswith("host: "))
    sdk = verify_sdk(sdk_bundle) if sdk_bundle else None
    if sdk and sdk["target_triple"] != target:
        raise ValueError("SDK target does not match native application build")
    started = time.monotonic()
    desktop_manifest = ROOT / "desktop/Cargo.toml"
    run(["cargo", "build", "--locked", "--release", "--manifest-path", str(desktop_manifest), "-p", "fframes-studio", "-p", "studio-sdk"])
    metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(desktop_manifest)], capture_output=True).stdout)
    release = Path(metadata["target_directory"]) / "release"
    output.mkdir(parents=True)
    bin_dir = output / "bin"
    bin_dir.mkdir()
    suffix = ".exe" if os.name == "nt" else ""
    install_binaries(release, bin_dir, PACKAGED_BINARIES, suffix)
    verify_helpers(bin_dir / ("fframes-studio" + suffix), suffix)
    shutil.copytree(ROOT / "desktop/fixtures/annotated-video-overlay", output / "worker-source", ignore=shutil.ignore_patterns("target"))
    shutil.copytree(ROOT / "desktop/packaging/sdk/notices", output / "notices")
    shutil.copy(ROOT / "desktop/fixtures/annotated-video-overlay/media/OFL.txt", output / "notices/DM-Sans-OFL.txt")
    if sdk_bundle:
        shutil.copytree(sdk_bundle, output / "sdk")
    if os.name == "nt":
        ffmpeg = os.environ.get("FFMPEG_DIR")
        if not ffmpeg:
            raise ValueError("FFMPEG_DIR required for native Windows DLL packaging")
        dlls = list((Path(ffmpeg) / "bin").glob("*.dll"))
        if not dlls:
            raise ValueError("FFMPEG_DIR contains no runtime DLLs")
        for dll in dlls:
            shutil.copy2(dll, bin_dir)
        (output / "launch.ps1").write_text('$ErrorActionPreference = "Stop"\n$env:FFRAMES_SDK_BUNDLE = Join-Path $PSScriptRoot "sdk"\n& (Join-Path $PSScriptRoot "bin/fframes-studio.exe") spike-ui\nexit $LASTEXITCODE\n')
        (output / "launch.bat").write_text('@echo off\r\nset "FFRAMES_SDK_BUNDLE=%~dp0sdk"\r\n"%~dp0bin\\fframes-studio.exe" spike-ui %*\r\n')
    else:
        launcher = output / "launch.sh"
        launcher.write_text('#!/usr/bin/env sh\nset -eu\npackage_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexport FFRAMES_SDK_BUNDLE="$package_dir/sdk"\nexec "$package_dir/bin/fframes-studio" spike-ui\n')
        launcher.chmod(0o755)
        (output / "native-dependencies.txt").write_text(native_dependencies(bin_dir))
        if platform.system() == "Darwin":
            contents = output / "fframes Studio.app/Contents"
            (contents / "MacOS").mkdir(parents=True)
            shutil.copy2(bin_dir / "fframes-studio", contents / "MacOS/fframes-studio")
            install_binaries(bin_dir, contents / "MacOS", HELPER_BINARIES, "")
            verify_helpers(contents / "MacOS/fframes-studio", "")
            if sdk_bundle:
                shutil.move(str(output / "sdk"), str(contents / "sdk"))
            with (contents / "Info.plist").open("wb") as stream:
                plistlib.dump({"CFBundleExecutable": "fframes-studio", "CFBundleIdentifier": "studio.fframes.spike", "CFBundleName": "fframes Studio", "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "0.1.0", "NSHighResolutionCapable": True}, stream)
            launcher.write_text('#!/usr/bin/env sh\nset -eu\npackage_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexport FFRAMES_SDK_BUNDLE="$package_dir/fframes Studio.app/Contents/sdk"\nexec "$package_dir/fframes Studio.app/Contents/MacOS/fframes-studio" spike-ui\n')
    ledger = json.loads((ROOT / "desktop/qualification/m0-results.json").read_text())
    ledger["timestamp"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    ledger["target_platform"] = {"os": platform.system().lower(), "arch": target.split("-")[0], "triple": target, "status": "PENDING"}
    ledger["metrics"] = {"native_package_build_seconds": time.monotonic() - started}
    ledger["build_evidence"] = {"native_build": "PASSED", "sdk_included": bool(sdk), "interactive_qualification": "NOT_RUN", "sterile_offline_build": "NOT_RUN", "authenticated_acp": "NOT_RUN"}
    for name, gate in ledger["gates"].items():
        if name == "acp_task":
            gate["status"] = "NOT_RUN"
        else:
            gate["passed"] = False
        gate["notes"] = "Native artifact built; this gate needs its own reproducible native run."
    (output / "qualification.json").write_text(json.dumps(ledger, indent=2) + "\n")
    write_inventory(output, target, bool(sdk))
    shutil.make_archive(str(output), "zip", output.parent, output.name)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--sdk-bundle", type=Path, help="Verified native SDK; omit for an app-only artifact")
    args = parser.parse_args()
    package(args.out, args.sdk_bundle)


if __name__ == "__main__":
    main()
