#!/usr/bin/env python3
"""Add winget and Scoop manifests, install.ps1, checksums and notes to an unsigned preview release."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import tomllib

ROOT = Path(__file__).resolve().parents[2]
INSTALL_SCRIPT = ROOT / "desktop/packaging/windows/install.ps1"
INNO_SCRIPT = ROOT / "desktop/packaging/windows/fframes-studio.iss"
DEFAULT_REPOSITORY = "khanhthanhdev/fframes-desktop"
WINDOWS_TARGET = "x86_64-pc-windows-msvc"
SETUP = f"fframes-studio-{WINDOWS_TARGET}-setup.exe"
WINDOWS_ZIP = f"fframes-studio-{WINDOWS_TARGET}.zip"
SCOOP_MANIFEST = "fframes-studio.json"
WINGET_ID = "khanhthanhdev.fframesStudio"
WINGET_MANIFEST_VERSION = "1.12.0"
PUBLISHER = "khanhthanhdev"
DESCRIPTION = "Native desktop studio for authoring fframes videos with coding agents"
BUILD_TOOLS = "winget install Microsoft.VisualStudio.BuildTools --override \"--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended\"; winget install LLVM.LLVM"
LABELS = {
    "x86_64-pc-windows-msvc": "Windows x64",
    "x86_64-unknown-linux-gnu": "Linux x64",
    "aarch64-apple-darwin": "macOS Apple Silicon",
}


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def release_version(tag):
    match = re.fullmatch(r"v(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)", tag)
    if not match:
        raise ValueError(f"release tag must look like v1.2.3: {tag}")
    version = match.group(1)
    app_version = tomllib.loads((ROOT / "desktop/app/Cargo.toml").read_text())["package"]["version"]
    if version != app_version:
        raise ValueError(f"tag {tag} does not match the app version {app_version} in desktop/app/Cargo.toml")
    return version


def product_code():
    match = re.search(r"^AppId=\{(\{[0-9A-F-]{36}\})$", INNO_SCRIPT.read_text(), re.MULTILINE)
    if not match:
        raise ValueError("installer script has no AppId")
    return f"{match.group(1)}_is1"


def yaml_string(value):
    return "'" + value.replace("'", "''") + "'"


def winget_manifests(version, repository, setup_url, setup_sha256):
    header = "# yaml-language-server: $schema=https://aka.ms/winget-manifest.{}.%s.schema.json\n\n" % WINGET_MANIFEST_VERSION
    identity = f"PackageIdentifier: {WINGET_ID}\nPackageVersion: {version}\n"
    footer = "ManifestType: {}\nManifestVersion: %s\n" % WINGET_MANIFEST_VERSION
    code = yaml_string(product_code())
    return {
        f"{WINGET_ID}.yaml": header.format("version") + identity + "DefaultLocale: en-US\n" + footer.format("version"),
        f"{WINGET_ID}.installer.yaml": header.format("installer") + identity + (
            "InstallerLocale: en-US\n"
            "InstallerType: inno\n"
            "Scope: user\n"
            "InstallModes:\n- interactive\n- silent\n- silentWithProgress\n"
            "UpgradeBehavior: install\n"
            "MinimumOSVersion: 10.0.0.0\n"
            f"ProductCode: {code}\n"
            "AppsAndFeaturesEntries:\n"
            f"- DisplayName: fframes Studio\n  Publisher: {PUBLISHER}\n  ProductCode: {code}\n"
            "InstallationMetadata:\n"
            "  DefaultInstallLocation: '%LocalAppData%\\Programs\\fframes Studio'\n"
            "Installers:\n"
            f"- Architecture: x64\n  InstallerUrl: {setup_url}\n  InstallerSha256: {setup_sha256.upper()}\n"
        ) + footer.format("installer"),
        f"{WINGET_ID}.locale.en-US.yaml": header.format("defaultLocale") + identity + (
            "PackageLocale: en-US\n"
            f"Publisher: {PUBLISHER}\n"
            f"PublisherUrl: https://github.com/{repository}\n"
            f"PublisherSupportUrl: https://github.com/{repository}/issues\n"
            "PackageName: fframes Studio\n"
            f"PackageUrl: https://github.com/{repository}\n"
            "License: MIT\n"
            f"LicenseUrl: https://github.com/{repository}/blob/main/LICENSE.txt\n"
            f"ShortDescription: {yaml_string(DESCRIPTION)}\n"
            "Tags:\n- video\n- rust\n- animation\n- motion-graphics\n"
            f"ReleaseNotesUrl: https://github.com/{repository}/releases/tag/v{version}\n"
        ) + footer.format("defaultLocale"),
    }


def scoop_manifest(version, repository, zip_url, zip_sha256):
    release = f"https://github.com/{repository}/releases/download/v$version"
    return {
        "version": version,
        "description": DESCRIPTION,
        "homepage": f"https://github.com/{repository}",
        "license": "MIT",
        "notes": f"Compiling projects also needs Visual Studio Build Tools (C++) and LLVM: {BUILD_TOOLS}",
        "architecture": {
            "64bit": {
                "url": zip_url,
                "hash": zip_sha256,
                "extract_dir": f"fframes-studio-{WINDOWS_TARGET}",
            }
        },
        "bin": [["bin\\fframes-studio.exe", "fframes-studio"]],
        "shortcuts": [["bin\\fframes-studio.exe", "fframes Studio"]],
        "checkver": {"github": f"https://github.com/{repository}"},
        "autoupdate": {
            "architecture": {"64bit": {"url": f"{release}/{WINDOWS_ZIP}"}},
            "hash": {"url": f"{release}/SHA256SUMS.txt"},
        },
    }


def release_notes(tag, repository, packages, sums):
    latest = f"https://github.com/{repository}/releases/latest/download"
    lines = [
        f"# fframes Studio {tag} (unsigned preview)",
        "",
        "Native desktop studio for authoring fframes videos with coding agents.",
        "",
        "> These builds are **not code-signed** and have not passed release qualification.",
        "> Windows SmartScreen shows \"Windows protected your PC\": choose **More info**, then **Run anyway**.",
        "> The `irm` and Scoop routes download outside the browser, so that prompt does not appear.",
        "",
        "## Install on Windows",
        "",
        "PowerShell, per user, no administrator rights:",
        "",
        "```powershell",
        f"irm {latest}/install.ps1 | iex",
        "```",
        "",
        "Scoop:",
        "",
        "```powershell",
        f"scoop install {latest}/{SCOOP_MANIFEST}",
        "```",
        "",
        f"winget (once `{WINGET_ID}` is accepted into the winget community repository):",
        "",
        "```powershell",
        f"winget install {WINGET_ID}",
        "```",
        "",
        f"Or download `{SETUP}` below and double-click it. `{WINDOWS_ZIP}` is the portable folder (run `launch.bat`).",
        "",
        "Compiling projects also needs Visual Studio Build Tools (C++ workload) and LLVM; the app's setup screen checks for them:",
        "",
        "```powershell",
        BUILD_TOOLS,
        "```",
        "",
        "Uninstall from Settings > Apps (or `scoop uninstall fframes-studio`); projects, the managed SDK and app data are kept.",
        "",
        "## Downloads",
        "",
    ]
    for name in packages:
        target = next((t for t in LABELS if t in name), None)
        kind = "installer" if name.endswith("-setup.exe") else "Debian package" if name.endswith(".deb") else "portable ZIP"
        lines.append(f"- {LABELS.get(target, 'Other')} {kind}: `{name}`")
    lines += ["", "## SHA-256", "", "```text", sums.rstrip(), "```", ""]
    return "\n".join(lines)


def prepare(tag, dist, repository=DEFAULT_REPOSITORY):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError(f"repository must be owner/name: {repository}")
    version = release_version(tag)
    dist = Path(dist)
    for name in (SETUP, WINDOWS_ZIP):
        if not (dist / name).is_file():
            raise ValueError(f"release directory is missing {name}")
    generated = [SCOOP_MANIFEST, "install.ps1", "SHA256SUMS.txt", "release-notes.md", *winget_manifests(version, repository, "", "")]
    existing = [name for name in generated if (dist / name).exists()]
    if existing:
        raise ValueError(f"release directory already contains generated files: {', '.join(existing)}")
    packages = sorted(
        path.name for path in dist.iterdir()
        if path.is_file() and path.name.startswith("fframes-studio-") and path.suffix in {".zip", ".exe", ".deb"}
    )
    base = f"https://github.com/{repository}/releases/download/{tag}"

    for name, text in winget_manifests(version, repository, f"{base}/{SETUP}", sha256(dist / SETUP)).items():
        (dist / name).write_text(text, newline="\n")
    scoop = scoop_manifest(version, repository, f"{base}/{WINDOWS_ZIP}", sha256(dist / WINDOWS_ZIP))
    (dist / SCOOP_MANIFEST).write_text(json.dumps(scoop, indent=4) + "\n", newline="\n")
    script = INSTALL_SCRIPT.read_text()
    marker = f'[string]$Repository = "{DEFAULT_REPOSITORY}"'
    if script.count(marker) != 1:
        raise ValueError("install.ps1 has no default repository line")
    (dist / "install.ps1").write_text(
        script.replace(DEFAULT_REPOSITORY, repository), newline="\r\n", encoding="ascii"
    )

    assets = sorted(path.name for path in dist.iterdir() if path.is_file() and path.name != "release-notes.md")
    sums = "".join(f"{sha256(dist / name)}  {name}\n" for name in assets)
    (dist / "SHA256SUMS.txt").write_text(sums, newline="\n")
    (dist / "release-notes.md").write_text(release_notes(tag, repository, packages, sums), newline="\n")
    return dist


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True, help="release tag, v<app version>")
    parser.add_argument("--dist", type=Path, required=True, help="directory holding the release packages")
    parser.add_argument("--repository", default=DEFAULT_REPOSITORY, help="GitHub owner/name serving the downloads")
    args = parser.parse_args()
    prepare(args.tag, args.dist, args.repository)
    print(f"prepared {args.dist}")


if __name__ == "__main__":
    main()
