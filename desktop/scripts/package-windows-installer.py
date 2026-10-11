#!/usr/bin/env python3
"""Build the per-user Windows Setup.exe from a native Windows package directory."""
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "desktop/packaging/windows/fframes-studio.iss"
INNO_SETUP_VERSION = "7.1.0"
REQUIRED = (
    "bin/fframes-studio.exe",
    "bin/studio-tools.exe",
    "bin/studio-mcp.exe",
    "bin/studio_setup.exe",
    "sdk/compatibility.json",
    "notices/LICENSE.txt",
    "qualification.json",
)


def app_version():
    manifest = tomllib.loads((ROOT / "desktop/app/Cargo.toml").read_text())
    version = manifest["package"]["version"]
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:[-+][0-9A-Za-z.-]+)?", version)
    if not match:
        raise ValueError(f"App version is not a supported release version: {version}")
    # Windows file versions are four numbers; prerelease labels stay in AppVersion only.
    return version, ".".join(match.groups()) + ".0"


def is_reparse_point(path):
    return path.is_symlink() or bool(getattr(path.lstat(), "st_file_attributes", 0) & 0x400)


def validate_source(source):
    if not source.is_dir() or is_reparse_point(source):
        raise ValueError(f"native Windows package directory does not exist: {source}")
    missing = [name for name in REQUIRED if not (source / name).is_file()]
    if missing:
        raise ValueError(f"native Windows package is missing: {', '.join(missing)}")
    for path in source.rglob("*"):
        if is_reparse_point(path):
            raise ValueError(f"package must not contain links or reparse points: {path.relative_to(source)}")


def compiler(explicit):
    candidate = explicit or os.environ.get("INNO_SETUP_COMPILER") or shutil.which("ISCC.exe") or shutil.which("iscc")
    if not candidate or not Path(candidate).is_file():
        raise ValueError("Inno Setup compiler (ISCC.exe) not found; pass --iscc or set INNO_SETUP_COMPILER")
    reported = subprocess.run([candidate, "--version"], capture_output=True, text=True, errors="replace").stdout.strip()
    if reported != INNO_SETUP_VERSION:
        raise ValueError(f"Inno Setup {INNO_SETUP_VERSION} is required (pinned); {candidate} reports {reported or 'no version'}")
    return candidate


def package(source, output, iscc=None, sign_command=None):
    source = Path(source).resolve()
    output = Path(output).resolve()
    if output.suffix.lower() != ".exe":
        raise ValueError(f"installer output must be an .exe path: {output}")
    if output.exists():
        raise ValueError(f"output already exists; choose a fresh path: {output}")
    validate_source(source)
    version, version_info = app_version()
    iscc = compiler(iscc)

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".fframes-studio-setup-", dir=output.parent) as temporary:
        command = [
            iscc,
            "/Q",
            f"/DAppVersion={version}",
            f"/DVersionInfo={version_info}",
            f"/DSourceDir={source}",
            f"/DOutputDir={temporary}",
            "/DOutputBaseFilename=candidate",
        ]
        if sign_command:
            # ISCC substitutes $f with the file to sign (Setup.exe and its uninstaller).
            command += ["/DSignTool", f"/Srelease={sign_command}"]
        subprocess.run([*command, str(SCRIPT)], check=True)
        candidate = Path(temporary) / "candidate.exe"
        if not candidate.is_file() or candidate.read_bytes()[:2] != b"MZ":
            raise ValueError("Inno Setup did not produce a Windows executable")
        try:
            os.link(candidate, output)
        except FileExistsError as error:
            raise ValueError(f"output appeared while packaging; existing bytes were preserved: {output}") from error
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="native Windows package directory")
    parser.add_argument("--out", type=Path, required=True, help="new Setup .exe output path")
    parser.add_argument("--iscc", help=f"path to the Inno Setup {INNO_SETUP_VERSION} ISCC.exe")
    parser.add_argument("--sign-command", help="signtool command line with $f for the file to sign")
    args = parser.parse_args()
    print(package(args.source, args.out, args.iscc, args.sign_command))


if __name__ == "__main__":
    main()
